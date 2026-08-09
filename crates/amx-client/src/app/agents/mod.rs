//! D15 surface 2: the agents view — the board a user surveys before they dive.
//!
//! `docs/10-attention-surfaces.md` §D15's scenario is 25 agents across five
//! projects. The shipped surfaces answer *how many need me* (`⚑N`) and *take me
//! to the next one* (`next-attention`); neither answers **which agents, of which
//! project, are blocked on what** without jumping into each in turn. This is
//! that answer: one screen, one row per agent, ordered so the top row is always
//! whoever has waited longest.
//!
//! ```text
//! agents — 25 · ⚑5 · by workspace
//! api/backend     ⚑ blocked  permission_dial   4m │ Allow Bash(git push origin main)? (y/n)
//! docs/writer     ⚑ blocked  permission_dial   7m │ Allow Write(chapter-3.md)? (y/n)
//! api/tests       ● working                    2m │ Running cargo test --workspace…
//! 12 idle
//! ```
//!
//! # It is the picker with one extension, and nothing more
//!
//! D15 licenses exactly one addition to the fuzzy-list primitive — "entries may
//! carry a live detail line, and the view may reserve a live peek region" — and
//! the code holds the primitive to it. [`crate::picker::Picker`] is the matcher
//! and the second column; this module is the ordering, the grouping and the
//! keymap over it. There is no filter syntax (`s:working` and friends are
//! rejected by name in D15) and no launcher: `amx agent start` and the worktree
//! flow own creation, and a monitor that could start things would be a second
//! surface pretending to be one.
//!
//! Two consequences of using the primitive rather than re-implementing it are
//! worth stating, because both look like omissions:
//!
//! - **The match set is a set, not an order.** The picker ranks by compactness;
//!   this view ranks by urgency and uses the picker's matches only to decide
//!   *membership*. A monitor whose rows reshuffle by fuzzy score as the user
//!   types is a monitor nobody can read.
//! - **`Space` is a key, not a character.** It peeks, so a query cannot contain
//!   one — which costs nothing, because what is matched is `workspace/name` and
//!   neither half holds a space.
//!
//! # One `agent.list` per window, never one per pane
//!
//! R-M4-7: one call walks every tracked pane, so a surface refreshing *per pane*
//! at 4 Hz with 25 agents would issue 100 calls a second each walking 25 panes.
//! [`App::refresh_agents`] is the only thing here that talks to the server, it
//! refuses to run twice inside [`AGENTS_REFRESH`], and [`App::agent_lists`]
//! counts what it did so the claim is measured rather than asserted.
//!
//! Every age on the screen is `now − since` from inside one reply (D-M4-4), so
//! nothing here reads this machine's own wall clock and a remote attach is dated
//! as accurately as a local one. The same `now` is handed to the status line
//! ([`App::note_server_clock`]), which is what stops the two surfaces from
//! disagreeing about how long the head of the queue has waited.
//!
//! # The peek region belongs to X15
//!
//! `Space` names a pane; `super::peek` binds it, draws it and releases it. This
//! module holds only the *intent* — [`AgentsUi::peek`], the pane the board wants
//! shown — because a key press is decoded synchronously and opening a peek is a
//! round trip. [`App::settle_peek`] is where the two meet: it compares what the
//! board asked for against [`App::peeked`], and calls
//! [`App::open_peek`](super::App::open_peek) or
//! [`App::close_peek`](super::App::close_peek) once. Moving the selection while
//! a peek is open is the same call with the next pane — X15's "the rebind *is*
//! the move" — so nothing here tracks a stream, and where the region sits comes
//! from [`App::peek_layout`](super::App::peek_layout) and never from a rect this
//! module computed.
//!
//! # Task ownership
//!
//! **X14** owns this module. It landed as a directory rather than the single
//! `app/agents.rs` §5 names, for X07's and X11's reason (R-M1-3, no split waits
//! for the hard limit): the state and its lifecycle here, what one key press
//! does to it in [`keys`], the ordering and the collapse in [`rows`], the paint
//! and the reserved region in [`draw`].
//!
//! [`jump`] came out of it when the M4 exit smoke found that `Enter` landed on
//! the wrong pane: which of two disagreeing focuses wins is a rule of its own,
//! and it is read with `app/events.rs` rather than with the board.

mod draw;
mod jump;
mod keys;
mod rows;

use std::collections::BTreeSet;
use std::io::Write;
use std::os::fd::AsFd;
use std::time::{Duration, Instant};

