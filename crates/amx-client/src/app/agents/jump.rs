//! Which focus wins when the board's `Enter` and the session disagree.
//!
//! D15's jump is the one focus this terminal chooses that it cannot tell the
//! session about. `pane.focus` speaks directions and `workspace.switch` carries
//! no pane, so the switch the jump makes is answered with whichever pane that
//! workspace was already remembering — on the bus as `FocusChanged`, and again
//! in the snapshot `mutates_layout` re-reads after the call. Both arrive after
//! the jump has written its pane, and until this module existed both overwrote
//! it: the M4 exit smoke selected `exp/exp-3`, pressed `Enter` and landed on
//! `exp-5` three runs in a row (`docs/notes/m4-live-smoke.md` §5.5, §6.4).
//!
//! Settling that by ordering the two writes would settle nothing — a race that
//! resolves one way on a unix socket resolves the other over a slow link — so
//! the precedence is stated as a rule about *content*:
//!
//! **A focus this terminal chose outranks a restatement of the focus the
//! session already had. A focus the session actually moved outranks the jump.**
//!
//! The durable repair is a pane argument on `workspace.switch`, which is a wire
//! change and belongs in a plan rather than in a fix; until then the client
//! carries the pane half itself, exactly as a numeric jump already does
//! (`super::super::actions`).

use std::io::Write;
use std::os::fd::AsFd;

use amx_core::{PaneId, WorkspaceId};

use super::App;

/// The jump `Enter` last made, and what the session was saying about that
/// workspace when it made it.
///
/// Recording the answer that is *owed* — the focus this client had for the
/// workspace before it jumped — is what lets [`App::jump_outranks`] tell the
/// switch's own restatement from a focus the session really moved, without
/// either of them having to arrive first.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) struct Jump {
    /// The workspace jumped into.
    pub(super) workspace: WorkspaceId,
    /// The selected agent's pane: what this terminal chose.
    pub(super) pane: PaneId,
    /// The focus this client had recorded for [`Self::workspace`] when it
    /// jumped — the pane the switch will be answered with.
    pub(super) echo: Option<PaneId>,
}

impl<Fd: AsFd, W: Write> App<Fd, W> {
    /// Record the jump `Enter` is about to make, so the switch it makes cannot
    /// undo it.
    ///
    /// Called before the local focus moves, because what is recorded is the
    /// focus this client had for `workspace` *before* the jump: that is the
    /// pane the server will name back.
    pub(in crate::app) fn note_jump(&mut self, workspace: WorkspaceId, pane: PaneId) {
        self.agents.jumped = Some(Jump {
            workspace,
            pane,
            echo: self.focus.get(&workspace).copied(),
        });
    }

    /// Whether `reported` — a focus the session states for `workspace` — must
    /// give way to the pane this terminal jumped to.
    ///
    /// The two halves of the rule this module opens with, both decided on
    /// content and neither on arrival order:
    ///
    /// - `reported == echo` is the switch's own answer, the session telling this
    ///   client what it already knew. It carries no news, and folding it would
    ///   land the user on a pane they did not pick. Refused, however many times
    ///   it arrives and by whichever path — the event and the snapshot both
    ///   carry it.
    /// - Anything else is news: `agent.next` moving focus to the head of the
    ///   queue, another client's `pane.focus`, a restore. Folded, with the jump
    ///   dropped along with it — a claim that outranked those would have fixed
    ///   the board by breaking "handle the next one".
    ///
    /// The jump is dropped too the moment this terminal's own focus for that
    /// workspace is no longer the pane it jumped to, which is what `hjkl`, a
    /// numeric jump, the picker's pane row and the next jump each write
    /// (`super::super::actions`, `super::super::overlay`). So the claim expires
    /// by being superseded rather than by a timer or a count of folds.
    pub(in crate::app) fn jump_outranks(
        &mut self,
        workspace: WorkspaceId,
        reported: PaneId,
    ) -> bool {
        let Some(jump) = self.agents.jumped else {
            return false;
        };
        if jump.workspace != workspace {
            return false;
        }
        if jump.echo != Some(reported) || self.focus.get(&workspace) != Some(&jump.pane) {
            self.agents.jumped = None;
            return false;
        }
        true
    }
}
