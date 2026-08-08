//! `session.*` payloads.

use std::path::PathBuf;

use amx_core::agent::AgentSnapshot;
use amx_core::{Layout, PaneId, RowId, Seq, SessionId, ShortNumber, WorkspaceId};
use serde::{Deserialize, Serialize};

use crate::hello::ServerInfo;

/// Parameters of `ping`.
///
/// Empty, and deliberately a struct rather than a unit: adding a field later
/// must not change the wire shape from `null` to an object.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct PingParams {}

/// Reply to `ping`.
///
/// Carries the bus sequence, like every state-carrying reply (04 §2), so even
/// a liveness probe tells the caller where the event stream is.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct PingReply {
    /// Who answered.
    pub server: ServerInfo,
    /// The session instance that answered.
    pub session: SessionId,
    /// The bus head at reply time.
    pub seq: Seq,
}

/// Parameters of `session.state`.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct StateParams {}

/// One workspace, as `session.state` reports it.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct WorkspaceState {
    /// The workspace.
    pub workspace: WorkspaceId,
    /// Its user-visible number.
    pub short: ShortNumber,
    /// Its label, if one was set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// The BSP layout tree, verbatim — the client mirrors it as state (04 §3).
    pub layout: Layout,
    /// The pane focused inside this workspace, if it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus: Option<PaneId>,
    /// The git worktree this workspace was created for, if `amx work` created
    /// it (D-M3-10).
    ///
    /// Additive and optional on the same terms as the label above it. The
    /// server learns no git: the CLI runs `git worktree add`, and this block is
    /// the membership the server has to *remember* so `amx work done` can find
    /// the workspace by branch and restore can check the tree is still there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<amx_core::Worktree>,
}

/// One pane, as `session.state` reports it.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct PaneState {
    /// The pane.
    pub pane: PaneId,
    /// Its user-visible number.
    pub short: ShortNumber,
    /// Its label, if one was set.
    ///
    /// Additive and optional, like the workspace label beside it: absent on
    /// the wire when unset, so a peer built before `pane.rename` existed reads
    /// exactly the bytes it always did (R-M1-8).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Where the pane's process was last known to be.
    ///
    /// Additive and optional under the same R-M1-8 terms as the label above and
    /// the agent below: absent on the wire when the server has recorded none,
    /// so a peer built before this field reads exactly the bytes it always did.
    ///
    /// The value `Core` holds — the spawn cwd, refreshed by the foreground-cwd
    /// probe the persist capture runs — and not a fresh reading: `session.state`
    /// answers synchronously from stored state, and a reply that asked five
    /// parser threads a question would be a reply that can hang.
    ///
    /// D-M3-11 asserts the reply carries cwds and it did not; `amx layout
    /// export` is the caller that needed it, and wrote none until this landed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    /// The pane grid's current rows.
    pub rows: u16,
    /// The pane grid's current columns.
    pub cols: u16,
    /// One past the newest row committed to this pane's history.
    pub history_head: RowId,
    /// The oldest history row still fetchable (the eviction floor).
    pub history_floor: RowId,
    /// What `AgentHub` makes of the program in this pane, when it tracks one.
    ///
    /// Additive and optional, like the label above it: absent on the wire when
    /// the pane has no tracked agent, so a peer built before M2 reads exactly
    /// the bytes it always did. The R-M1-8 precedent — additive optional fields
    /// under the unknown-field contract stay inside protocol v1.
    ///
    /// The same value `AgentHub`'s `StatusView` holds, not a projection of it:
    /// a client rendering one shape while a wait evaluates another is how the
    /// status line and `amx wait` come to disagree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<AgentSnapshot>,
    /// What the pane's application asked its terminal to report about the
    /// mouse, when it asked for anything.
    ///
    /// D9 forwards SGR reports to a pane whose application enabled mouse
    /// reporting and to no other, and until M4 nothing on the wire carried the
    /// answer — `Terminal::mouse_tracking` had no caller and the client's
    /// per-pane gate had no production writer (`docs/11-m4-plan.md` D-M4-1).
    /// This is that answer, and it is read off the parser thread's own terminal
    /// and folded into `Core` the way the history window already is, so
    /// `session.state` still answers synchronously with no parser round trip
    /// (`docs/notes/m4-mouse-path.md` §3).
    ///
    /// Absent when the pane asked for nothing — which is every pane running a
    /// shell — and also when the peer answering does not report this field at
    /// all. The two collapse deliberately: both mean "do not forward", which is
    /// the only decision a reader makes with it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mouse: Option<MouseMode>,
}

/// What a pane's application asked its terminal to report about the mouse.
///
/// Two independent choices, and `docs/notes/m4-mouse-path.md` F-2 is why this
/// is not the boolean the plan first assumed. An application picks *which*
/// events it wants and, separately, *how* they should be encoded; a pane that
/// enabled `?1000` without `?1006` expects the X10 encoding, and forwarding an
/// SGR report to it delivers bytes it cannot parse. A reader has to be able to
/// answer "would this pane understand what I am about to send it", so both
/// halves travel.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub struct MouseMode {
    /// Which events the application asked for.
    pub events: MouseEvents,
    /// How it asked for them to be encoded.
    pub format: MouseFormat,
}

