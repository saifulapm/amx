//! Layouts: a session's shape as a file, and back (D-M3-11).
//!
//! Two verbs and no server code. `amx layout export` asks for `session.state`
//! and renders the shape it already describes; `amx apply <file>` reads that
//! file and replays it through `workspace.create`, `pane.split`, `pane.resize`,
//! `pane.rename` and `agent.start`. There is no layout method on the wire and
//! no layout state on the server, which is what lets a layout be exported from
//! — or applied to — a remote session over the SSH bridge with no change at
//! all.
//!
//! [`schema`] is the file: its types, its reader and its writer. [`build`] is
//! the pair of translations between a session and that file — [`build::project`]
//! one way, [`build::plan`] the other. Both are pure, so the round trip is
//! testable without a running server, and the two verbs in
//! [`cmd`](crate::cmd) are the only parts that talk to one.

pub mod build;
pub mod schema;
