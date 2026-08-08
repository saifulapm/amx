//! The connection → `Core` vocabulary: one variant per dispatch group.
//!
//! Split out of [`super`] beside [`super::panes`] by W03 (`docs/09-m3-plan.md`
//! R-M3-7). A connection decodes a call, hands the `Core` the typed parameters
//! and a reply channel, and never touches session state itself — so this module
//! mirrors the method table's domains and grows when a *method* does.

use amx_core::PaneId;
use amx_proto::control::{client, pane, session, workspace};
use tokio::sync::oneshot;

use super::pane_host::SnapshotFeed;
use super::panes::PaneReport;
use super::persist::Capture;
use super::{AgentCall, PaneHandle, Reply};

/// What a connection task asks of the `Core`.
///
/// One variant per dispatch group, mirroring the method table's domains: the
/// connection decodes a call, hands the `Core` the typed parameters and a reply
/// channel, and never touches session state itself.
#[derive(Debug)]
pub enum CoreCommand {
    /// A `session.*` call.
    Session(SessionCall),
    /// A `workspace.*` call.
    Workspace(WorkspaceCall),
    /// A `pane.*` call.
    Pane(PaneCall),
    /// A `client.*` call.
    Client(ClientCall),
    /// A `stream.*` call's `Core` half.
    Stream(StreamCall),
    /// A pane actor reported a transition.
    PaneReport {
        /// Which pane.
        pane: PaneId,
        /// What happened.
        report: PaneReport,
    },
    /// Something `AgentHub` decided; see [`AgentCall`].
    Agent(AgentCall),
    /// Shut the session down.
    Shutdown,
}

/// A live pane's plumbing, handed to the connection that binds a stream on it.
///
/// The connection talks to the pane directly from then on — input bytes go to
/// [`PaneWiring::handle`], grid frames come off [`PaneWiring::frames`] —
/// so a keystroke never queues behind the `Core`'s mailbox (04 §4's round-trip
/// budget is the reason this is a hand-off rather than a relay).
#[derive(Clone, Debug)]
pub struct PaneWiring {
    /// The pane's command mailbox.
    pub handle: PaneHandle,
    /// The pane's published frames.
    pub frames: SnapshotFeed,
}

/// The `Core` half of stream binding: resolving a pane to its live plumbing.
#[derive(Debug)]
pub enum StreamCall {
    /// Fetch the wiring of a live pane.
    Wiring {
        /// The pane a stream is being bound for.
        pane: PaneId,
        /// Where the wiring goes.
        reply: Reply<PaneWiring>,
    },
}

/// `session.*` calls.
#[derive(Debug)]
pub enum SessionCall {
    /// `ping`.
    Ping {
        /// Parameters.
        params: session::PingParams,
        /// Where the reply goes.
        reply: Reply<session::PingReply>,
    },
    /// An attached client completed its handshake.
    ///
    /// Not a wire method: the gateway's connection task sends this on behalf
    /// of a client whose hello declared an attach, and awaits the reply
    /// before writing the welcome. The `Core` seeds a session with no
    /// workspaces with its first one — a live shell to land in — so a bare
    /// `amx` never renders an empty session. Seeding is idempotent by
    /// construction: the `Core` serializes its mailbox, so the second of two
    /// racing first attaches sees the first one's workspace and does nothing.
    Attached {
        /// Where the acknowledgement goes, once any seeding is done.
        reply: Reply<()>,
    },
    /// `session.state`.
    State {
        /// Parameters.
        params: session::StateParams,
        /// Where the snapshot goes.
        reply: Reply<session::StateReply>,
    },
    /// `session.handoff`, with the pre-flight already run.
    ///
    /// The verdict on `<binary> _handoff-caps` travels *with* the call rather
    /// than being fetched inside it, and that ordering is the whole of
    /// "refused before any pane is touched": the probe runs on the connection
    /// task, and a `Core` that refuses this has quiesced nothing (D-M3-6
    /// point 2). It is also the reason the probe cannot stall the session —
    /// exec'ing a wrong binary on `Core`'s own loop would hold every other
    /// verb behind it.
    Handoff {
        /// Parameters.
        params: session::HandoffParams,
        /// What the staged binary said it can be handed, or why it cannot be.
        preflight: Result<crate::handoff::export::Caps, String>,
        /// Where the accepted-or-refused answer goes. Acceptance is not
        /// completion: the caller's own connection dies when the gateway
        /// retires, and the outcome is read back from `session.report`.
        reply: Reply<session::HandoffReply>,
    },
    /// `session.report`.
    ///
    /// Answered from the [`RestoreReport`](amx_proto::control::session::RestoreReport)
    /// the startup restore left on `Core`, which is where it lives for the
    /// server's lifetime: 04 §6 requires restore loss to stay queryable, so it
    /// is state, not a log line that has already scrolled away.
    Report {
        /// Parameters.
        params: session::ReportParams,
        /// Where the report goes.
        reply: Reply<session::ReportReply>,
    },
    /// Assemble a snapshot of the session for persistence.
    ///
    /// Not a wire method: the `Persist` actor sends this through the ordinary
    /// [`CoreHandle`](super::CoreHandle) when its debounce fires — no back
    /// door, the same rule the connection path follows — and `Core` answers
    /// synchronously from [`SessionState`](amx_core::SessionState) plus its
    /// shorts maps. A capture can never fail, so the reply channel carries the
    /// value bare.
    Capture {
        /// Whether the caller wants pane handles for sidecar dumps too.
        sidecars: bool,
        /// Where the capture goes.
        reply: oneshot::Sender<Capture>,
    },
}

