//! Typed protocol errors.

use thiserror::Error;

use crate::frame::MAX_CONTROL_FRAME;
use crate::stream::StreamId;

/// A frame could not be read or was not allowed.
///
/// Length violations are separate variants from decode failures because they
/// are detected at different times: an oversized length is rejected from the
/// 5-byte header, before the payload is read or a buffer for it is allocated.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum FrameError {
    /// A control frame declared more than [`MAX_CONTROL_FRAME`] bytes.
    #[error("control frame declares {len} bytes, over the {MAX_CONTROL_FRAME} byte cap")]
    ControlFrameTooLarge {
        /// The declared payload length.
        len: usize,
    },
    /// A stream frame declared more than the cap negotiated for that stream.
    #[error("stream {stream:?} frame declares {len} bytes, over its negotiated cap of {cap}")]
    StreamFrameTooLarge {
        /// The stream the frame was for.
        stream: StreamId,
        /// The declared payload length.
        len: usize,
        /// The cap negotiated for that stream.
        cap: u32,
    },
    /// The header was shorter than the fixed 5 bytes.
    #[error("frame header truncated: {len} of {expected} bytes")]
    TruncatedHeader {
        /// Bytes available.
        len: usize,
        /// Bytes required.
        expected: usize,
    },
    /// The payload was shorter than the header declared.
    #[error("frame payload truncated: {len} of {expected} bytes")]
    TruncatedPayload {
        /// Bytes available.
        len: usize,
        /// Bytes the header declared.
        expected: usize,
    },
    /// A frame arrived on a channel that is not bound to a stream.
    #[error("frame on unbound channel {channel}")]
    UnboundChannel {
        /// The channel byte.
        channel: u8,
    },
    /// The payload was not the shape its channel requires.
    #[error("malformed payload: {reason}")]
    MalformedPayload {
        /// What was wrong.
        reason: &'static str,
    },
}

/// Hello/Welcome did not produce a usable session.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum NegotiationError {
    /// The client's version window and the server's do not overlap.
    ///
    /// The only unrecoverable negotiation outcome: features intersect to the
    /// empty set happily, but there is no version both sides can speak.
    #[error(
        "no common protocol version: peer offers {peer_min}..={peer_max}, we speak {our_min}..={our_max}"
    )]
    NoCommonVersion {
        /// Lowest version the peer offers.
        peer_min: u16,
        /// Highest version the peer offers.
        peer_max: u16,
        /// Lowest version we speak.
        our_min: u16,
        /// Highest version we speak.
        our_max: u16,
    },
    /// The peer sent a version window whose bounds are inverted.
    #[error("malformed version window {min}..={max}")]
    MalformedWindow {
        /// Claimed minimum.
        min: u16,
        /// Claimed maximum.
        max: u16,
    },
    /// The first frame on the connection was not a `Hello`.
    #[error("expected hello, got {found}")]
    ExpectedHello {
        /// What arrived instead.
        found: String,
    },
}
