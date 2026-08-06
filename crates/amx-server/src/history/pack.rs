//! How one history row is packed, and how it is hashed.
//!
//! Both live here because they must agree: the hash a row is *announced* with
//! (04 §3) has to cover exactly the bytes the row is *served* as, or a client
//! cannot use one to check the other.
//!
//! The packing is self-delimiting — a length prefix, the text, one flag byte —
//! which is what makes a chunked transfer reassemble byte-identically no matter
//! where the chunk boundaries fall. It is little-endian and hand-rolled like
//! the rest of the wire encodings, so the goldens stay readable.
//!
//! **Provisional:** rows carry their characters and their soft-wrap flag, not
//! their styling; scrollback renders unstyled in the client. The row layout
//! itself is `amx_proto::stream::history`'s — this module only reads a row out
//! of the terminal and hands it to that one writer, so the bytes served and
//! the bytes a client unpacks can never disagree.

use amx_core::RowHash;
use amx_vt::{Point, Terminal};

/// The FNV-1a 64-bit offset basis.
const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
/// The FNV-1a 64-bit prime.
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// One row's packed bytes.
///
/// Appended to a caller-owned buffer rather than returned, because a history
/// transfer packs thousands of rows into one reused `Vec`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RowPack;

impl RowPack {
    /// Append the row at `offset` in the pane's history to `out`.
    ///
    /// The caller is responsible for `offset` being inside the scrollback:
    /// libghostty-vt's history space runs on into the live rows and will
    /// happily return one (measured — see the spike note), so an unbounded
    /// offset serves the live grid as history.
    ///
    /// # Errors
    ///
    /// Propagates a library failure, including the [`amx_vt::Error::InvalidValue`]
    /// a point past the whole screen produces.
    pub fn append(
        terminal: &Terminal,
        offset: u32,
        scratch: &mut String,
        out: &mut Vec<u8>,
    ) -> amx_vt::Result<()> {
        scratch.clear();
        let read = terminal.read_row(Point::history(offset), scratch)?;
        amx_proto::stream::history::put_row(scratch.as_bytes(), read.wrapped, out);
        Ok(())
    }
}

/// The content hash of one packed row.
///
/// FNV-1a rather than the standard library's hasher: this value crosses a
/// process boundary — a client compares the hash of a row it cached against the
/// hash the server announces after a restart or a handoff — and `RandomState`
/// would make the same row hash differently in every process.
#[must_use]
pub fn hash_bytes(bytes: &[u8]) -> RowHash {
    let mut hash = FNV_OFFSET;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    RowHash::from_raw(hash)
}

/// Pack the row at `offset` and hash it, leaving the packed bytes in `out`.
///
/// # Errors
///
/// Propagates a library failure.
pub fn hash_row(
    terminal: &Terminal,
    offset: u32,
    scratch: &mut String,
    out: &mut Vec<u8>,
) -> amx_vt::Result<RowHash> {
    out.clear();
    RowPack::append(terminal, offset, scratch, out)?;
    Ok(hash_bytes(out))
}
