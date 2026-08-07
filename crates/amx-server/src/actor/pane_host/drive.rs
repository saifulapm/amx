//! Driven input: 04 §8's `send-text`, `send-keys` and `run`, executed on the
//! parser thread.
//!
//! > The full pane-driving surface is API + CLI: `pane send-text`,
//! > `pane send-keys` (key-combo grammar: `ctrl+h`, `f1`, …), `pane run`
//! > (bracketed-paste-aware queue-order-atomic text+submit), `pane read` …
//! > the primitives for driving ordinary terminals and tests (04 §8).
//!
//! Three of the four are writes, and all three run here rather than on the
//! connection that asked, for one reason each that turns out to be the same
//! reason: the pane's *terminal* is the authority on what the bytes should be
//! and when they may be queued, and the terminal belongs to the parser thread
//! and to nothing else (04 §3).
//!
//! - `send_keys` encodes against the pane's live DECCKM, keypad and
//!   Kitty-keyboard state, which [`KeyEncoder::sync_from`] reads off the
//!   terminal in one call. An encoder that guessed would send `\x1b[A` to an
//!   application that asked for `\x1bOA`.
//! - `run` brackets its text **only** when the application enabled bracketed
//!   paste, because sending the markers to one that did not types `[200~` into
//!   it — the failure the verb exists to avoid.
//! - `send_text` needs neither, and still comes through here: queueing every
//!   driven write from this thread is what keeps it ordered against the query
//!   replies a parse produces (04 §3's retained response-order guarantee).
//!   A parse already under way finishes, and its replies reach the pty's write
//!   queue, before the drive command behind it is even taken off the queue.
//!
//! The write itself never blocks. The pty thread's read callback blocks on this
//! thread while a parse runs, so a drive that waited for room in a full input
//! queue would be waiting on the thread that is waiting on it. Full is
//! therefore reported ([`DriveError::Full`]), never waited out.

use amx_vt::{KeyAction, KeyEncoder, KeyEvent, Mode, Terminal};
use bytes::Bytes;
use thiserror::Error;

use super::keys::KeyStroke;
use crate::pty::{PtyActorError, PtyActorHandle};

/// What bracketed paste wraps text in: `ESC [ 200 ~` … `ESC [ 201 ~`.
const PASTE_START: &[u8] = b"\x1b[200~";
/// The end marker.
pub const PASTE_END: &[u8] = b"\x1b[201~";

/// What `pane.run` submits with: the byte `enter` encodes to.
const SUBMIT: &[u8] = b"\r";

/// The chunks `pane.run` queues, in order: the paste, then the submit.
///
/// **The submit is a chunk of its own on purpose — never appended to the
/// paste.** A paste-aware TUI that finds a `CR` in the same read as the paste
/// terminator can take it for trailing whitespace *of the paste* rather than
/// for a keypress after it, and swallow the submit: the text lands in the
/// input box and nothing is sent. Measured against real Claude Code at about
/// 3% of turn-starting prompts, against 0 for a two-write path — the trials
/// are in `docs/notes/m2-live-smoke.md` §8.2.
///
/// Splitting costs nothing that 04 §8's "atomic text+submit" needs. The pane's
/// input queue is ordered and one chunk is one `write()`, so what the verb has
/// to keep is that nothing lands *between* the two chunks — and that is what
/// [`PtyActorHandle::try_write_input_pair`] is: both sends under the queue's
/// ordering lock, which is stronger than this call site could arrange for
/// itself, since a pane's queue also carries the keystrokes a connection
/// forwards. Atomicity here is queue-order atomicity, which is the property,
/// not single-`write()` atomicity, which was only ever how it was spelled.
pub fn run_chunks(text: &str, bracketed: bool) -> [Bytes; 2] {
    let mut paste = Vec::with_capacity(
        text.len()
            + if bracketed {
                PASTE_START.len() + PASTE_END.len()
            } else {
                0
            },
    );
    if bracketed {
        paste.extend_from_slice(PASTE_START);
    }
    paste.extend_from_slice(text.as_bytes());
    if bracketed {
        paste.extend_from_slice(PASTE_END);
    }
    [Bytes::from(paste), Bytes::from_static(SUBMIT)]
}

/// What a driving verb asks the pane to put in front of its child.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Drive {
    /// `pane.send_text`: these bytes, verbatim, with nothing added.
    Text(String),
    /// `pane.send_keys`: these combos, encoded against the pane's own modes.
    Keys(Vec<KeyStroke>),
    /// `pane.run`: this text, bracketed if the application asked for it, then
    /// submitted — two chunks queued back to back, so nothing can land between
    /// the text and its newline. See [`run_chunks`] for why the newline is a
    /// chunk of its own rather than the tail of one write.
    Run(String),
}