/// `workspace.*` calls.
#[derive(Debug)]
pub enum WorkspaceCall {
    /// `workspace.create`.
    Create {
        /// Parameters.
        params: workspace::CreateParams,
        /// Where the reply goes.
        reply: Reply<workspace::CreateReply>,
    },
    /// `workspace.rename`.
    Rename {
        /// Parameters.
        params: workspace::RenameParams,
        /// Where the reply goes.
        reply: Reply<workspace::RenameReply>,
    },
    /// `workspace.kill`.
    Kill {
        /// Parameters.
        params: workspace::KillParams,
        /// Where the reply goes.
        reply: Reply<workspace::KillReply>,
    },
    /// `workspace.switch`.
    Switch {
        /// Parameters.
        params: workspace::SwitchParams,
        /// Where the reply goes.
        reply: Reply<workspace::SwitchReply>,
    },
}

/// `pane.*` calls.
#[derive(Debug)]
pub enum PaneCall {
    /// `pane.split`.
    Split {
        /// Parameters.
        params: pane::SplitParams,
        /// Where the reply goes.
        reply: Reply<pane::SplitReply>,
    },
    /// `pane.zoom`.
    Zoom {
        /// Parameters.
        params: pane::ZoomParams,
        /// Where the reply goes.
        reply: Reply<pane::ZoomReply>,
    },
    /// `pane.swap`.
    Swap {
        /// Parameters.
        params: pane::SwapParams,
        /// Where the reply goes.
        reply: Reply<pane::SwapReply>,
    },
    /// `pane.move`.
    Move {
        /// Parameters.
        params: pane::MoveParams,
        /// Where the reply goes.
        reply: Reply<pane::MoveReply>,
    },
    /// `pane.rename`.
    Rename {
        /// Parameters.
        params: pane::RenameParams,
        /// Where the reply goes.
        reply: Reply<pane::RenameReply>,
    },
    /// `pane.close`.
    Close {
        /// Parameters.
        params: pane::CloseParams,
        /// Where the reply goes.
        reply: Reply<pane::CloseReply>,
    },
    /// `pane.focus`.
    Focus {
        /// Parameters.
        params: pane::FocusParams,
        /// Where the reply goes.
        reply: Reply<pane::FocusReply>,
    },
    /// `pane.resize`.
    Resize {
        /// Parameters.
        params: pane::ResizeParams,
        /// Where the reply goes.
        reply: Reply<pane::ResizeReply>,
    },
}

/// `client.*` calls.
#[derive(Debug)]
pub enum ClientCall {
    /// The client declared its terminal size and visible panes.
    Viewport {
        /// Parameters.
        params: client::Viewport,
        /// Where the acknowledgement goes.
        reply: Reply<client::ViewportReply>,
    },
}
