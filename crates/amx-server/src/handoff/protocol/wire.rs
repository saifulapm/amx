//! What crosses the handoff socket, and the token that opens it.
//!
//! Every message is one line of JSON except the descriptor messages, which are
//! [`fd`](crate::handoff::fd)'s. The two share a socket, and that is the whole
//! reason [`read_line`] reads the way it does: a descriptor rides beside a
//! byte, so a reader that pulls in more bytes than its line needs can swallow
//! the byte a descriptor is attached to, and the kernel closes that descriptor
//! with no error anywhere. Reading a line therefore peeks for the newline and
//! then consumes exactly up to it, never one byte further.

use std::io::{self, Read, Write};
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;

use rustix::io::Errno;
use rustix::net::{RecvFlags, recv};
use serde::{Deserialize, Serialize};

/// Cap on a stage line: the handshake lines are tens of bytes, and an abort
/// reason is a sentence.
pub const MAX_STAGE_LINE: usize = 4 << 10;

/// Cap on the manifest line.
///
/// The manifest's own budget is per pane — 500 rows and 256 KiB of packed
/// scrollback each (D-M3-4), plus the session snapshot — so this is not the
/// number a real session is measured against. It bounds a corrupt or hostile
/// peer, and it sits where a full 256-pane handoff (the width of
/// [`MAX_PANES`](crate::handoff::fd::MAX_PANES)) still fits under it.
pub const MAX_MANIFEST_LINE: usize = 96 << 20;

/// Bytes a single peek asks for while hunting the newline.
const PEEK_CHUNK: usize = 8 << 10;

/// One line of the handoff protocol.
///
/// The manifest rides as an opaque [`serde_json::Value`]: its shape is the
/// manifest codec's (W04) and its read window is checked by the importer
/// against the manifest's own version field, separately from this protocol
/// (D-M3-6 point 2). This module carries it and does not read it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "stage", rename_all = "snake_case")]
pub enum Line {
    /// Step 2: the importer proves it is the process the exporter spawned.
    Token {
        /// The secret, as it came off the importer's stdin.
        token: String,
    },
    /// Step 5: the session, as bytes.
    Manifest {
        /// The manifest codec's own document.
        manifest: serde_json::Value,
    },
    /// Step 7: the manifest reads, the descriptors may come.
    Validated,
    /// Step 10: the successor is assembled and touching nothing.
    Restored,
    /// Step 12: the successor holds the session socket.
    Ready,
    /// Step 13: ownership has transferred.
    Committed,
    /// Step 14: advisory — the successor is serving.
    Owned,
    /// Any step: this handoff is over and nothing partial will serve.
    Abort {
        /// Why, for `session report` and for the user.
        reason: String,
    },
}

impl Line {
    /// What to call this line when a peer sent it out of order.
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Token { .. } => "token",
            Self::Manifest { .. } => "manifest",
            Self::Validated => "validated",
            Self::Restored => "restored",
            Self::Ready => "ready",
            Self::Committed => "committed",
            Self::Owned => "owned",
            Self::Abort { .. } => "abort",
        }
    }
}