/// The event set a pane's application asked for; mutually exclusive modes.
///
/// The names and the modes behind them are the terminal's own
/// (`vendor/libghostty-vt/src/terminal/mouse.zig:6-13`), not a vocabulary amx
/// invented, so a pane's answer means the same thing here as it does in the
/// emulator that produced it. The `none` case is not a variant: a pane that
/// asked for nothing carries no [`MouseMode`] at all.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseEvents {
    /// `?9` — press only, and no release.
    X10,
    /// `?1000` — press and release.
    Normal,
    /// `?1002` — press, release, and motion while a button is held.
    Button,
    /// `?1003` — press, release, and all motion.
    Any,
}

/// The encoding a pane's application asked reports to arrive in.
///
/// Also the terminal's own list (`mouse.zig:21-28`). Only [`Sgr`](Self::Sgr) is
/// the encoding amx's client recognises on the way in
/// (`amx-client/src/input/mouse.rs`), which is why the honest first cut of
/// forwarding is "forward to panes whose format is `sgr`, and record the drop
/// for the rest" rather than a translation layer nobody has measured.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseFormat {
    /// The original encoding: one byte per coordinate, offset by 32.
    X10,
    /// `?1005` — UTF-8 coordinates.
    Utf8,
    /// `?1006` — `ESC [ < btn ; col ; row (M|m)`.
    Sgr,
    /// `?1015` — urxvt's decimal form.
    Urxvt,
    /// `?1016` — SGR, in pixels rather than cells.
    SgrPixels,
}

/// Reply to `session.state`: the full snapshot a fresh client folds into its
/// model.
///
/// Carries the bus sequence it was captured at (04 §2): "every state-query
/// response … carries the bus sequence number at which it was captured", so a
/// future event subscription can resume from `seq` without a gap between the
/// snapshot and the stream.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct StateReply {
    /// The bus sequence this snapshot was captured at.
    pub seq: Seq,
    /// The workspace the session currently has focused, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focused_workspace: Option<WorkspaceId>,
    /// Every workspace, in creation order.
    pub workspaces: Vec<WorkspaceState>,
    /// Every pane, across all workspaces.
    pub panes: Vec<PaneState>,
    /// The attention queue, in queue order: agents waiting on the user.
    ///
    /// The *same* queue the status line renders `⚑N` from, `agent.next`
    /// focuses the head of, and the reference notifier consumes — D-M2-8 makes
    /// `session.state` the query, so there is no second "read the queue"
    /// method that could answer differently. Ordered by block time; a pane that
    /// unblocked and blocked again is at the tail.
    ///
    /// Additive: empty on a session with no blocked agents, and absent from the
    /// bytes a pre-M2 peer parses.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attention: Vec<PaneId>,
    /// What this server's startup restore cost, if it performed one.
    ///
    /// Counts only: the entries are served whole by `session.report`. They
    /// ride here so an attaching client can render the status-line indicator
    /// without a second call — the indicator is the reason 04 §6 says restore
    /// loss is "never log-only". Absent when the server started fresh.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restore: Option<RestoreSummary>,
}

/// Parameters of `session.report`.
///
/// Empty, and a struct rather than a unit for the same reason [`PingParams`]
/// is: a field added later must not change the wire shape.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct ReportParams {}

/// Reply to `session.report`: the restore report, entry by entry.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct ReportReply {
    /// The bus sequence this report was read at.
    pub seq: Seq,
    /// The report itself, empty when the restore lost nothing.
    pub report: RestoreReport,
    /// What the last live-upgrade handoff did, if this server has seen one.
    ///
    /// Additive and optional, under the same R-M1-8 contract as every other
    /// field on this surface: absent from the bytes a pre-M3 peer parses, and
    /// absent on a server that has never been asked to hand itself over.
    ///
    /// It rides `session.report` rather than getting a method of its own
    /// because a refused or aborted handoff is exactly the kind of loss this
    /// reply already exists to make queryable (04 §6, "never log-only"): the
    /// caller's own connection dies at gateway retirement, so a handoff is
    /// observed by reconnecting and then asking what happened (D-M3-8).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handoff: Option<HandoffAttempt>,
}

/// Parameters of `session.handoff`.
///
/// The one new method row of M3 (`docs/09-m3-plan.md` §4). Deliberately small:
/// the caller stages a binary and names it, and everything else about the
/// swap — the token, the private socket, the stage timeouts — is the server's
/// own business.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct HandoffParams {
    /// The staged binary to hand the session over to.
    ///
    /// Already verified and in place by the time this call is made: `amx
    /// update apply` stages, checksums and renames first, and only then asks
    /// for the swap (D-M3-8).
    pub binary: PathBuf,
    /// How long any one stage of the protocol may take, in milliseconds.
    ///
    /// Absent means the server's own budget, which is herdr's 30 s per stage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

