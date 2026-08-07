//! The staged commit, as two typed state machines.
//!
//! **W05 fills this.** `docs/09-m3-plan.md` §3 has the stages, the ownership at
//! each step, and the crash table; D-M3-6 has the five places amx departs from
//! herdr's protocol and why. The shape to build to: exporter and importer are
//! state machines whose transitions are the only public surface, so a caller
//! cannot send descriptors before validation because no method exists in that
//! state.
//!
//! Stage timeouts are herdr's — 30 s per stage, 500 ms for the advisory `owned`
//! ack, 5 s for the socket-free probe loop — and the abort rule is herdr's kept
//! strict: **no partial import ever serves.** The importer's only two endings
//! are "owned everything" and "exited having touched nothing but descriptors
//! that die with it".
//!
//! The token rides the importer's stdin, never argv: `/proc/*/cmdline` is
//! world-readable on Linux, and a secret that leaks is worse than no secret
//! (D-M3-6 point 1).
