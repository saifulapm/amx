//! Safe wrapper over libghostty-vt.
//!
//! The terminal core is an FFI object with paged scrollback, modes and dirty
//! flags. 04 §3 states the ownership rule this crate exists to enforce: "the
//! parser I/O thread **exclusively owns the libghostty-vt instance**; VT state
//! is never shared or swapped … At frame boundaries the parser copies damaged
//! visible rows + cursor into a derived, double-buffered POD cell snapshot
//! published lock-free for render."
//!
//! The FFI surface, the vendored library and the terminal handle land with the
//! vendoring and wrapper work; what is fixed here is the snapshot handle, which
//! the server's pane actor names in its command enum.

pub mod snapshot;
pub mod sys;

pub use snapshot::{Snapshot, SnapshotRef};
