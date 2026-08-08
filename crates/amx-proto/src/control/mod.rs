//! The control method table.
//!
//! 04 §4: "The control schema is one annotated Rust enum from which we
//! **derive**: serde wire names, method dispatch, the JSON Schema artifact, CLI
//! clap tree and parsing, and docs (fixes W6's four hand-synced lists — adding
//! a method is one enum variant + one handler)."
//!
//! That is what `method_table!` (in [`table`]) does. One row per method
//! produces:
//!
//! - a [`Method`] variant and its wire name, in both directions;
//! - a [`Call`] variant carrying the method's parameter type;
//! - a [`Dispatch`] trait method, so a handler that is never written is a
//!   compile error rather than a method that silently 404s;
//! - a [`MethodSpec`] row in [`SPECS`], from which the CLI tree is built.
//!
//! Payload types live in the per-domain modules, so only the variant list lives
//! on the enum — which is what keeps the enum small as the surface grows.
//!
//! The JSON Schema artifact named in 04 §4 has no consumer until `amx api
//! schema`, so it is not generated here yet; [`SPECS`] is the table it will be
//! generated from.

pub mod agent;
#[cfg(feature = "cli")]
pub mod cli;
pub mod client;
pub mod pane;
pub mod session;
pub mod stream;
mod table;
pub mod wait;
pub mod workspace;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use table::MethodSpec;
use table::method_table;

use crate::rpc::RpcError;

