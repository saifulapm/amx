//! The raw pane I/O stream: observe, or control.

use amx_core::PaneId;
use serde::{Deserialize, Serialize};

use crate::error::FrameError;

/// Which way raw bytes are flowing.
///
/// The two directions are separate because they carry different authority: a
/// tool that only observes a pane never sends [`ToPane`](Self::ToPane), and the
/// server can refuse that direction without refusing the stream.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RawDirection {
    /// Bytes written into the pane's PTY, as if typed.
    ToPane,
    /// Bytes the pane's process produced.
    FromPane,
}

/// A run of raw pane bytes.
///
/// Untranslated on purpose: this stream is the escape hatch for tools that want
/// the terminal's own byte stream rather than amx's cell model, so nothing here
/// interprets, re-encodes or reflows what passes through.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct RawPaneIo<'a> {
    /// The pane.
    pub pane: PaneId,
    /// Which direction these bytes travelled.
    pub direction: RawDirection,
    /// The bytes, verbatim.
    pub bytes: &'a [u8],
}

impl RawPaneIo<'_> {
    /// Append the little-endian encoding of this message to `out`.
    pub fn encode(&self, out: &mut Vec<u8>) {
        let _ = out;
        todo!("write the hand-rolled little-endian raw io encoding")
    }
}

impl<'a> RawPaneIo<'a> {
    /// Decode a message that borrows from `bytes`.
    pub fn decode(bytes: &'a [u8]) -> Result<Self, FrameError> {
        let _ = bytes;
        todo!("read the hand-rolled little-endian raw io encoding")
    }
}
