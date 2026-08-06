//! Protocol versions and the compatibility window.

use crate::error::NegotiationError;

/// Oldest protocol version this build speaks.
pub const PROTO_MIN: u16 = 1;

/// Newest protocol version this build speaks.
pub const PROTO_MAX: u16 = 1;

/// How many versions back the server must keep speaking.
///
/// 04 §4: "the server supports current and previous protocol version, minimum.
/// A release never strands every remote host at once." A window of 2 is
/// current-and-previous; while only version 1 exists the window is degenerate,
/// and the skew harness runs current-against-current until a second version
/// lands.
pub const COMPAT_WINDOW: u16 = 2;

/// The inclusive version window this build offers in `Hello`/`Welcome`.
#[must_use]
pub const fn window() -> (u16, u16) {
    (PROTO_MIN, PROTO_MAX)
}

/// Whether this build speaks `version`.
#[must_use]
pub const fn supports(version: u16) -> bool {
    version >= PROTO_MIN && version <= PROTO_MAX
}

/// Pick the highest version both sides speak.
///
/// 04 §4: the server "picks the highest common version" — highest, not equal
/// and not the client's preference. Fails only when the windows do not overlap
/// at all, which is the one negotiation outcome that cannot be degraded into a
/// working session.
pub fn negotiate(peer: (u16, u16), ours: (u16, u16)) -> Result<u16, NegotiationError> {
    let _ = (peer, ours);
    todo!("intersect the windows and take the maximum")
}