method_table! {
    /// Liveness and clock check; the connect probe's payload.
    Ping {
        wire: "ping",
        handler: ping,
        params: session::PingParams,
        reply: session::PingReply,
        cli: ["ping"],
        about: "Check that the session server is alive",
    }

    /// Create a workspace.
    WorkspaceCreate {
        wire: "workspace.create",
        handler: workspace_create,
        params: workspace::CreateParams,
        reply: workspace::CreateReply,
        cli: ["workspace", "create"],
        about: "Create a workspace",
    }

    /// Rename a workspace.
    WorkspaceRename {
        wire: "workspace.rename",
        handler: workspace_rename,
        params: workspace::RenameParams,
        reply: workspace::RenameReply,
        cli: ["workspace", "rename"],
        about: "Rename a workspace",
    }

    /// Kill a workspace and every pane it holds.
    WorkspaceKill {
        wire: "workspace.kill",
        handler: workspace_kill,
        params: workspace::KillParams,
        reply: workspace::KillReply,
        cli: ["workspace", "kill"],
        about: "Kill a workspace and every pane it holds",
    }

    /// Focus a workspace.
    WorkspaceSwitch {
        wire: "workspace.switch",
        handler: workspace_switch,
        params: workspace::SwitchParams,
        reply: workspace::SwitchReply,
        cli: ["workspace", "switch"],
        about: "Focus a workspace",
    }

    /// Split a pane, creating a new one beside it.
    PaneSplit {
        wire: "pane.split",
        handler: pane_split,
        params: pane::SplitParams,
        reply: pane::SplitReply,
        cli: ["pane", "split"],
        about: "Split a pane and run a command in the new one",
    }

    /// Toggle a pane between its layout slot and the full workspace.
    PaneZoom {
        wire: "pane.zoom",
        handler: pane_zoom,
        params: pane::ZoomParams,
        reply: pane::ZoomReply,
        cli: ["pane", "zoom"],
        about: "Toggle a pane between its layout slot and the full workspace",
    }

    /// Exchange two panes' positions in the layout.
    PaneSwap {
        wire: "pane.swap",
        handler: pane_swap,
        params: pane::SwapParams,
        reply: pane::SwapReply,
        cli: ["pane", "swap"],
        about: "Swap two panes' positions without restarting either",
    }

    /// Move a pane into a different workspace.
    PaneMove {
        wire: "pane.move",
        handler: pane_move,
        params: pane::MoveParams,
        reply: pane::MoveReply,
        cli: ["pane", "move"],
        about: "Move a pane into a different workspace",
    }

    /// Close a pane.
    PaneClose {
        wire: "pane.close",
        handler: pane_close,
        params: pane::CloseParams,
        reply: pane::CloseReply,
        cli: ["pane", "close"],
        about: "Close a pane",
    }

    /// Move a workspace's focus to the geometric neighbour in a direction.
    PaneFocus {
        wire: "pane.focus",
        handler: pane_focus,
        params: pane::FocusParams,
        reply: pane::FocusReply,
        cli: ["pane", "focus"],
        about: "Move focus to the neighbouring pane in a direction",
    }

    /// Nudge the split holding a pane by a ratio delta.
    PaneResize {
        wire: "pane.resize",
        handler: pane_resize,
        params: pane::ResizeParams,
        reply: pane::ResizeReply,
        cli: ["pane", "resize"],
        about: "Grow or shrink a pane's slot in a direction",
    }

    /// Give a pane a user-visible label.
    PaneRename {
        wire: "pane.rename",
        handler: pane_rename,
        params: pane::RenameParams,
        reply: pane::RenameReply,
        cli: ["pane", "rename"],
        about: "Give a pane a user-visible label",
    }

    /// The full session snapshot a fresh client folds into its model.
    SessionState {
        wire: "session.state",
        handler: session_state,
        params: session::StateParams,
        reply: session::StateReply,
        cli: ["session", "state"],
        about: "Report the session's workspaces, layouts, focus and panes",
    }

    /// What the last startup restore lost or degraded, entry by entry.
    SessionReport {
        wire: "session.report",
        handler: session_report,
        params: session::ReportParams,
        reply: session::ReportReply,
        cli: ["session", "report"],
        about: "Report what the last restore lost or degraded",
    }

    /// Hand this session over to a staged binary, live (04 §6, D-M3-6).
    ///
    /// The reply says only whether the protocol *started*: the exporter retires
    /// its gateway partway through, so the connection that asked dies before
    /// the swap finishes. Completion is observed by reconnecting and reading
    /// `Welcome.session`; what went wrong is read from `session.report`.
    SessionHandoff {
        wire: "session.handoff",
        handler: session_handoff,
        params: session::HandoffParams,
        reply: session::HandoffReply,
        cli: ["session", "handoff"],
        about: "Hand this session over to a staged binary without dropping a pane",
    }

    /// Bind a binary stream to a frame channel on this connection.
    StreamBind {
        wire: "stream.bind",
        handler: stream_bind,
        params: stream::BindParams,
        reply: stream::BindReply,
        cli: ["stream", "bind"],
        about: "Bind a binary stream (grid, history, raw i/o) to a channel",
    }

    /// Fetch a history range; chunks ride the bound history stream.
    PaneHistory {
        wire: "pane.history",
        handler: pane_history,
        params: stream::HistoryParams,
        reply: stream::HistoryReply,
        cli: ["pane", "history"],
        about: "Fetch a range of a pane's scrollback over the bound stream",
    }

    /// Declare this client's terminal size and visible panes.
    ClientViewport {
        wire: "client.viewport",
        handler: client_viewport,
        params: client::Viewport,
        reply: client::ViewportReply,
        cli: ["client", "viewport"],
        about: "Declare this client's terminal size and visible panes",
    }

    /// One agent hook invocation, forwarded by `amx _hook`.
    ///
    /// Boxed, like `PersistCommand::Snapshot`: a hook payload is much the
    /// largest thing on the table — it carries a session id, a transcript
    /// path, a tool name and a subagent scope — and [`Call`]'s size is set by
    /// its biggest variant, which every other call would then pay for.
    AgentReport {
        wire: "agent.report",
        handler: agent_report,
        params: Box<agent::ReportParams>,
        reply: agent::ReportReply,
        cli: ["agent", "report"],
        about: "Report one agent hook event (used by `amx _hook`)",
    }

    /// Start a named agent in a new pane and wait for it to be ready.
    AgentStart {
        wire: "agent.start",
        handler: agent_start,
        params: agent::StartParams,
        reply: agent::StartReply,
        cli: ["agent", "start"],
        about: "Start a named agent in a new pane, ready for input",
    }

    /// Submit a prompt to an agent, optionally waiting for what it does next.
    AgentPrompt {
        wire: "agent.prompt",
        handler: agent_prompt,
        params: agent::PromptParams,
        reply: agent::PromptReply,
        cli: ["agent", "prompt"],
        about: "Send a prompt to an agent, optionally waiting for it to finish",
    }

    /// Report how a pane's status was detected, rule by rule.
    AgentExplain {
        wire: "agent.explain",
        handler: agent_explain,
        params: agent::ExplainParams,
        reply: agent::ExplainReply,
        cli: ["agent", "explain"],
        about: "Explain how a pane's agent status was detected",
    }

    /// Focus the head of the attention queue.
    AgentNext {
        wire: "agent.next",
        handler: agent_next,
        params: agent::NextParams,
        reply: agent::NextReply,
        cli: ["agent", "next"],
        about: "Focus the agent at the head of the attention queue",
    }

    /// Every tracked agent, with what it is doing and the line it last drew.
    ///
    /// D15's one data source, read by three surfaces: the status line's
    /// per-workspace breakdown, the agents view and `amx agents`. One reply,
    /// one `Core` round trip, whatever the pane count (D-M4-2).
    AgentList {
        wire: "agent.list",
        handler: agent_list,
        params: agent::ListParams,
        reply: agent::ListReply,
        cli: ["agent", "list"],
        about: "List every tracked agent with its status, reason and last line",
    }

    /// Wait for a pane's agent status, or for its process to end.
    Wait {
        wire: "wait",
        handler: wait,
        params: wait::WaitParams,
        reply: wait::WaitReply,
        cli: ["wait"],
        about: "Wait until a pane's agent is blocked or idle, or its process exits",
    }

    /// Subscribe this connection to the event bus.
    EventsSubscribe {
        wire: "events.subscribe",
        handler: events_subscribe,
        params: wait::SubscribeParams,
        reply: wait::SubscribeReply,
        cli: ["events", "subscribe"],
        about: "Stream bus events on this connection as notifications",
    }

    /// Write text into a pane's process, verbatim.
    PaneSendText {
        wire: "pane.send_text",
        handler: pane_send_text,
        params: pane::SendTextParams,
        reply: pane::SendTextReply,
        cli: ["pane", "send-text"],
        about: "Write text into a pane's process, verbatim",
    }

    /// Encode and send key combos to a pane's process.
    PaneSendKeys {
        wire: "pane.send_keys",
        handler: pane_send_keys,
        params: pane::SendKeysParams,
        reply: pane::SendKeysReply,
        cli: ["pane", "send-keys"],
        about: "Send key combos (`ctrl+h`, `f1`, …) to a pane's process",
    }

    /// Type a line into a pane and submit it, bracketed-paste aware.
    PaneRun {
        wire: "pane.run",
        handler: pane_run,
        params: pane::RunParams,
        reply: pane::RunReply,
        cli: ["pane", "run"],
        about: "Type a line into a pane and submit it",
    }

    /// Read a pane's visible grid as text.
    PaneRead {
        wire: "pane.read",
        handler: pane_read,
        params: pane::ReadParams,
        reply: pane::ReadReply,
        cli: ["pane", "read"],
        about: "Read a pane's visible screen as text",
    }

    /// Wait for a pattern to appear on a pane's screen.
    PaneWaitOutput {
        wire: "pane.wait_output",
        handler: pane_wait_output,
        params: wait::WaitOutputParams,
        reply: wait::WaitOutputReply,
        cli: ["pane", "wait-output"],
        about: "Wait for text or a regex to appear on a pane's screen",
    }
}
