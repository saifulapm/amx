//! A second vendor, so that the first one cannot quietly become the shape of
//! everything.
//!
//! Test builds only, and never in the table: it is nobody's agent, nothing can
//! spawn it, and it answers to no command anybody would type. Its whole job is
//! to be unlike claude in every way the descriptor allows, so that a test
//! which passes for both is a test of the machinery rather than of claude.
//!
//! It declares a dial claude does not spell the same way, leaves out one
//! claude has, and its values are words claude has never heard of. When a
//! later field arrives on the descriptor, the way to keep it honest is to
//! answer it differently here.

use super::{Capability, DEFAULT, DialSpec, Vendor};

/// The fixture. Read the module docs before changing a value: each of these
/// disagrees with claude on purpose.
pub const SECOND: Vendor = Vendor {
    name: "second",
    // A short flag, a closed set, and values that are nobody else's words.
    model: Some(DialSpec {
        cycle: &[DEFAULT, "small", "large"],
        open: false,
        flag: "-m",
    }),
    // No permission dial at all, which is the difference between a dial
    // nobody has turned and a dial that does not exist.
    permission: None,
    // Open where claude's is closed, and under a flag of its own.
    effort: Some(DialSpec {
        cycle: &[DEFAULT, "quick", "thorough"],
        open: true,
        flag: "--care",
    }),
    // A session variable spelled nothing like the other one, so that a test
    // reading it is reading the descriptor.
    session_env: Some("SECOND_SESSION"),
    not_inherited: &["SECOND_SESSION", "SECOND_PARENT"],
    // Two of the six, so that half the questions a verb asks come back the
    // other way. It carries a session on and can be taken over, and it has no
    // hooks, no transcript, no way to branch and no trust screen: the shape of
    // a vendor amx has to refuse things for.
    capabilities: &[Capability::Resume, Capability::Adopt],
    // Nothing to wire and nothing to read: this vendor tells amx nothing about
    // what it is doing, which is the shape install has to leave alone and the
    // reason the capability above is asked before either is touched.
    hooks: None,
};