/// What a line failed at, before a stage is attached to it.
#[derive(Debug, thiserror::Error)]
pub enum WireError {
    /// The peer closed the socket rather than sending the line.
    #[error("the handoff peer closed the socket")]
    PeerGone,
    /// The line ran past its cap without a newline.
    #[error("a handoff line ran past its {cap}-byte cap")]
    TooLong {
        /// The cap that was in force.
        cap: usize,
    },
    /// The line arrived whole and is not a message this protocol has.
    #[error("a handoff line is not a protocol message: {0}")]
    Malformed(#[from] serde_json::Error),
    /// The socket itself failed.
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// Write one line, newline-terminated, and flush it.
pub fn write_line(socket: &mut UnixStream, line: &Line) -> Result<(), WireError> {
    let mut bytes = serde_json::to_vec(line)?;
    bytes.push(b'\n');
    socket.write_all(&bytes)?;
    socket.flush()?;
    Ok(())
}

/// Read one line, consuming its newline and nothing after it.
///
/// `cap` bounds the line's bytes; a peer that never sends a newline is refused
/// at the cap rather than being allowed to allocate.
pub fn read_line(socket: &mut UnixStream, cap: usize) -> Result<Line, WireError> {
    let mut line = Vec::new();
    let mut buf = [0u8; PEEK_CHUNK];
    loop {
        let room = cap.saturating_sub(line.len()).min(buf.len());
        if room == 0 {
            return Err(WireError::TooLong { cap });
        }
        let peeked = peek(socket, &mut buf[..room])?;
        if peeked == 0 {
            return Err(WireError::PeerGone);
        }
        // Everything up to and including the first newline, or the whole peek
        // when there is none yet. Either way the socket gives up exactly this
        // many bytes and keeps whatever a descriptor message is attached to.
        let take = match buf[..peeked].iter().position(|&byte| byte == b'\n') {
            Some(at) => at + 1,
            None => peeked,
        };
        socket.read_exact(&mut buf[..take])?;
        let done = buf[take - 1] == b'\n';
        line.extend_from_slice(&buf[..if done { take - 1 } else { take }]);
        if done {
            return Ok(serde_json::from_slice(&line)?);
        }
    }
}

/// Look at what has arrived without consuming any of it.
///
/// `MSG_PEEK` rather than `UnixStream::peek`, which is still unstable, and
/// rather than a buffered reader, which is the thing this must not be: a
/// buffer that reads ahead can swallow the byte a descriptor is attached to.
fn peek(socket: &UnixStream, buf: &mut [u8]) -> Result<usize, WireError> {
    loop {
        return match recv(socket.as_fd(), &mut *buf, RecvFlags::PEEK) {
            Ok((_, got)) => Ok(got),
            Err(Errno::INTR) => continue,
            Err(err) => Err(WireError::Io(err.into())),
        };
    }
}

/// The secret that authenticates the importer, minted per attempt.
///
/// It travels on the importer's stdin and never on its command line:
/// `/proc/*/cmdline` is world-readable on Linux, so an argv token leaks to
/// every local user, and a secret that leaks is worse than no secret (D-M3-6
/// point 1). The socket's 0600 mode is the real wall; this is the second one.
#[derive(Clone, PartialEq, Eq)]
pub struct Token(String);

impl Token {
    /// Bytes of a token line this will read before giving up.
    const READ_CAP: usize = 512;

    /// A fresh token: 122 random bits from the generator every id in the tree
    /// already uses.
    pub fn mint() -> Self {
        Self(uuid::Uuid::new_v4().simple().to_string())
    }

    /// The secret itself, for the one place that writes it to a pipe.
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// Read a token from the importer's stdin: one line, newline optional at
    /// end of stream.
    pub fn read_from(source: &mut impl Read) -> io::Result<Self> {
        let mut seen = Vec::new();
        let mut byte = [0u8; 1];
        loop {
            match source.read(&mut byte)? {
                0 => break,
                _ if byte[0] == b'\n' => break,
                _ => seen.push(byte[0]),
            }
            if seen.len() > Self::READ_CAP {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "the handoff token on stdin ran past its cap",
                ));
            }
        }
        while seen.last() == Some(&b'\r') {
            seen.pop();
        }
        String::from_utf8(seen).map(Self).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidData, "the handoff token is not text")
        })
    }

    /// Whether `offered` is this token, in time that does not depend on how
    /// much of it a guess got right.
    pub fn matches(&self, offered: &str) -> bool {
        let mine = self.0.as_bytes();
        let theirs = offered.as_bytes();
        let mut diff = mine.len() ^ theirs.len();
        if !theirs.is_empty() {
            for (a, b) in mine.iter().zip(theirs.iter().cycle()) {
                diff |= usize::from(a ^ b);
            }
        }
        diff == 0
    }
}

impl std::fmt::Debug for Token {
    /// Redacted: a token that reaches a log has already leaked.
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("Token(<redacted>)")
    }
}