/// What one drive did.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Driven {
    /// How many bytes reached the pty's input queue.
    pub bytes: usize,
    /// Whether the text was bracketed — a fact about the application in the
    /// pane, which is worth reporting rather than leaving the caller to infer.
    pub bracketed: bool,
}

/// Why a drive did not reach the child.
#[derive(Clone, PartialEq, Eq, Debug, Error)]
pub enum DriveError {
    /// The pane is stopping, or already stopped.
    #[error("the pane is gone")]
    Gone,
    /// The key encoder could not be built or could not encode a combo.
    #[error("key encoding failed: {0}")]
    Encode(String),
    /// The pane's input queue is full: the child is not reading.
    #[error("the pane's input queue is full")]
    Full,
    /// The pane's terminal is not taking input: it has been quiesced or
    /// released. Distinct from [`Gone`](Self::Gone) because it is a state a
    /// pane comes back from, and a caller told "gone" would stop retrying.
    #[error("the pane is not accepting input")]
    NotAccepting,
}

impl From<PtyActorError> for DriveError {
    fn from(err: PtyActorError) -> Self {
        match err {
            PtyActorError::QueueFull => Self::Full,
            PtyActorError::NotAccepting | PtyActorError::Released => Self::NotAccepting,
            _ => Self::Gone,
        }
    }
}

/// The parser thread's driving state: an encoder and the buffer it fills.
///
/// Both are kept across calls rather than built per call. The encoder is only
/// built when a pane is first driven — most panes never are — and is re-synced
/// from the terminal on every use, because the application may have changed a
/// mode since the last one.
#[derive(Debug, Default)]
pub(super) struct Driver {
    encoder: Option<KeyEncoder>,
    buffer: Vec<u8>,
}

impl Driver {
    /// Turn one [`Drive`] into bytes and queue them for the child.
    ///
    /// `terminal` is read, never written: what it contributes is the mode state
    /// the encoding and the bracketing depend on.
    pub(super) fn perform(
        &mut self,
        what: Drive,
        terminal: &Terminal,
        pty: &PtyActorHandle,
    ) -> Result<Driven, DriveError> {
        self.buffer.clear();
        let mut bracketed = false;
        match what {
            Drive::Text(text) => self.buffer.extend_from_slice(text.as_bytes()),
            Drive::Keys(strokes) => self.encode(&strokes, terminal)?,
            Drive::Run(text) => {
                // `mode` reads the terminal's own record of what the
                // application asked for; a failure to read it is treated as
                // "not enabled", which is the safe half of the choice — an
                // unbracketed paste is ordinary typing, a bracketed one sent to
                // an application that never asked is visible garbage.
                bracketed = terminal.mode(Mode::BRACKETED_PASTE).unwrap_or(false);
                let [paste, submit] = run_chunks(&text, bracketed);
                let bytes = paste.len() + submit.len();
                // Paired, not concatenated, and not two loose calls: the pair
                // is what makes the submit its own `write()` while keeping it
                // unseparable from the text. `run_chunks` has the why.
                pty.try_write_input_pair(paste, submit)?;
                return Ok(Driven { bytes, bracketed });
            }
        }
        let bytes = self.buffer.len();
        // Never `write_input`, which waits for queue room: see the module note
        // on the thread that would be waiting on us.
        pty.try_write_input(Bytes::copy_from_slice(&self.buffer))?;
        Ok(Driven { bytes, bracketed })
    }

    /// Encode every combo into the buffer, against this terminal's modes.
    fn encode(&mut self, strokes: &[KeyStroke], terminal: &Terminal) -> Result<(), DriveError> {
        if self.encoder.is_none() {
            self.encoder =
                Some(KeyEncoder::new().map_err(|err| DriveError::Encode(err.to_string()))?);
        }
        let Some(encoder) = self.encoder.as_mut() else {
            // Unreachable: the branch above just filled it or returned.
            return Err(DriveError::Encode("no encoder".to_owned()));
        };
        encoder.sync_from(terminal);
        for stroke in strokes {
            let mut event = KeyEvent::new().map_err(|err| DriveError::Encode(err.to_string()))?;
            event.set_action(KeyAction::Press);
            event.set_key(stroke.key);
            event.set_mods(stroke.mods);
            if let Some(text) = &stroke.text {
                event.set_text(text.as_bytes());
                if let Some(first) = text.chars().next() {
                    event.set_unshifted_codepoint(first as u32);
                }
            }
            // Zero bytes is a normal answer — a bare modifier encodes to
            // nothing — so the count is not checked, only the failure.
            encoder
                .encode(&event, &mut self.buffer)
                .map_err(|err| DriveError::Encode(err.to_string()))?;
        }
        Ok(())
    }
}
