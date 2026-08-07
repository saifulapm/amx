//! `Core`'s half of persistence: capture, restore, and the loss report.
//!
//! The capture split of `docs/07-m1-plan.md` §2: capture happens here because
//! `Core` owns the state, writing happens on the `Persist` actor, and fsync
//! happens on the blocking pool. `Persist` asks through the ordinary
//! [`CoreHandle`](crate::actor::CoreHandle) mailbox — no back door — so a
//! capture that arrives while `Core` is folding a batch simply queues, exactly
//! like every other command.
//!
//! # Task ownership
//!
//! U01 planted the routing arm and this stub; **U06** implements it
//! (`docs/07-m1-plan.md` §4), and adds `Core::capture_cheap` for the shutdown
//! push, `Core::restore` with D-M1-9's prune-and-report table, and the
//! [`RestoreReport`](amx_proto::control::session::RestoreReport) that
//! `session.report` serves and `session.state` summarizes. The arm lives in
//! `core/mod.rs` and this file's signature is what it calls, so U06 never
//! reopens the routing table.

use tokio::sync::oneshot;

use super::Core;
use crate::actor::Capture;
use crate::persist::Snapshot;

impl Core {
    /// Assemble a snapshot of the session for the `Persist` actor.
    ///
    /// `sidecars` asks for a clone of each live pane's command handle as well,
    /// so a scrollback dump can read history through the parser thread; it is
    /// false whenever `[persist] history` is off, and then the dump path costs
    /// nothing.
    ///
    /// **Stub — U06 fills this.** Until then a capture reports an empty
    /// session, which is honest for a build where nothing writes snapshots
    /// yet, and wrong the moment one does: U05 and U06 land in the same wave.
    pub(super) fn handle_capture(&mut self, sidecars: bool, reply: oneshot::Sender<Capture>) {
        let _ = sidecars;
        let _ = reply.send(Capture::without_sidecars(Snapshot::empty()));
    }
}
