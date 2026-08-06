//! The amx wire protocol (04 §4).
//!
//! One transport, two channel kinds multiplexed over the session socket:
//!
//! ```text
//! frame  = [u32 len][u8 channel][payload]     len capped: 1 MiB control frames,
//! chan 0 = control: JSON-RPC 2.0              per-stream negotiated caps for
//! chan N = binary stream bound by a           binary channels
//!          control call
//! ```
//!
//! Two rules run through every type in this crate:
//!
//! - **Skew tolerance.** Hello/Welcome negotiates capabilities, not equality;
//!   the server supports the current and previous protocol version at minimum;
//!   and unknown JSON object fields are ignored by contract. No deserializer
//!   here is `deny_unknown_fields`, and a test asserts it — a strict
//!   deserializer would turn a newer peer's extra field into a hard failure,
//!   which is exactly the strandedness the compatibility window exists to
//!   prevent.
//! - **Visible bytes.** Binary stream messages carry hand-rolled little-endian
//!   codecs rather than a derived binary format, so the layout the goldens and
//!   the skew harness freeze is readable in the source.

#![forbid(unsafe_code)]

pub mod control;
pub mod error;
pub mod frame;
pub mod hello;
pub mod rpc;
pub mod stream;
pub mod version;

pub use error::{FrameError, NegotiationError};
pub use frame::{CONTROL_CHANNEL, FrameHeader, MAX_CONTROL_FRAME};
pub use hello::{ClientInfo, Feature, Hello, Resume, ServerInfo, Welcome};
pub use rpc::{Notification, Request, RequestId, Response, RpcError, RpcOutcome};
pub use stream::{FlowControl, Priority, StreamId, StreamKind};
pub use version::{PROTO_MAX, PROTO_MIN};