use amx_core::agent::{AgentState, EpochMillis};
use amx_core::{Effect, PaneId};
use amx_proto::control::Method;
use amx_proto::control::agent::{ListParams, ListReply};

use self::jump::Jump;
use self::keys::Key;
use super::status::ServerClock;
use super::{App, AppError};
use crate::picker::Picker;

pub use rows::Grouping;

/// How often the open view asks the server for a fresh `agent.list`.
///
/// D15's "debounced to ≤ 4 Hz per pane, so 25 flooding agents cannot turn any
/// surface into a firehose", applied to the surface rather than to the pane:
/// one call covers every row, so the window is the view's and not each agent's.
pub const AGENTS_REFRESH: Duration = Duration::from_millis(250);

/// What a row of the list stands for, kept by identity rather than by position.
///
/// The list is rebuilt on every refresh and reordered whenever an agent's state
/// moves, so an index would silently select a different agent four times a
/// second. What the user is pointing at is a pane, or a collapsed run of idle
/// agents in one group; both survive a rebuild that changed everything around
/// them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Cursor {
    /// One agent, by its pane.
    Agent(PaneId),
    /// A group's collapsed idle run.
    Collapsed(rows::GroupKey),
}

/// A one-line text entry the view is taking, and what it is for.
///
/// Not a second dialog type: it is the header line wearing a different hat for
/// as long as one key press asked it to. D15's fence rejects forms; a line that
/// takes a prompt and sends it is the one thing `ctrl+p` and `ctrl+r` cannot be
/// built without, and `Esc` puts the header back.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Entry {
    /// `ctrl+p`: send the text to the selected agent as a prompt.
    Prompt,
    /// `ctrl+r`: give the selected pane the text as its label.
    Rename,
}

impl Entry {
    /// How the header names what is being typed.
    const fn verb(self) -> &'static str {
        match self {
            Self::Prompt => "prompt",
            Self::Rename => "rename",
        }
    }
}

/// The agents view: open or not, and everything it remembers either way.
///
/// One struct rather than an `Option` beside a state block, because D15 requires
/// the filter, the grouping and the selection to **persist per client** across
/// closing and reopening — "that is the 'back to the board' motion" — and a view
/// whose state died with its `Option` would have to copy it out and back on
/// every open. Closing sets [`Self::open`] and touches nothing else.
///
/// None of it is ever sent to the server: 04 §6 puts client presentation state
/// on the client, and the server holds one attention queue for every surface
/// rather than one grouping per viewer.
#[derive(Debug, Default)]
pub struct AgentsUi {
    /// Whether the view is on screen.
    open: bool,
    /// The fuzzy matcher over [`Self::rows`]' labels, and their second column.
    picker: Picker,
    /// Every agent the last reply reported, in the reply's own order.
    rows: Vec<rows::Row>,
    /// What the last rebuild decided is on screen, top to bottom.
    visible: Vec<rows::Line>,
    /// Which of D15's two groupings the rows are laid out by (`ctrl+s`).
    grouping: Grouping,
    /// `ctrl+b`: show only agents that are waiting on the user.
    blocked_only: bool,
    /// What the cursor is on, by identity.
    cursor: Option<Cursor>,
    /// Where that identity was found in [`Self::visible`] last rebuild, so a
    /// selection whose agent has gone keeps its place in the list instead of
    /// jumping to the top.
    at: usize,
    /// Groups whose collapsed idle run the user has expanded.
    expanded: BTreeSet<rows::GroupKey>,
    /// The entry line, when one is open, and the text typed into it.
    entry: Option<(Entry, String)>,
    /// The pane a first `ctrl+x` armed. A second press on the same row kills it;
    /// anything else disarms, which is what makes "press twice" a confirmation
    /// without a dialog to confirm in.
    armed: Option<PaneId>,
    /// The jump `Enter` last made, while it still outranks what the session
    /// says about the workspace it landed in ([`jump`]).
    jumped: Option<Jump>,
    /// The pane the board wants peeked, if `Space` has asked for one.
    ///
    /// An intent and not a stream: the key table is synchronous and opening a
    /// peek is a round trip, so this records what was asked for and
    /// [`App::settle_peek`] is what makes it so.
    peek: Option<PaneId>,
    /// The clock every age here is rendered against, anchored on the server's
    /// own `now` (D-M4-4).
    clock: ServerClock,
    /// How long the attention queue was in the last reply — the `⚑N` the header
    /// shows, taken from the queue and not from a tally of the rows, for the
    /// reason the status line takes it from the same place.
    queued: usize,
    /// When [`App::refresh_agents`] last called, for the 4 Hz window.
    refreshed: Option<Instant>,
    /// How many `agent.list` calls this view has made.
    calls: u64,
    /// Reused row buffer, so a repaint of a settled list allocates nothing.
    scratch: String,
    /// Reused detail buffer, for the same reason.
    details: Vec<String>,
    /// Reused decode buffer for one read's keys.
    keys: Vec<Key>,
}

