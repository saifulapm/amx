//! `pane.*` payloads.
//!
//! M0 and M1's layout verbs take a [`PaneId`] outright, because a client that
//! has just been handed a session tree knows one. M2's *driving* verbs — 04
//! §8's `send-text`, `send-keys`, `run`, `read`, `wait-output` — take a
//! [`PaneTarget`] instead, because the things that use them are scripts, tests
//! and agents, for which "the pane called `build`" is the address a human wrote
//! down. See [`PaneTarget`] for what that resolves to.
//!
//! # Task ownership
//!
//! V02 froze the shapes. **V12** fills `pane.send_text`, `pane.send_keys`,
//! `pane.run` and `pane.read`; `pane.wait_output`'s payloads live in
//! [`wait`](super::wait) with the other two long-polls, and V11 fills it.

use std::path::PathBuf;

use amx_core::{PaneId, Seq, ShortNumber, WorkspaceId};
use serde::{Deserialize, Serialize};

/// A pane named by UUID or by label.
///
/// D-M2-9: "every agent verb resolves its target as pane-UUID-or-label, where
/// a label match must be unique among *agent* panes (ambiguity is an error
/// naming the candidates)". The same wire shape serves the pane-driving verbs
/// with the wider resolver — any pane, not only agent panes — because the
/// alternative is two identical string types whose only difference is which
/// function is allowed to read them.
///
/// A bare string on the wire. Resolution is the server's (V13's
/// `agent/address.rs`), and its order is fixed: a UUID first, then a label
/// match that must be unique. That order is what stops a pane labelled with
/// another pane's UUID from shadowing it.
///
/// Five identical Claude panes are only orchestratable if they have names
/// (04 §5), and in amx the name is the pane's label — the one M1 already
/// persists, renames and shows in the picker.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PaneTarget(String);

impl PaneTarget {
    /// Adopt a target as the caller wrote it.
    ///
    /// Total on purpose: an unresolvable target is an error the *server*
    /// reports, naming the candidates it found, because only the server knows
    /// what panes exist. Refusing to build one here would only move the same
    /// failure earlier and make it less informative.
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        Self(text.into())
    }

    /// The target, as written.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<PaneId> for PaneTarget {
    fn from(pane: PaneId) -> Self {
        Self(pane.to_string())
    }
}

impl std::fmt::Display for PaneTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Which way a split cuts the pane it is applied to.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SplitDirection {
    /// Cut left/right: the new pane sits beside the old one.
    Vertical,
    /// Cut top/bottom: the new pane sits below the old one.
    Horizontal,
}

/// Parameters of `pane.split`.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct SplitParams {
    /// The pane to split.
    pub pane: PaneId,
    /// Which way to cut it.
    pub direction: SplitDirection,
    /// Command to run, or the user's shell when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<Vec<String>>,
    /// Working directory override.
    ///
    /// Absent means the default, which is *not* the source pane's own cwd but
    /// its **foreground process** cwd (04 §7: "split and land in the same
    /// directory"), falling back to the pane's cwd when that is unreadable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
}

/// Reply to `pane.split`.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct SplitReply {
    /// The new pane.
    pub pane: PaneId,
    /// Its user-visible number.
    pub short: ShortNumber,
    /// The bus sequence at which the pane existed.
    pub seq: Seq,
}

/// Parameters of `pane.zoom`.
///
/// Zoom toggles rather than taking a target state: 04 §7's prefix-mode `zoom`
/// is one-shot, and T16's acceptance test
/// `zoom_projects_one_pane_to_the_full_workspace_and_restores_exactly` only
/// makes sense if the second call is what restores it.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct ZoomParams {
    /// The pane to toggle zoom on.
    pub pane: PaneId,
}

/// Reply to `pane.zoom`.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct ZoomReply {
    /// Whether the pane is zoomed after this call.
    pub zoomed: bool,
    /// The bus sequence at which the zoom state changed.
    pub seq: Seq,
}

/// Parameters of `pane.swap`.
///
/// Exchanges two panes' positions in the layout tree. Neither pane's identity
/// nor its process is touched (T16's `swap_does_not_restart_either_process`).
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct SwapParams {
    /// One pane to swap.
    pub pane: PaneId,
    /// The pane to swap it with.
    pub with: PaneId,
}

/// Reply to `pane.swap`.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct SwapReply {
    /// The bus sequence at which the swap took effect.
    pub seq: Seq,
}

/// Parameters of `pane.move`.
///
/// Moves a pane into a different workspace; 04 §7 has the picker choose the
/// target. The process in the pane is never restarted (T16's
/// `move_pane_between_workspaces_does_not_restart_the_process`).
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct MoveParams {
    /// The pane to move.
    pub pane: PaneId,
    /// The workspace to move it into.
    pub to: WorkspaceId,
}

/// Reply to `pane.move`.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct MoveReply {
    /// The bus sequence at which the move took effect.
    pub seq: Seq,
}

/// A compass direction on the wire: which way focus moves (`hjkl`) or which
/// way a pane's slot grows (`HJKL`).
///
/// Distinct from [`SplitDirection`], which names a cut; this maps one to one
/// onto `amx_core::Direction`, the type both focus movement and resize take.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MoveDirection {
    /// `h`.
    Left,
    /// `j`.
    Down,
    /// `k`.
    Up,
    /// `l`.
    Right,
}

/// Parameters of `pane.focus`.
///
/// Mirrors `SessionState::move_focus`'s signature: the server moves its own
/// canonical focus to the geometric neighbour of whatever it currently has
/// focused. A client echoing local `hjkl` movement through this call keeps
/// the two focus states from silently diverging — the reply names the pane
/// the server actually landed on, which is the ground truth.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct FocusParams {
    /// The workspace whose focus moves.
    pub workspace: WorkspaceId,
    /// Which way it moves.
    pub direction: MoveDirection,
}

