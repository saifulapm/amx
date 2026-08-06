//! Framing: `[u32 len][u8 channel][payload]`.

use crate::error::FrameError;

/// The control channel. Channel 0 is always JSON-RPC 2.0 (04 §4).
pub const CONTROL_CHANNEL: u8 = 0;

/// Hard cap on a control frame payload: 1 MiB.
///
/// 04 §4 states the cap as a property of the protocol, not of an
/// implementation. It is enforced from the header — see
/// [`FrameHeader::decode`] — so an oversized declaration costs a rejection, not
/// an allocation.
pub const MAX_CONTROL_FRAME: usize = 1 << 20;

/// Default cap for a binary stream frame, before per-stream negotiation.
///
/// Streams negotiate their own cap with
/// [`FlowControl::StreamCap`](crate::stream::FlowControl::StreamCap); this is
/// what applies until they do.
pub const DEFAULT_STREAM_FRAME: u32 = 1 << 20;

/// Bytes in a frame header: `u32` length plus a channel byte.
pub const FRAME_HEADER_LEN: usize = 5;

/// The fixed-size prefix of every frame.
///
/// `len` counts payload bytes only, not the header. Both fields are
/// little-endian on the wire: the binary stream codecs are hand-rolled so their
/// layout stays visible in the goldens, and the header uses the same
/// convention as the payloads it introduces.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct FrameHeader {
    /// Payload length in bytes.
    pub len: u32,
    /// Channel: 0 for control, N for a bound binary stream.
    pub channel: u8,
}

impl FrameHeader {
    /// A header for `len` payload bytes on `channel`.
    #[must_use]
    pub const fn new(len: u32, channel: u8) -> Self {
        Self { len, channel }
    }

    /// Whether this frame is on the control channel.
    #[must_use]
    pub const fn is_control(&self) -> bool {
        self.channel == CONTROL_CHANNEL
    }

    /// The declared payload length as a `usize`.
    #[must_use]
    pub const fn payload_len(&self) -> usize {
        self.len as usize
    }

    /// Serialize the header.
    #[must_use]
    pub const fn encode(&self) -> [u8; FRAME_HEADER_LEN] {
        let len = self.len.to_le_bytes();
        [len[0], len[1], len[2], len[3], self.channel]
    }

    /// Parse a header and enforce the control-frame cap.
    ///
    /// A control frame over [`MAX_CONTROL_FRAME`] is rejected here, from the
    /// header alone — before the payload is read and before a buffer is sized
    /// from an attacker-controlled length. Stream frames are checked separately
    /// by [`check_stream_len`](Self::check_stream_len), because their cap is
    /// per-stream and known only after the stream is bound.
    pub fn decode(bytes: [u8; FRAME_HEADER_LEN]) -> Result<Self, FrameError> {
        let header = Self {
            len: u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
            channel: bytes[4],
        };
        if header.is_control() && header.payload_len() > MAX_CONTROL_FRAME {
            return Err(FrameError::ControlFrameTooLarge {
                len: header.payload_len(),
            });
        }
        Ok(header)
    }

    /// Parse a header from a slice that may be short.
    pub fn decode_slice(bytes: &[u8]) -> Result<Self, FrameError> {
        let fixed: [u8; FRAME_HEADER_LEN] = bytes
            .get(..FRAME_HEADER_LEN)
            .ok_or(FrameError::TruncatedHeader {
                len: bytes.len(),
                expected: FRAME_HEADER_LEN,
            })?
            .try_into()
            .map_err(|_| FrameError::TruncatedHeader {
                len: bytes.len(),
                expected: FRAME_HEADER_LEN,
            })?;
        Self::decode(fixed)
    }

    /// Check this header against the cap negotiated for its stream.
    pub fn check_stream_len(&self, cap: u32) -> Result<(), FrameError> {
        if self.len > cap {
            return Err(FrameError::StreamFrameTooLarge {
                stream: crate::stream::StreamId::new(u16::from(self.channel)),
                len: self.payload_len(),
                cap,
            });
        }
        Ok(())
    }
}