impl<Fd: AsFd, W: Write> App<Fd, W> {
    /// Open the agents view over whatever the last reply said, and ask for a
    /// fresh one.
    ///
    /// The rows already held are drawn immediately rather than after the round
    /// trip, so reopening the board is instant and the first refresh only
    /// corrects it. A client that has never called sees an empty list for one
    /// window, which is honest: it has not been told anything yet.
    #[must_use]
    pub(super) fn open_agents(&mut self) -> Effect {
        self.agents.open = true;
        // A window that has already elapsed, so the open is followed by a call
        // rather than by up to a quarter second of whatever was on screen last
        // time the board was up.
        self.agents.refreshed = None;
        self.agents.rebuild();
        Effect::Full
    }

    /// Whether the agents view is on screen; its bytes bypass the mode machine.
    #[must_use]
    pub const fn agents_open(&self) -> bool {
        self.agents.open
    }

    /// Close the view and forget the rows, keeping what the client chose.
    ///
    /// Called when the session this client was watching is gone: the rows name
    /// panes a dead server minted, and drawing a board of them would be drawing
    /// a session that no longer exists. The grouping, the filter and the query
    /// are 04 §6's client presentation state and survive — a user who comes back
    /// after a live upgrade comes back to the board they left.
    pub(super) fn close_agents(&mut self) {
        self.agents.open = false;
        self.agents.peek = None;
        self.agents.entry = None;
        self.agents.armed = None;
        // The jump named a pane of a session that is gone; a claim against a
        // dead id could only refuse the successor's own focus.
        self.agents.jumped = None;
        self.agents.rows.clear();
        self.agents.visible.clear();
        self.agents.queued = 0;
        self.agents.refreshed = None;
    }

    /// Whether a surface is drawn over the panes.
    ///
    /// The predicate the two cursor placements ask, in one place: a chrome
    /// surface owns the screen while it is up, so the terminal cursor is hidden
    /// rather than left parked in a pane the user cannot see. A third surface
    /// added without this would put a block cursor in the middle of it.
    pub(super) const fn overlay_open(&self) -> bool {
        self.picker.is_some() || self.copy.is_some() || self.agents.open
    }

    /// How many `agent.list` calls the agents view has made.
    ///
    /// Exposed for the same reason [`App::repaints`] and [`App::resyncs`] are:
    /// R-M4-7's constraint is about *how many calls a surface makes*, which is
    /// invisible in the frame and exact here.
    #[must_use]
    pub const fn agent_lists(&self) -> u64 {
        self.agents.calls
    }

    /// Make the peek region show what the board last asked it to.
    ///
    /// Seam 5's client half: the key table records an intent because it is
    /// synchronous, and this is the one place that turns it into X15's
    /// [`open_peek`](super::App::open_peek) /
    /// [`close_peek`](super::App::close_peek). Called after every read of stdin
    /// (`super::wired`), so `Space`, a selection move under an open peek and the
    /// `Esc` that closes one all take effect before the next frame is drawn.
    ///
    /// Idempotent: a settled peek costs one comparison, and `open_peek` on the
    /// pane already showing answers `Effect::Nothing` without a round trip.
    ///
    /// # Errors
    ///
    /// The socket failed, which the wired loop answers with a reattach.
    pub async fn settle_peek(&mut self) -> Result<(), AppError> {
        let wanted = self.agents.open.then_some(self.agents.peek).flatten();
        if wanted == self.peeked() {
            return Ok(());
        }
        let effect = match wanted {
            Some(pane) => self.open_peek(pane).await?,
            None => self.close_peek().await?,
        };
        self.absorb(effect);
        Ok(())
    }