/// Reply to `pane.focus`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct FocusReply {
    /// The pane focused after this call: the neighbour if one existed, the
    /// unchanged focus if not (already at that edge — a legal no-op), or
    /// `None` if the workspace holds no panes at all.
    pub pane: Option<PaneId>,
    /// The bus sequence at which this focus held.
    pub seq: Seq,
}

/// Parameters of `pane.resize`.
///
/// Mirrors `SessionState::resize`: nudges the ratio of the split immediately
/// containing `pane` — whatever its axis — clamped by the layout. `Right`/
/// `Down` grow `pane`'s slot by `delta` of that ratio, `Left`/`Up` shrink it.
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub struct ResizeParams {
    /// The pane whose slot changes.
    pub pane: PaneId,
    /// Which way the slot grows (or, for `Left`/`Up`, shrinks).
    pub direction: MoveDirection,
    /// How much of the split's ratio to move. Must be finite and
    /// non-negative; the sign comes from `direction`.
    pub delta: f32,
}

/// Reply to `pane.resize`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct ResizeReply {
    /// Whether any rect actually changed — `false` for the legal no-op of
    /// resizing a workspace's sole pane, which has no split to nudge.
    pub resized: bool,
    /// The bus sequence at which the new layout held.
    pub seq: Seq,
}

/// Parameters of `pane.rename`.
///
/// The workspace verb has existed since M0 (`workspace.rename`); this is its
/// pane counterpart, and M1 adds it because a label is one of the things a
/// snapshot restores — an unlabelled pane is indistinguishable from every
/// other shell after a reboot.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct RenameParams {
    /// The pane to rename.
    pub pane: PaneId,
    /// Its new label.
    pub label: String,
}

/// Reply to `pane.rename`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct RenameReply {
    /// The bus sequence at which the new label held.
    pub seq: Seq,
}

/// Parameters of `pane.close`.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct CloseParams {
    /// The pane to close.
    pub pane: PaneId,
}

/// Reply to `pane.close`.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct CloseReply {
    /// The bus sequence at which the pane was gone.
    pub seq: Seq,
}

/// Parameters of `pane.send_text`.
///
/// The rawest of the driving verbs: the bytes go to the child verbatim, with no
/// bracketing and no submit. `pane.run` is the one that wraps and submits.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct SendTextParams {
    /// Which pane.
    pub target: PaneTarget,
    /// The text, delivered to the child's pty unchanged.
    pub text: String,
}

/// Reply to `pane.send_text`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct SendTextReply {
    /// The pane written to.
    pub pane: PaneId,
    /// The bus head at reply time.
    pub seq: Seq,
}

/// Parameters of `pane.send_keys`.
///
/// The key-combo grammar of 04 §8: `ctrl+h`, `f1`, `enter`, `alt+shift+tab`,
/// and a bare character for itself. Carried as strings on the wire and parsed
/// server-side, for two reasons — the encoding depends on the pane's kitty
/// keyboard flags, which the *parser thread* owns and not the connection; and a
/// grammar frozen into wire types could not grow a key without a protocol bump.
///
/// A combo this build cannot parse is `INVALID_PARAMS` naming the offending
/// element, never a silently dropped keystroke.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct SendKeysParams {
    /// Which pane.
    pub target: PaneTarget,
    /// The combos, in order.
    pub keys: Vec<String>,
}

/// Reply to `pane.send_keys`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct SendKeysReply {
    /// The pane written to.
    pub pane: PaneId,
    /// How many combos were encoded and sent.
    pub keys: u32,
    /// The bus head at reply time.
    pub seq: Seq,
}

/// Parameters of `pane.run`.
///
/// 04 §8's "bracketed-paste-aware atomic text+submit": the text is wrapped in
/// paste markers **only when the application in the pane enabled paste mode**,
/// then submitted. Sending the markers to an application that did not ask for
/// them types `[200~` into it, which is the failure this verb exists to avoid.
///
/// Load-bearing well past its own row: `agent.prompt` rides it, and so does the
/// resume path, which types a planned argv into a restored shell so the
/// invocation lands in the user's own history (D-M2-7).
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct RunParams {
    /// Which pane.
    pub target: PaneTarget,
    /// The text to type and submit.
    pub text: String,
}

/// Reply to `pane.run`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct RunReply {
    /// The pane it went to.
    pub pane: PaneId,
    /// Whether the text was bracketed, which is a fact about the application in
    /// the pane and worth reporting rather than inferring.
    pub bracketed: bool,
    /// The bus head at reply time.
    pub seq: Seq,
}

/// Parameters of `pane.read`.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct ReadParams {
    /// Which pane.
    pub target: PaneTarget,
    /// How many rows from the bottom. Absent reads the whole visible grid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lines: Option<u16>,
}

/// Reply to `pane.read`: the visible grid, as text.
///
/// **The visible grid**, which in amx is by construction the live bottom:
/// scrollback and scroll position are client-side (04 §3), so the server's
/// published snapshot cannot be a scrolled view. herdr had to anchor its
/// detection buffer to the scrollback bottom and regression-test that a
/// scrolled viewport never moved it; here there is nothing to anchor.
///
/// Scrollback proper is `pane.history`, which serves stable row ids over a
/// bound binary stream.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct ReadReply {
    /// The pane read.
    pub pane: PaneId,
    /// The rows, top to bottom, unstyled.
    pub rows: Vec<String>,
    /// The bus sequence the grid was read at.
    pub seq: Seq,
}