/// Reply to `session.handoff`: accepted, or refused with the reason.
///
/// Acceptance is not completion. The exporter retires its gateway partway
/// through the protocol, so the connection that made this call dies before the
/// swap finishes by design; the caller learns the outcome by reconnecting and
/// reading [`ReportReply::handoff`] (D-M3-8).
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct HandoffReply {
    /// Whether the server accepted the handoff and started the protocol.
    pub accepted: bool,
    /// Why it was refused, for a human. Absent when it was accepted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The bus sequence at reply time.
    pub seq: Seq,
}

/// How far a handoff got, by the stage names of `docs/09-m3-plan.md` §3.
///
/// Ordered as the protocol runs, so a reader can say "it got at least as far
/// as X" without a table of its own.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HandoffStage {
    /// `<binary> _handoff-caps` ran, before any pane was touched.
    PreFlight,
    /// Panes were quiesced and their state captured.
    Quiesce,
    /// The manifest line was sent and the importer validated it.
    Manifest,
    /// The pty descriptors crossed, one message per pane.
    Descriptors,
    /// The importer rebuilt the session and reported `restored`.
    Restore,
    /// The exporter retired its gateway and unlinked the session socket.
    Retire,
    /// The importer bound the session socket and reported `ready`.
    Ready,
    /// Ownership transferred: the exporter reported `committed`.
    Commit,
}

/// What became of one handoff attempt.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HandoffOutcome {
    /// Refused before anything was touched — a bad binary, a window that does
    /// not overlap, a handoff already running.
    Refused,
    /// Started and rolled back: panes resumed, the socket answers again, and
    /// this is still the server the caller was talking to.
    Aborted,
    /// Ownership transferred. Only a successor ever reports this, since the
    /// exporter is gone by the time anyone could ask it.
    Committed,
}

/// One handoff attempt, as `session.report` tells it.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct HandoffAttempt {
    /// What became of it.
    pub outcome: HandoffOutcome,
    /// The last stage it completed.
    pub stage: HandoffStage,
    /// The binary it was aimed at.
    pub binary: PathBuf,
    /// Why, in one line, for a human reading `amx session report`.
    ///
    /// Absent on a committed handoff, which needs no explanation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// What a restore cost, in counts.
///
/// The summary of a [`RestoreReport`], and derivable from it — but carried
/// separately on `session.state` so the common case (render an indicator)
/// never fetches the entries.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct RestoreSummary {
    /// Workspaces and panes that came back.
    pub restored: u32,
    /// Entries that were pruned outright.
    pub lost: u32,
    /// Entries that came back with something missing.
    pub degraded: u32,
}

impl RestoreSummary {
    /// Whether anything at all went wrong.
    #[must_use]
    pub const fn is_clean(&self) -> bool {
        self.lost == 0 && self.degraded == 0
    }
}

/// Everything a restore could not do faithfully.
///
/// 04 §6: restore loss is "shown in the status line and queryable via `amx
/// session report` — never log-only", which is why this is a reply payload and
/// not a `warn!`. An empty report is the successful case, not an absent one.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct RestoreReport {
    /// Every entry, in the order restore produced them.
    #[serde(default)]
    pub entries: Vec<RestoreLoss>,
}

impl RestoreReport {
    /// Count this report's entries by severity.
    ///
    /// `restored` is not derivable from the entries — restore knows it and the
    /// report does not — so the caller supplies it.
    #[must_use]
    pub fn summary(&self, restored: u32) -> RestoreSummary {
        let count = |severity| {
            u32::try_from(
                self.entries
                    .iter()
                    .filter(|entry| entry.severity == severity)
                    .count(),
            )
            .unwrap_or(u32::MAX)
        };
        RestoreSummary {
            restored,
            lost: count(RestoreSeverity::Lost),
            degraded: count(RestoreSeverity::Degraded),
        }
    }
}

/// How badly one entry fared.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RestoreSeverity {
    /// It was pruned: a failed spawn, or a workspace left with no panes.
    Lost,
    /// It came back with something missing: a vanished cwd, an unreadable
    /// scrollback sidecar.
    Degraded,
}

/// What a restore entry is about.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RestoreEntity {
    /// The snapshot as a whole — unreadable, or from a newer amx.
    Session,
    /// One workspace.
    Workspace,
    /// One pane.
    Pane,
}

/// One thing a restore lost or degraded.
///
/// Named for the common case; a `Degraded` entry is a partial loss and lands
/// here too, because the surfaces that consume this — the status line, `amx
/// session report` — want one list, ordered as restore produced it.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct RestoreLoss {
    /// How badly this one fared.
    pub severity: RestoreSeverity,
    /// What kind of thing it is.
    pub entity: RestoreEntity,
    /// The workspace, when the entry names one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<WorkspaceId>,
    /// The pane, when the entry names one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane: Option<PaneId>,
    /// The label the entity had in the snapshot, if it had one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// The path the entry is about: a pane's saved cwd, a sidecar file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    /// Why, in one line, for a human reading `amx session report`.
    pub reason: String,
}