    /// Ask the server for a fresh `agent.list`, at most once per
    /// [`AGENTS_REFRESH`].
    ///
    /// A no-op when the view is closed and when the window has not elapsed, so
    /// the loop may call it after every wake and the rate is this function's
    /// property rather than its caller's discipline.
    ///
    /// # Errors
    ///
    /// The transport, which the wired loop answers with a reattach. A *refused*
    /// call is not an error here: a board that took the client down because one
    /// projection could not be built would be worse than a board that is one
    /// window stale.
    pub async fn refresh_agents(&mut self) -> Result<(), AppError> {
        if !self.agents.open || !self.agents.due(Instant::now()) {
            return Ok(());
        }
        self.agents.refreshed = Some(Instant::now());
        self.agents.calls += 1;
        let params = serde_json::to_value(ListParams { workspace: None })
            .map_err(|_| AppError::BadState("unencodable agent.list"))?;
        let value = match self.call(Method::AgentList.wire_name(), params).await {
            Ok(value) => value,
            Err(crate::net::NetError::Call(_)) => return Ok(()),
            Err(err) => return Err(err.into()),
        };
        let reply: ListReply =
            serde_json::from_value(value).map_err(|_| AppError::BadState("agent.list reply"))?;
        self.apply_agent_list(reply);
        Ok(())
    }

    /// Fold one `agent.list` reply into the view.
    ///
    /// Public because it is the seam a test drives: assembling 25 tracked agents
    /// behind a real server needs 25 real agent processes, which no runner has
    /// (R-M2-8), and what this surface is about is what it does with a reply
    /// rather than how the reply was fetched.
    pub fn apply_agent_list(&mut self, reply: ListReply) {
        // The server's own clock, to both surfaces that render an age against
        // it: X11's status line asked for exactly this call so its `4m` and this
        // view's `4m` cannot disagree about the same wait.
        self.note_server_clock(reply.now);
        self.agents.clock.observe_at(reply.now, Instant::now());
        self.agents.queued = reply.attention.len();
        self.agents.absorb_reply(reply);
        self.absorb(Effect::Full);
    }
}

impl AgentsUi {
    /// Whether a refresh is owed at `at`.
    fn due(&self, at: Instant) -> bool {
        self.refreshed
            .is_none_or(|last| at.saturating_duration_since(last) >= AGENTS_REFRESH)
    }

    /// The agent the cursor is on, if it is on one rather than on a collapsed
    /// run.
    fn selected_row(&self) -> Option<&rows::Row> {
        match self.visible.get(self.at)? {
            rows::Line::Agent(at) => self.rows.get(*at),
            rows::Line::Collapsed { .. } => None,
        }
    }

    /// The pane the cursor is on.
    fn selected_pane(&self) -> Option<PaneId> {
        self.selected_row().map(|row| row.pane)
    }

    /// The row a pane holds, for naming an armed kill after the list beneath it
    /// has moved.
    fn row_of(&self, pane: PaneId) -> Option<&rows::Row> {
        self.rows.iter().find(|row| row.pane == pane)
    }
}

/// A pane's agent status as one of D15's three bands.
///
/// The five wire states collapse to three because that is what the ordering and
/// the idle collapse are written in terms of: tier 3's `busy` is an unidentified
/// program producing output and belongs beside `working`, its `quiet` beside
/// `idle`, and neither can ever be `blocked` (04 §5 — a probe cannot spell one).
const fn band(state: AgentState) -> rows::Band {
    match state {
        AgentState::Blocked => rows::Band::Blocked,
        AgentState::Working | AgentState::Busy => rows::Band::Working,
        AgentState::Idle | AgentState::Quiet => rows::Band::Idle,
    }
}

/// How long ago `since` was, against the server's own clock.
fn age_of(clock: &ServerClock, since: Option<EpochMillis>, at: Instant) -> Option<u64> {
    clock.age_at(since?, at)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The refresh window, walked with instants rather than by waiting for one.
    ///
    /// The suite proves the half that needs a socket — thirty invitations to
    /// refresh produce one call — and this proves the half that needs a clock.
    /// A test that napped for the window instead would be a wall-clock wait that
    /// reddens on a loaded machine, which is the shape X04 spent a task removing
    /// from four suites.
    #[test]
    fn a_window_that_has_not_elapsed_owes_no_call() {
        let mut view = AgentsUi::default();
        let start = Instant::now();
        assert!(view.due(start), "a board that has never called owes one");

        view.refreshed = Some(start);
        assert!(!view.due(start), "and owes nothing again straight away");
        assert!(
            !view.due(start + AGENTS_REFRESH - Duration::from_millis(1)),
            "nor a millisecond short of the window",
        );
        assert!(
            view.due(start + AGENTS_REFRESH),
            "and owes one the moment it has elapsed",
        );
    }
}
