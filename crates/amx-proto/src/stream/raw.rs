//! The raw pane I/O stream: observe, or control.
//!
//! ```text
//! message := [u8;16] pane uuid, u8 direction (0 to-pane, 1 from-pane), bytes…
//! ```
//!
//! The pane rides in every message even though the stream is bound to one pane
//! already: a frame that names its pane is checkable at the receiving end, so a
//! stale frame for a stream that has since unbound cannot land on the wrong
//! process.

use amx_core::PaneId;
use serde::{Deserialize, Serialize};

use super::codec::{CodecError, Reader};

/// Bytes the fixed prefix (pane id and direction) occupies.
pub const RAW_PREFIX: usize = 17;

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
        out.extend_from_slice(&self.pane.into_bytes());
        out.push(match self.direction {
            RawDirection::ToPane => 0,
            RawDirection::FromPane => 1,
        });
        out.extend_from_slice(self.bytes);
    }
}

impl<'a> RawPaneIo<'a> {
    /// Decode a message that borrows from `bytes`.
    ///
    /// # Errors
    ///
    /// [`CodecError::Truncated`] if the payload ends inside the prefix, and
    /// [`CodecError::UnknownDirection`] for a direction byte this build does
    /// not know.
    pub fn decode(bytes: &'a [u8]) -> Result<Self, CodecError> {
        let mut reader = Reader::new(bytes);
        let uuid = reader.take(16)?;
        // Exactly 16 bytes were just taken.
        let mut fixed = [0_u8; 16];
        fixed.copy_from_slice(uuid);
        let pane = PaneId::from_bytes(fixed);
        let direction = match reader.u8()? {
            0 => RawDirection::ToPane,
            1 => RawDirection::FromPane,
            byte => return Err(CodecError::UnknownDirection { byte }),
        };
        let rest = reader.take(reader.left())?;
        Ok(Self {
            pane,
            direction,
            bytes: rest,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_raw_run_round_trips_byte_identical() {
        let message = RawPaneIo {
            pane: PaneId::new_v4(),
            direction: RawDirection::ToPane,
            bytes: b"ls -la\r\x1b[A",
        };
        let mut bytes = Vec::new();
        message.encode(&mut bytes);
        assert_eq!(bytes.len(), RAW_PREFIX + message.bytes.len());
        let decoded = RawPaneIo::decode(&bytes).expect("the run decodes");
        assert_eq!(decoded, message);
    }

    #[test]
    fn an_unknown_direction_is_refused() {
        let mut bytes = vec![0_u8; 16];
        bytes.push(9);
        assert_eq!(
            RawPaneIo::decode(&bytes),
            Err(CodecError::UnknownDirection { byte: 9 })
        );
    }

    #[test]
    fn a_truncated_prefix_is_refused() {
        assert!(matches!(
            RawPaneIo::decode(&[0_u8; 5]),
            Err(CodecError::Truncated { .. })
        ));
    }
}
