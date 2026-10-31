//! The view's list of agents: grouping, ordering, folding, narrowing, and the
//! cursor over the resulting lines.
//!
//! - Groups read Pinned, Review, NeedsInput, Working, Completed, Asleep,
//!   Shelved. Inside
//!   a group, agents keep start order, except Completed, newest ending first.
//!   A hand-made order for a group overrides both.
//! - The state, project and repo axes draw agents in the same order, so
//!   turning the axis never changes a row's neighbours.
//! - A group or project past [`FOLD_AT`] rows folds the rest behind a count.
//!   The fold does not depend on terminal height.
//! - An agent hidden by a narrowing is counted by nothing and drawn nowhere.
//! - Shut headings, opened folds, pins, sleeps and hand-made orders are kept
//!   by group, project path or agent id, never by line number, because the
//!   list is rebuilt on every reading.

use crate::derive::{Evidence, View};
use crate::pr::{self, Pr, Standing};
use crate::store::{Ask, Meta, Phase};
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Rows of one group shown before the rest fold behind a count.
pub const FOLD_AT: usize = 30;

/// The heading an agent is listed under.
///
/// Serialized by name because hand-made group orders are persisted per group.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Group {
    /// Pinned by the user, whatever its phase.
    Pinned,
    /// Turn over, with a pull request on its branch still open for review.
    Review,
    /// Stopped on a question.
    NeedsInput,
    /// Mid-turn.
    Working,
    /// Turn over: idle, done, failed, stopped or unknown. The row's glyph says
    /// whether a process is still behind it.
    Completed,
    /// Put to sleep by the user, whatever its phase. The opposite of a pin.
    Asleep,
    /// Turn over, and put away by the user since. See [`shelves`].
    Shelved,
}

impl Group {
    /// Every group, in display order.
    pub const ALL: [Group; 7] = [
        Group::Pinned,
        Group::Review,
        Group::NeedsInput,
        Group::Working,
        Group::Completed,
        Group::Asleep,
        Group::Shelved,
    ];

    /// The group for an agent with this phase and these marks.
    ///
    /// A pin, then a sleep, then a shelve win over the phase. A shelve only
    /// holds while the turn is over (see [`shelves`]). An open pull request
    /// only moves an agent whose turn is over, never one that is asking or
    /// working.
    pub fn of(phase: Phase, held: bool, asleep: bool, shelved: bool, reviewable: bool) -> Group {
        if held {
            return Group::Pinned;
        }
        if asleep {
            return Group::Asleep;
        }
        if shelved {
            return Group::Shelved;
        }
        match phase {
            Phase::Waiting => Group::NeedsInput,
            Phase::Starting | Phase::Working => Group::Working,
            _ if reviewable => Group::Review,
            _ => Group::Completed,
        }
    }

    /// The heading text for the group.
    pub fn title(self) -> &'static str {
        match self {
            Group::Pinned => "Pinned",
            Group::Review => "Ready for review",
            Group::NeedsInput => "Needs input",
            Group::Working => "Working",
            Group::Completed => "Completed",
            Group::Asleep => "Asleep",
            Group::Shelved => "Shelved",
        }
    }

    /// The word the header counts the group by, and the word `s:` narrows by.
    ///
    /// The header shows these words so it doubles as a guide to `s:`. Every
    /// word here must narrow to the group it counts.
    pub fn state(self) -> &'static str {
        match self {
            Group::Pinned => "pinned",
            Group::Review => "review",
            Group::NeedsInput => "waiting",
            Group::Working => "working",
            Group::Completed => "done",
            Group::Asleep => "asleep",
            Group::Shelved => "shelved",
        }
    }
}

/// How the list's headings divide the agents.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Axis {
    /// By [`Group`].
    #[default]
    State,
    /// By project directory, reading amx's own worktree layout.
    Project,
    /// By repository, asking git, so every linked worktree (including ones
    /// `workflow run` cut) heads with its main checkout.
    Repo,
}

/// What the user said about an agent's place on the shelf. Each field lapses
/// once the agent's phase changes after it was set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Shelf {
    /// Epoch seconds of a press of `s` that shelved it by hand.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<u64>,
    /// Epoch seconds of a press of `s` that took it off the shelf when a rule
    /// had put it there, holding the rules off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep: Option<u64>,
}

impl Shelf {
    /// Shelved by hand at `at`.
    pub fn by_hand(at: u64) -> Shelf {
        Shelf {
            at: Some(at),
            keep: None,
        }
    }
}

/// Whether a shelve mark puts the agent on the shelf: its turn is over, and
/// the mark is no older than the agent's last phase change (`since`), so
/// replying to it, or anything else that moves its phase, brings it back.
/// A mark set in the second the agent finished still holds.
pub fn shelves(mark: Option<&Shelf>, phase: Phase, since: u64) -> bool {
    let over = matches!(
        phase,
        Phase::Idle | Phase::Done | Phase::Failed | Phase::Stopped
    );
    over && mark.and_then(|mark| mark.at).is_some_and(|at| at >= since)
}

/// When a rule put the agent on the shelf, if one did. Only a turn that is
/// over is shelved, never while the mark's `keep` is no older than its last
/// phase change (`since`), never while one of its descendants is working or
/// waiting (`busy`), and never while a request on its branch asks for
/// review. Then a merged or closed request shelves it as of `since`, since
/// the cache keeps no merge time. Failing that, so does being over for longer
/// than `after` seconds at `now`, as of when it passed `after`. An `after` of
/// 0 never shelves for idling, and a `since` of 0 (no time recorded) or ahead
/// of `now` is not idle.
pub fn ruled(
    mark: Option<&Shelf>,
    phase: Phase,
    since: u64,
    requests: &[Pr],
    busy: bool,
    after: u64,
    now: u64,
) -> Option<u64> {
    let over = matches!(
        phase,
        Phase::Idle | Phase::Done | Phase::Failed | Phase::Stopped
    );
    let kept = mark
        .and_then(|mark| mark.keep)
        .is_some_and(|at| at >= since);
    if !over || kept || busy || requests.iter().any(|pr| asking(pr.standing)) {
        return None;
    }
    if requests.iter().any(|pr| pr.standing.settled()) {
        return Some(since);
    }
    let due = since.saturating_add(after);
    (after > 0 && since > 0 && now > due).then_some(due)
}

/// The `shelve_after` of the project the agent works in.
fn shelve_after_of(meta: &Meta) -> u64 {
    crate::config::for_project(&crate::spawn::project_dir(meta)).shelve_after
}

/// What `s` did to the row under the cursor. See [`List::shelve_or_return`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shelving {
    Shelved,
    Returned,
    /// Not finished, and why, for the keys row.
    Refused(&'static str),
    /// The cursor is not on an agent.
    Nothing,
}

/// The user's persisted arrangement of the list: axis, pins, sleeps, shelve
/// marks and hand-made group orders.
///
/// Keyed by agent id and group name so a later view can apply it. Ids of
/// agents that no longer exist are harmless misses.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Arrangement {
    axis: Axis,
    held: BTreeSet<String>,
    asleep: BTreeSet<String>,
    shelved: BTreeMap<String, Shelf>,
    order: BTreeMap<Group, Vec<String>>,
}

impl Arrangement {
    /// The arrangement the last view saved under this state root, for readers
    /// outside the view (`park` checks pins).
    ///
    /// Read through [`crate::tui::Remembered`] so there is one parser for the
    /// file. Missing or unreadable files give the default.
    pub fn from_disk(root: &Path) -> Arrangement {
        crate::paths::view_file(root)
            .map(|path| super::Remembered::read(&path).arrangement)
            .unwrap_or_default()
    }

    /// `disk` with this view's changes since `published` applied on top.
    ///
    /// Several views can write the same file. Only fields that differ between
    /// `published` and `local` (the axis, each pin, sleep and shelve mark,
    /// each group's order) are taken, so another view's changes survive.
    pub fn merged(published: &Arrangement, local: &Arrangement, disk: &Arrangement) -> Arrangement {
        let mut merged = disk.clone();
        if local.axis != published.axis {
            merged.axis = local.axis;
        }
        for id in published.held.difference(&local.held) {
            merged.held.remove(id);
        }
        for id in local.held.difference(&published.held) {
            merged.held.insert(id.clone());
        }
        for id in published.asleep.difference(&local.asleep) {
            merged.asleep.remove(id);
        }
        for id in local.asleep.difference(&published.asleep) {
            merged.asleep.insert(id.clone());
        }
        let marked = published.shelved.keys().chain(local.shelved.keys());
        for id in marked {
            match local.shelved.get(id) {
                mark if mark == published.shelved.get(id) => {}
                Some(mark) => {
                    merged.shelved.insert(id.clone(), *mark);
                }
                None => {
                    merged.shelved.remove(id);
                }
            }
        }
        for (group, order) in &local.order {
            if published.order.get(group) != Some(order) {
                merged.order.insert(*group, order.clone());
            }
        }
        merged
    }

    /// Drop the shelve marks of agents for which `recorded` is false, so the
    /// file does not keep marks for every agent ever forgotten.
    pub fn forget_the_gone(&mut self, recorded: impl Fn(&str) -> bool) {
        self.shelved.retain(|id, _| recorded(id));
    }

    /// Whether the agent with this id is pinned. See [`List::holding`].
    pub fn has_pinned(&self, id: &str) -> bool {
        self.held.contains(id)
    }

    /// Whether the agent with this id is asleep. See [`List::sleeping`].
    #[cfg(test)]
    pub fn has_asleep(&self, id: &str) -> bool {
        self.asleep.contains(id)
    }
}

/// What a heading stands for, valid for the current reading only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Under {
    /// A group, on the state axis.
    Group(Group),
    /// An index into the list's project table, so [`Item`] stays `Copy`.
    Project(usize),
}

/// Counts for one heading, taken after narrowing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tally {
    /// Top-level agents under the heading.
    pub members: usize,
    /// Failed agents under the heading, children included.
    pub failures: usize,
    /// Rows per group, indexed by position in [`Group::ALL`].
    ///
    /// Counts children, unlike `members`: this is how much work is under the
    /// heading, and `members` is how many rows opening it brings back.
    pub states: [usize; Group::ALL.len()],
    pub shut: bool,
}

impl Tally {
    /// The non-empty group counts, in [`Group::ALL`] order.
    pub fn doing(&self) -> Vec<(Group, usize)> {
        Group::ALL
            .into_iter()
            .enumerate()
            .filter(|&(at, _)| self.states[at] > 0)
            .map(|(at, group)| (group, self.states[at]))
            .collect()
    }
}

/// One line of the list. The cursor can stop on every line except a blank.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Item {
    Heading(Under, Tally),
    /// An index into the list's views.
    Agent(usize),
    /// The fold under a heading, and how many rows it hides. Opening it
    /// unfolds that heading only.
    Fold(Under, usize),
    /// The fold under a parent row, and how many of its descendants it hides.
    /// Opening it unfolds the parent's heading.
    Sub(usize, usize),
    /// Spacing above a heading.
    Blank,
}

/// A heading identity that survives rebuilds, unlike [`Under`], whose project
/// index changes with every reading.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Key {
    Group(Group),
    Project(PathBuf),
}

/// What the cursor is on, by stable identity.
enum On {
    Agent(String),
    Heading(Key),
    Nothing,
}

impl On {
    fn agent(&self) -> Option<&str> {
        match self {
            On::Agent(id) => Some(id),
            _ => None,
        }
    }
}

/// One change to the narrowing. `None` clears that part. See [`List::narrow`]
/// for how a batch of them combines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Narrow {
    State(Option<String>),
    Name(Option<String>),
}

/// The active narrowing. The state and name parts must both match when set.
///
/// Several states match if any one does, so `s:waiting s:working` shows both
/// groups.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Filters {
    state: Vec<String>,
    name: Option<String>,
}

impl Filters {
    fn keeps(&self, view: &View, group: Group, prs: &[Pr]) -> bool {
        // A group word matches the group, so `s:` leaves exactly what the
        // header counted. Any other word is matched against the phase, so
        // `s:failed` still finds failures. `working` and `done` are both group
        // words and phases, and they must stay group words, or `s:working`
        // would also match a pinned working agent.
        let state = self.state.is_empty()
            || self.state.iter().any(|want| {
                if Group::ALL.iter().any(|group| group.state() == want) {
                    group.state() == want
                } else {
                    view.phase().as_str() == want
                }
            });
        // Id, display name, pull request label (`#12`) and task. Not the
        // summary: it changes as the agent works, so rows would drop out
        // while being read.
        let name = self.name.as_ref().is_none_or(|want| {
            holds(view.id(), want)
                || holds(called(view), want)
                || holds(&view.meta.task, want)
                || prs.iter().any(|pr| holds(&pr.label(), want))
        });
        state && name
    }

    /// The narrowing as the find line would spell it (`s:waiting /port`).
    fn label(&self) -> Option<String> {
        let said: Vec<String> = self
            .state
            .iter()
            .map(|want| format!("s:{want}"))
            .chain(self.name.as_ref().map(|want| format!("/{want}")))
            .collect();
        (!said.is_empty()).then(|| said.join(" "))
    }
}

/// The agents laid out as lines, with a cursor.
#[derive(Debug)]
pub struct List {
    views: Vec<View>,
    items: Vec<Item>,
    cursor: usize,
    /// Whether the cursor has been placed since the list was last empty. The
    /// first placement lands on an agent rather than a heading.
    landed: bool,
    /// The agent the user just removed, so the next reading puts the cursor
    /// on the agent after it rather than on the heading. See [`List::let_go`].
    let_go: Option<String>,
    /// Headings whose fold the user opened.
    unfolded: HashSet<Key>,
    /// Headings the user shut.
    shut: HashSet<Key>,
    /// Pinned agent ids.
    held: BTreeSet<String>,
    /// Sleeping agent ids.
    asleep: BTreeSet<String>,
    /// Shelve marks by agent id, kept whether or not they still hold.
    shelved: BTreeMap<String, Shelf>,
    /// Hand-made group orders, as the ids in the group when it was arranged.
    order: BTreeMap<Group, Vec<String>>,
    axis: Axis,
    /// The path axis shown last, so leaving the state axis alternates between
    /// the two. Starts at `Repo` so the first turn reaches `Project`. Not
    /// persisted.
    last_path: Axis,
    filters: Filters,
    /// Top-level agents per group, cached per rebuild for the header.
    counts: Vec<(Group, usize)>,
    /// Agents waiting on a question, by phase rather than group, so a pinned
    /// or sleeping agent still counts.
    waiting: usize,
    /// The paths the project headings name, in display order.
    projects: Vec<PathBuf>,
    /// Project root per agent id on a path axis. Cached because finding it
    /// probes the disk or runs git, and an agent's directory does not change.
    /// Cleared when the axis turns and pruned to the current fleet on rebuild.
    roots: HashMap<String, PathBuf>,
    /// Each agent's parent index, where the parent is on the list. `None` for a
    /// missing parent, a self-reference, or a link that closes a loop, so such
    /// a child is drawn as a root.
    parents: Vec<Option<usize>>,
    /// Each agent's children, newest first.
    children: Vec<Vec<usize>>,
    /// Top-level agents, in list order.
    tops: Vec<usize>,
    /// Whether a directory is a repository top. Injected for tests.
    probe: fn(&Path) -> bool,
    /// The repository holding a directory, per git. Injected for tests.
    repo_of: fn(&Path) -> Option<PathBuf>,
    /// The branch a repository root has checked out. Injected for tests.
    branch_at: fn(&Path) -> Option<String>,
    /// Checked-out branch per repository root, repo axis only. Cached per root
    /// (a root on no branch included), cleared and pruned with `roots`.
    branches: HashMap<PathBuf, Option<String>>,
    /// Pull requests per agent id, read again on every reading because their
    /// status changes while the row is on screen.
    prs: HashMap<String, Vec<Pr>>,
    /// Source of `prs`. Injected for tests.
    asks: fn(&Meta) -> Vec<Pr>,
    /// The time the rules judge idleness by. 0 until the view says, so a list
    /// that is never told the time shelves nothing for idling.
    now: u64,
    /// `shelve_after` per agent id, read once per agent because the project it
    /// works in does not change. Pruned to the current fleet on each reading.
    afters: HashMap<String, u64>,
    /// Source of `afters`. Injected for tests.
    after: fn(&Meta) -> u64,
    /// `$HOME`, read once, for abbreviating heading paths to `~`.
    home: Option<PathBuf>,
}

impl Default for List {
    fn default() -> List {
        List {
            views: Vec::new(),
            items: Vec::new(),
            cursor: 0,
            landed: false,
            let_go: None,
            unfolded: HashSet::new(),
            // Shelved opens shut in every view: it holds what the user put
            // away.
            shut: HashSet::from([Key::Group(Group::Shelved)]),
            held: BTreeSet::new(),
            asleep: BTreeSet::new(),
            shelved: BTreeMap::new(),
            order: BTreeMap::new(),
            axis: Axis::default(),
            last_path: Axis::Repo,
            filters: Filters::default(),
            counts: Vec::new(),
            waiting: 0,
            projects: Vec::new(),
            roots: HashMap::new(),
            parents: Vec::new(),
            children: Vec::new(),
            tops: Vec::new(),
            probe: holds_a_repository,
            repo_of: main_repo_of,
            branch_at: crate::worktree::branch_at,
            branches: HashMap::new(),
            prs: HashMap::new(),
            asks: pr::of,
            now: 0,
            afters: HashMap::new(),
            after: shelve_after_of,
            home: std::env::home_dir(),
        }
    }
}

impl List {
    /// A list with a fake repository probe and home.
    #[cfg(test)]
    fn probing(probe: fn(&Path) -> bool, home: Option<PathBuf>) -> List {
        List {
            probe,
            home,
            ..List::default()
        }
    }

    /// A list with fake git answers for the repo axis.
    #[cfg(test)]
    fn probing_repos(
        repo_of: fn(&Path) -> Option<PathBuf>,
        branch_at: fn(&Path) -> Option<String>,
        home: Option<PathBuf>,
    ) -> List {
        List {
            repo_of,
            branch_at,
            home,
            ..List::default()
        }
    }

    /// Replace the pull request source with a fake.
    #[cfg(test)]
    pub(super) fn asking(&mut self, asks: fn(&Meta) -> Vec<Pr>) {
        self.asks = asks;
    }

    /// Replace the `shelve_after` source with a fake.
    #[cfg(test)]
    pub(super) fn shelving_after(&mut self, after: fn(&Meta) -> u64) {
        self.after = after;
    }

    /// Judge idleness at `now` from the next rebuild on.
    pub fn time_is(&mut self, now: u64) {
        self.now = now;
    }

    /// Replace the views with a fresh reading and rebuild the lines.
    ///
    /// The cursor follows the agent or heading it was on, since rows move
    /// between groups from one reading to the next.
    pub fn show(&mut self, views: Vec<View>) {
        let on = self.on();
        let placed = self.landed;
        self.remember_the_requests(&views);
        self.remember_the_afters(&views);
        self.views = views;
        self.rebuild(on.agent().filter(|_| placed));
        if placed {
            self.follow(&on);
        }
        // Only the next reading: later ones were not asked for.
        self.let_go = None;
    }

    /// Place the cursor afresh on the next reading, as if the list had just
    /// opened. For a reading of the records alone, which may sort an agent
    /// whose pane is gone where the next reading will not.
    pub fn land_again(&mut self) {
        self.landed = false;
    }

    /// Note that the user removed agent `id`, so that when the next reading
    /// drops it the cursor goes to the agent after it, or the one above if
    /// it was the last.
    pub fn let_go(&mut self, id: &str) {
        self.let_go = Some(id.to_string());
    }

    /// Read every agent's pull requests again. Unlike project roots these are
    /// not cached, because their status changes between readings.
    fn remember_the_requests(&mut self, views: &[View]) {
        self.prs = views
            .iter()
            .map(|view| (view.id().to_string(), (self.asks)(&view.meta)))
            .collect();
    }

    /// Look up the `shelve_after` of every agent not yet seen, and forget the
    /// agents that left.
    fn remember_the_afters(&mut self, views: &[View]) {
        let mut known = std::mem::take(&mut self.afters);
        self.afters = views
            .iter()
            .map(|view| {
                let id = view.id().to_string();
                let after = known
                    .remove(&id)
                    .unwrap_or_else(|| (self.after)(&view.meta));
                (id, after)
            })
            .collect();
    }

    /// The pull requests on this agent's branch, as of the last reading.
    pub fn requests(&self, view: &View) -> &[Pr] {
        self.prs.get(view.id()).map_or(&[], Vec::as_slice)
    }

    pub fn axis(&self) -> Axis {
        self.axis
    }

    /// Turn to the next axis: state, project, state, repo, and around.
    ///
    /// The state axis sits between the two path axes because they often show
    /// the same headings, and a direct turn between them would look like a
    /// no-op. Cached roots are dropped since each path axis resolves them
    /// differently. The cursor keeps its agent.
    pub fn turn(&mut self) {
        let on = self.on();
        self.axis = match self.axis {
            Axis::State => match self.last_path {
                Axis::Repo => Axis::Project,
                _ => Axis::Repo,
            },
            path => {
                self.last_path = path;
                Axis::State
            }
        };
        self.roots.clear();
        self.branches.clear();
        self.rebuild(on.agent());
        self.follow(&on);
    }

    /// The current arrangement, for persisting.
    pub fn arrangement(&self) -> Arrangement {
        Arrangement {
            axis: self.axis,
            held: self.held.clone(),
            asleep: self.asleep.clone(),
            shelved: self.shelved.clone(),
            order: self.order.clone(),
        }
    }

    /// Apply a saved arrangement. The cursor keeps what it was on.
    pub fn arrange(&mut self, arrangement: Arrangement) {
        let on = self.on();
        // Another view may have turned the axis, and cached roots belong to
        // the old one.
        if arrangement.axis != self.axis {
            self.roots.clear();
            self.branches.clear();
        }
        self.axis = arrangement.axis;
        self.held = arrangement.held;
        self.asleep = arrangement.asleep;
        self.shelved = arrangement.shelved;
        self.order = arrangement.order;
        self.rebuild(on.agent());
        self.follow(&on);
    }

    /// Whether this agent is pinned.
    pub fn holding(&self, view: &View) -> bool {
        self.held.contains(view.id())
    }

    /// Whether this agent is asleep.
    pub fn sleeping(&self, view: &View) -> bool {
        self.asleep.contains(view.id())
    }

    /// Whether this agent's shelve mark still holds. See [`shelves`].
    fn shelving(&self, view: &View) -> bool {
        shelves(self.shelved.get(view.id()), view.phase(), view.state.since)
    }

    /// When a rule put agent `n` on the shelf, if one did. See [`ruled`].
    fn ruling(&self, n: usize) -> Option<u64> {
        let view = &self.views[n];
        let after = self.afters.get(view.id()).copied().unwrap_or(0);
        ruled(
            self.shelved.get(view.id()),
            view.phase(),
            view.state.since,
            self.requests(view),
            self.busy(n).is_some(),
            after,
            self.now,
        )
    }

    /// When agent `n` went on the shelf: the press of `s`, while it holds,
    /// else when a rule first held.
    fn shelved_at(&self, n: usize) -> Option<u64> {
        let view = &self.views[n];
        self.shelved
            .get(view.id())
            .and_then(|mark| mark.at)
            .filter(|_| self.shelving(view))
            .or_else(|| self.ruling(n))
    }

    /// Toggle the pin on the agent under the cursor.
    ///
    /// A pin holds across phase changes. Pinning a sleeping agent wakes it.
    /// Returns false when the cursor is not on an agent.
    pub fn hold_or_let_go(&mut self) -> bool {
        let Some(id) = self.selected().map(|view| view.id().to_string()) else {
            return false;
        };
        if !self.held.remove(&id) {
            self.asleep.remove(&id);
            self.held.insert(id);
        }
        let on = self.on();
        self.rebuild(on.agent());
        self.follow(&on);
        true
    }

    /// Toggle sleep on the agent under the cursor.
    ///
    /// Sleep holds across phase changes, and a sleeping agent still counts in
    /// [`List::waiting`]. Sleeping a pinned agent unpins it. Returns false when
    /// the cursor is not on an agent.
    pub fn sleep_or_wake(&mut self) -> bool {
        let Some(id) = self.selected().map(|view| view.id().to_string()) else {
            return false;
        };
        if !self.asleep.remove(&id) {
            self.held.remove(&id);
            self.asleep.insert(id);
        }
        let on = self.on();
        self.rebuild(on.agent());
        self.follow(&on);
        true
    }

    /// Shelve the agent under the cursor at `now`, or put a shelved one back.
    /// Putting back drops a press of `s`, and holds a rule off until the
    /// agent's next phase change.
    ///
    /// Only a finished turn is shelved, and only when no one in its family is
    /// working or waiting, which would pull the row back under Working or
    /// Needs input. Shelving unpins and wakes the agent. Either way the row
    /// leaves the cursor, which goes to the line that followed it.
    pub fn shelve_or_return(&mut self, now: u64) -> Shelving {
        let Some(Item::Agent(n)) = self.items.get(self.cursor).copied() else {
            return Shelving::Nothing;
        };
        let id = self.views[n].id().to_string();
        let done = match self.group(n) {
            Group::Shelved => {
                match self.ruling(n) {
                    Some(_) => self.shelved.insert(
                        id,
                        Shelf {
                            at: None,
                            keep: Some(now),
                        },
                    ),
                    None => self.shelved.remove(&id),
                };
                Shelving::Returned
            }
            _ => {
                if let Some(why) = self.unfinished(n) {
                    return Shelving::Refused(why);
                }
                self.held.remove(&id);
                self.asleep.remove(&id);
                self.shelved.insert(id, Shelf::by_hand(now));
                Shelving::Shelved
            }
        };
        let next = self.after(self.cursor);
        self.rebuild(next.agent());
        self.follow(&next);
        done
    }

    /// Why the agent's row cannot be shelved, if it cannot: its own turn, or
    /// a descendant's, is not over.
    fn unfinished(&self, n: usize) -> Option<&'static str> {
        let pulled = match self.views[n].phase() {
            Phase::Waiting => Some(Group::NeedsInput),
            Phase::Starting | Phase::Working => Some(Group::Working),
            Phase::Unknown => return Some("cannot tell it finished; z sleeps it"),
            _ => self.busy(n),
        };
        match pulled {
            Some(Group::NeedsInput) => Some("waiting on you; z sleeps it"),
            Some(Group::Working) => Some("still working; z sleeps it"),
            _ => None,
        }
    }

    /// What is on the first line below the row on line `at` and the rows
    /// drawn under it, or failing that the nearest line above it under the
    /// same heading, or failing that the row itself, wherever it goes.
    fn after(&self, at: usize) -> On {
        let Some(Item::Agent(n)) = self.items.get(at).copied() else {
            return self.on();
        };
        let under = |m: usize| {
            let mut here = Some(m);
            while let Some(row) = here {
                if row == n {
                    return true;
                }
                here = self.parents[row];
            }
            false
        };
        let below = self.items[at + 1..]
            .iter()
            .position(|item| match item {
                Item::Agent(m) | Item::Sub(m, _) => !under(*m),
                _ => true,
            })
            .map(|skip| at + 1 + skip);
        // Not past the heading: it may go with the row, and the line above it
        // belongs to another group.
        let line = below
            .and_then(|from| (from..self.items.len()).find(|&i| self.items[i] != Item::Blank))
            .or_else(|| {
                (0..at)
                    .rev()
                    .take_while(|&i| !matches!(self.items[i], Item::Heading(..)))
                    .find(|&i| self.items[i] != Item::Blank)
            });
        match line {
            Some(line) => self.on_line(line),
            None => On::Agent(self.views[n].id().to_string()),
        }
    }

    /// Move the agent under the cursor `by` rows within its group.
    ///
    /// Records the whole group's current order, so agents that join later sort
    /// after the arranged ones. Agents hidden by a narrowing are left out of
    /// the recorded order. Returns false when nothing moved.
    pub fn move_by(&mut self, by: isize) -> bool {
        let Some(Item::Agent(n)) = self.items.get(self.cursor).copied() else {
            return false;
        };
        let id = self.views[n].id().to_string();
        let group = self.family(n);
        let mut members: Vec<String> = self
            .ordered()
            .into_iter()
            .filter(|&n| self.family(n) == group)
            .map(|n| self.views[n].id().to_string())
            .collect();

        let Some(at) = members.iter().position(|other| *other == id) else {
            return false;
        };
        let Some(to) = at.checked_add_signed(by).filter(|to| *to < members.len()) else {
            return false;
        };
        // Refuse to move behind a fold, where the cursor could not follow.
        if !self.drawn(&members[to]) {
            return false;
        }

        members.swap(at, to);
        self.order.insert(group, members);
        let on = self.on();
        self.rebuild(on.agent());
        self.follow(&on);
        true
    }

    /// Whether this agent has a line, as opposed to being hidden under a shut
    /// heading or a fold.
    fn drawn(&self, id: &str) -> bool {
        self.items
            .iter()
            .any(|item| self.agent(*item).is_some_and(|view| view.id() == id))
    }

    /// The position of the agent's heading group in [`Group::ALL`].
    fn rank(&self, n: usize) -> usize {
        let group = self.family(n);
        Group::ALL
            .iter()
            .position(|other| *other == group)
            .unwrap_or(Group::ALL.len())
    }

    /// The agent's position in its group's hand-made order, or `usize::MAX`
    /// when it has none.
    fn seat(&self, n: usize) -> usize {
        let view = &self.views[n];
        self.order
            .get(&self.family(n))
            .and_then(|ids| ids.iter().position(|id| id == view.id()))
            .unwrap_or(usize::MAX)
    }

    /// Apply one reading of the find line.
    ///
    /// A batch with any state change replaces the state set (a bare `s:`
    /// clears it) and also replaces the name, which clears it when the batch
    /// has none. A batch with no state change leaves the states alone. The
    /// name is dropped with states because a half-typed `s:waiting s` parses
    /// as a name first, and it must not linger once the line is all states.
    pub fn narrow(&mut self, changes: Vec<Narrow>) {
        let on = self.on();
        let mut states: Option<Vec<String>> = None;
        let mut name: Option<Option<String>> = None;
        for change in changes {
            match change {
                Narrow::State(state) => states.get_or_insert_default().extend(state),
                Narrow::Name(named) => name = Some(named),
            }
        }
        match states {
            Some(states) => {
                self.filters.state = states;
                self.filters.name = name.flatten();
            }
            None => {
                if let Some(name) = name {
                    self.filters.name = name;
                }
            }
        }
        self.rebuild(on.agent());
    }

    /// The active narrowing as find-line text, for the header.
    pub fn narrowing(&self) -> Option<String> {
        self.filters.label()
    }

    /// The heading text.
    pub fn title(&self, under: Under) -> String {
        match under {
            Under::Group(group) => group.title().to_string(),
            Under::Project(n) => match self.projects.get(n) {
                Some(root) => self.path_title(root),
                None => String::new(),
            },
        }
    }

    /// A path heading: the root with `~` for home, plus the checked-out branch
    /// on the repo axis. The branch is what tells the two path axes apart when
    /// they head the same paths.
    fn path_title(&self, root: &Path) -> String {
        let path = shorten(root, self.home.as_deref());
        match self.branches.get(root).and_then(Option::as_deref) {
            Some(branch) => format!("{path} ({branch})"),
            None => path,
        }
    }

    pub fn items(&self) -> &[Item] {
        &self.items
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// The line and [`Under`] of the heading for `key`, if it is drawn.
    pub fn heading_at(&self, key: &Key) -> Option<(usize, Under)> {
        self.items
            .iter()
            .enumerate()
            .find_map(|(at, item)| match item {
                Item::Heading(under, _) if self.key(*under).as_ref() == Some(key) => {
                    Some((at, *under))
                }
                _ => None,
            })
    }

    /// The agent on this line, if any.
    pub fn agent(&self, item: Item) -> Option<&View> {
        match item {
            Item::Agent(n) => self.views.get(n),
            _ => None,
        }
    }

    /// The agent under the cursor, if any.
    pub fn selected(&self) -> Option<&View> {
        self.agent(*self.items.get(self.cursor)?)
    }

    /// Whether the cursor is on a fold line.
    pub fn on_fold(&self) -> bool {
        matches!(
            self.items.get(self.cursor),
            Some(Item::Fold(..) | Item::Sub(..))
        )
    }

    /// Whether the cursor is on a heading.
    pub fn on_heading(&self) -> bool {
        matches!(self.items.get(self.cursor), Some(Item::Heading(..)))
    }

    /// The heading under the cursor, if any.
    pub fn heading(&self) -> Option<Under> {
        match self.items.get(self.cursor) {
            Some(Item::Heading(under, _)) => Some(*under),
            _ => None,
        }
    }

    /// The project of the heading or agent under the cursor, on a path axis.
    /// `None` on the state axis.
    pub fn project_under_cursor(&self) -> Option<PathBuf> {
        if self.axis == Axis::State {
            return None;
        }
        match self.items.get(self.cursor)? {
            Item::Heading(Under::Project(at), _) => self.projects.get(*at).cloned(),
            Item::Agent(n) => Some(self.root_of(*n)),
            _ => None,
        }
    }

    /// Every agent under a heading, in list order.
    ///
    /// Includes agents under a shut heading or behind a fold, so an action on
    /// a heading reaches everything its count claims. Excludes agents hidden
    /// by a narrowing.
    pub fn members(&self, under: Under) -> Vec<&View> {
        self.ordered()
            .into_iter()
            .filter(|&n| self.belongs(n, under))
            .map(|n| &self.views[n])
            .collect()
    }

    /// The current view of the agent with this id.
    pub fn agent_by_id(&self, id: &str) -> Option<&View> {
        self.views.iter().find(|view| view.id() == id)
    }

    /// Whether the narrowing keeps this agent.
    fn keeps(&self, n: usize) -> bool {
        let view = &self.views[n];
        self.filters
            .keeps(view, self.family(n), self.requests(view))
    }

    /// The agent's own group, ignoring its children. See [`Self::family`] for
    /// the heading it is drawn under.
    fn group(&self, n: usize) -> Group {
        let view = &self.views[n];
        Group::of(
            view.phase(),
            self.holding(view),
            self.sleeping(view),
            self.shelving(view) || self.ruling(n).is_some(),
            self.reviewable(view),
        )
    }

    /// The group a row is drawn under: the most urgent group in its subtree.
    ///
    /// A parent whose turn ended while a child still works must not land in
    /// Completed, where `c` would clear the running family unseen. A child's
    /// pin or sleep applies to that child only and does not move the family.
    /// A shelved parent is pulled back only by a child working or waiting,
    /// pinned or asleep or not.
    fn family(&self, n: usize) -> Group {
        let mine = self.group(n);
        if matches!(mine, Group::Pinned | Group::Asleep) {
            return mine;
        }
        match (mine, self.pulled(n)) {
            (Group::Shelved, _) => self.busy(n).unwrap_or(mine),
            (_, Some(group)) => group.min(mine),
            (_, None) => mine,
        }
    }

    /// NeedsInput or Working, for the most urgent of the row's descendants
    /// that is waiting or working by its phase, whatever its marks. A shelve
    /// must not hide one of those, since it would be drawn under the shut
    /// heading with its parent.
    fn busy(&self, n: usize) -> Option<Group> {
        self.nested(&self.children[n])
            .into_iter()
            .filter_map(|at| match self.views[at].phase() {
                Phase::Waiting => Some(Group::NeedsInput),
                Phase::Starting | Phase::Working => Some(Group::Working),
                _ => None,
            })
            .min()
    }

    /// The most urgent group among the row's descendants, leaving out pinned
    /// and sleeping ones. `None` for a row with no such descendant.
    fn pulled(&self, n: usize) -> Option<Group> {
        self.nested(&self.children[n])
            .into_iter()
            .map(|at| self.group(at))
            .filter(|group| !matches!(group, Group::Pinned | Group::Asleep))
            .min()
    }

    /// Whether the agent's turn is over and a pull request on its branch is
    /// still open for review.
    ///
    /// Excludes `Unknown`: Review claims the agent has nothing left to do, and
    /// an unknown phase cannot support that.
    pub fn reviewable(&self, view: &View) -> bool {
        let over = matches!(
            view.phase(),
            Phase::Idle | Phase::Done | Phase::Failed | Phase::Stopped
        );
        over && self.requests(view).iter().any(|pr| asking(pr.standing))
    }

    /// Whether the agent is drawn under this heading, which is decided by its
    /// top-level ancestor.
    fn belongs(&self, n: usize, under: Under) -> bool {
        let anchor = self.anchor_of(n);
        match under {
            Under::Group(group) => self.family(anchor) == group,
            Under::Project(at) => self
                .projects
                .get(at)
                .is_some_and(|root| *root == self.root_of(anchor)),
        }
    }

    /// The agent's top-level ancestor, or itself.
    fn anchor_of(&self, n: usize) -> usize {
        let mut here = n;
        while let Some(parent) = self.parents[here] {
            here = parent;
        }
        here
    }

    /// Toggle the heading under the cursor between shut and open. A shut
    /// heading stays on the list with its rows hidden.
    pub fn shut_or_open(&mut self) {
        let Some(Item::Heading(under, _)) = self.items.get(self.cursor).copied() else {
            return;
        };
        let Some(key) = self.key(under) else {
            return;
        };
        if !self.shut.remove(&key) {
            self.shut.insert(key);
        }
        let on = self.on();
        self.rebuild(on.agent());
        self.follow(&on);
    }

    /// Whether there are no agents at all, on the state axis with nothing
    /// narrowed. The only empty list that gets the first-run screen; an empty
    /// narrowing shows the narrowing instead.
    pub fn unstarted(&self) -> bool {
        self.axis == Axis::State && self.views.is_empty() && self.filters.label().is_none()
    }

    /// Open the fold under the cursor, for this heading only, for the life of
    /// the list.
    pub fn unfold(&mut self) {
        self.unfold_at(self.cursor);
    }

    /// Open the fold on line `at`, as a click does. The cursor keeps what it
    /// was on.
    pub fn unfold_at(&mut self, at: usize) {
        let key = match self.items.get(at).copied() {
            Some(Item::Fold(under, _)) => self.key(under),
            // A subtree fold opens the heading its parent is drawn under.
            Some(Item::Sub(n, _)) => self.key_of_row(n),
            _ => None,
        };
        let Some(key) = key else {
            return;
        };
        self.unfolded.insert(key);
        let on = self.on();
        self.rebuild(on.agent());
        self.follow(&on);
    }

    /// The [`Key`] of the heading this agent is drawn under.
    fn key_of_row(&self, n: usize) -> Option<Key> {
        let anchor = self.anchor_of(n);
        match self.axis {
            Axis::State => Some(Key::Group(self.family(anchor))),
            Axis::Project | Axis::Repo => Some(Key::Project(self.root_of(anchor))),
        }
    }

    /// Agents stopped on a question, counted by phase, so pinned and sleeping
    /// ones count too.
    pub fn waiting(&self) -> usize {
        self.waiting
    }

    /// Top-level agents per non-empty group, whatever the axis.
    ///
    /// Computed once per rebuild, since the header and the terminal title read
    /// it on every frame.
    pub fn counts(&self) -> &[(Group, usize)] {
        &self.counts
    }

    /// Whether the list has no lines. A narrowing can empty it while agents
    /// exist.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Agents holding a slot against the spawn cap, ignoring any narrowing.
    ///
    /// Matches the spawn gate without asking tmux: a gone pane already reads
    /// as a terminal phase, and a parked agent keeps its phase but has
    /// `Evidence::LetGo`, so the evidence excludes it.
    pub fn live(&self) -> usize {
        self.views
            .iter()
            .filter(|view| !view.phase().is_terminal() && view.verdict.evidence != Evidence::LetGo)
            .count()
    }

    pub fn down(&mut self) {
        self.step(1);
    }

    pub fn up(&mut self) {
        self.step(-1);
    }

    /// Move to the first line, skipping a blank the way [`Self::step`] does.
    pub fn top(&mut self) {
        self.cursor = 0;
        if matches!(self.items.first(), Some(Item::Blank)) {
            self.step(1);
        }
    }

    /// Move to the last line, skipping a trailing blank.
    pub fn bottom(&mut self) {
        self.cursor = self.items.len().saturating_sub(1);
        if matches!(self.items.last(), Some(Item::Blank)) {
            self.step(-1);
        }
    }

    /// Put the cursor on line `at`, as a click does. Returns false for a blank
    /// or out-of-range line.
    pub fn land(&mut self, at: usize) -> bool {
        match self.items.get(at) {
            Some(Item::Blank) | None => false,
            Some(_) => {
                self.cursor = at;
                true
            }
        }
    }

    /// Put the cursor on the agent with this id. Returns false when it has no
    /// line, e.g. it is narrowed out or not in the reading yet.
    pub fn land_on(&mut self, id: &str) -> bool {
        let Some(at) = self.row_of(id) else {
            return false;
        };
        self.cursor = at;
        true
    }

    /// The id of the first drawn agent that needs the user, per
    /// [`needing_you`], judged by each agent's own group whatever the axis.
    ///
    /// Only drawn agents are considered, so [`List::land_on`] always succeeds
    /// on the answer.
    pub fn first_needing(&self) -> Option<String> {
        let showing: Vec<(Group, String)> = self
            .ordered()
            .into_iter()
            .map(|n| (self.group(n), self.views[n].id().to_string()))
            .filter(|(_, id)| self.drawn(id))
            .collect();
        needing_you(&showing)
    }

    fn row_of(&self, id: &str) -> Option<usize> {
        self.items
            .iter()
            .position(|item| self.agent(*item).is_some_and(|view| view.id() == id))
    }

    /// Move `by` lines, skipping blanks and stopping at the ends.
    fn step(&mut self, by: isize) {
        let mut at = self.cursor;
        loop {
            let Some(next) = at.checked_add_signed(by) else {
                return;
            };
            if next >= self.items.len() {
                return;
            }
            at = next;
            if !matches!(self.items[at], Item::Blank) {
                self.cursor = at;
                return;
            }
        }
    }

    /// Rebuild the lines from the current views.
    ///
    /// `keeping` is the id the cursor was on, passed in because some callers
    /// have already replaced `views`, so the old items no longer index them.
    fn rebuild(&mut self, keeping: Option<&str>) {
        self.remember_the_roots();
        // Plant the forest twice. Narrowing needs `family`, which needs the
        // whole fleet's parent links. The lines are drawn from the second
        // planting, where a child of a narrowed-out parent is a root.
        let fleet: Vec<usize> = (0..self.views.len()).collect();
        self.plant(&fleet);
        let order = self.ordered();
        self.plant(&order);
        self.counts = self.counted(&order);
        self.waiting = order
            .iter()
            .filter(|&&n| self.views[n].phase() == Phase::Waiting)
            .count();
        match self.axis {
            Axis::State => {
                self.projects.clear();
                self.items = self.by_state(&order, keeping);
            }
            Axis::Project | Axis::Repo => {
                let (projects, items) = self.by_project(&order, keeping);
                self.projects = projects;
                self.items = items;
            }
        }
        self.settle();
    }

    /// Resolve the project root of every agent not yet cached, on a path axis
    /// only, since resolving probes the disk or runs git.
    fn remember_the_roots(&mut self) {
        let axis = self.axis;
        if axis == Axis::State {
            return;
        }
        let (probe, repo_of) = (self.probe, self.repo_of);
        // Rebuilt from this reading so agents that left are dropped.
        let mut known = std::mem::take(&mut self.roots);
        self.roots = self
            .views
            .iter()
            .map(|view| {
                known.remove_entry(view.id()).unwrap_or_else(|| {
                    (
                        view.id().to_string(),
                        root_of(axis, &view.meta, probe, repo_of),
                    )
                })
            })
            .collect();
        if axis == Axis::Repo {
            self.remember_the_branches();
        }
    }

    /// Look up the checked-out branch of every root not yet cached. Keyed by
    /// root, so many agents in one repository cost one lookup. Roots no agent
    /// is under any more are dropped.
    fn remember_the_branches(&mut self) {
        let branch_at = self.branch_at;
        let mut known = std::mem::take(&mut self.branches);
        for root in self.roots.values() {
            if self.branches.contains_key(root) {
                continue;
            }
            let (root, branch) = known
                .remove_entry(root)
                .unwrap_or_else(|| (root.clone(), branch_at(root)));
            self.branches.insert(root, branch);
        }
    }

    /// Build `parents`, `children` and `tops` for the agents in `order`.
    ///
    /// A parent link to an agent outside `order`, to itself, or one that closes
    /// a loop is dropped, and the child becomes a root. Loops are cut rather
    /// than walked so a hand-edited record cannot hang the view.
    fn plant(&mut self, order: &[usize]) {
        let kept: HashSet<usize> = order.iter().copied().collect();
        let at: HashMap<&str, usize> = self
            .views
            .iter()
            .enumerate()
            .map(|(n, view)| (view.id(), n))
            .collect();
        self.parents = vec![None; self.views.len()];
        self.children = vec![Vec::new(); self.views.len()];
        for &n in order {
            let Some(id) = self.views[n].meta.parent.as_deref() else {
                continue;
            };
            let Some(&parent) = at.get(id) else {
                continue;
            };
            if parent != n && kept.contains(&parent) {
                self.parents[n] = Some(parent);
            }
        }
        for &n in order {
            let mut seen = HashSet::new();
            let mut here = n;
            while let Some(parent) = self.parents[here] {
                if !seen.insert(here) {
                    self.parents[here] = None;
                    break;
                }
                here = parent;
            }
        }
        for &n in order {
            if let Some(parent) = self.parents[n] {
                self.children[parent].push(n);
            }
        }
        // Children sort newest first by creation, not by group, so the
        // closing connector lands on the oldest.
        let created: Vec<u64> = self.views.iter().map(|view| view.meta.created).collect();
        for children in &mut self.children {
            children.sort_by(|&a, &b| created[b].cmp(&created[a]));
        }
        self.tops = order
            .iter()
            .copied()
            .filter(|&n| self.parents[n].is_none())
            .collect();
    }

    /// The row's depth in the planted forest. The record's own `depth` is not
    /// used, since a child whose parent is gone is drawn as a root.
    fn depth_of_row(&self, n: usize) -> usize {
        let mut depth = 0;
        let mut here = n;
        while let Some(parent) = self.parents[here] {
            depth += 1;
            here = parent;
        }
        depth
    }

    /// These roots and all their descendants, depth first, in drawing order.
    fn nested(&self, roots: &[usize]) -> Vec<usize> {
        let mut out = Vec::new();
        for &n in roots {
            self.push_subtree(n, &mut out);
        }
        out
    }

    fn push_subtree(&self, n: usize, out: &mut Vec<usize>) {
        out.push(n);
        for &child in &self.children[n] {
            self.push_subtree(child, out);
        }
    }

    /// Whether the row is its parent's last child, which draws `└─` rather
    /// than `├─`.
    fn last_sibling(&self, n: usize) -> bool {
        match self.parents[n] {
            Some(parent) => self.children[parent].last() == Some(&n),
            None => self.tops.last() == Some(&n),
        }
    }

    /// Nesting levels every root is padded by: always 0.
    ///
    /// A child's connector starts in its parent's glyph column, so roots keep
    /// their column when a family appears.
    pub fn root_pad(&self) -> usize {
        0
    }

    /// The nesting depth of an agent line, 0 for anything else. The name
    /// column gives up two cells per level so later columns stay aligned.
    pub(super) fn depth(&self, item: Item) -> usize {
        match item {
            Item::Agent(n) => self.depth_of_row(n),
            _ => 0,
        }
    }

    /// The tree connectors before an agent's glyph, two cells per level.
    ///
    /// Empty for a root. The last level is `├─`, or `└─` for a last child.
    /// Each level above it is `│ ` where that ancestor has a later sibling,
    /// else blank.
    pub fn gutter(&self, item: Item) -> String {
        let n = match item {
            Item::Agent(n) => n,
            // A subtree fold sits where a child's glyph would, with no
            // connector.
            Item::Sub(n, _) => {
                return "  ".repeat(self.depth_of_row(n) + 1);
            }
            _ => return String::new(),
        };
        let depth = self.depth_of_row(n);
        let mut cells = String::new();
        if depth == 0 {
            return cells;
        }
        // Root-to-row path, so each level can ask its ancestor.
        let mut path = vec![n];
        let mut here = n;
        while let Some(parent) = self.parents[here] {
            path.push(parent);
            here = parent;
        }
        path.reverse();
        for level in 0..depth {
            if level + 1 == depth {
                cells.push_str(match self.last_sibling(n) {
                    true => "└─",
                    false => "├─",
                });
            } else {
                let ancestor = path[level + 1];
                cells.push_str(match self.last_sibling(ancestor) {
                    true => "  ",
                    false => "│ ",
                });
            }
        }
        cells
    }

    /// The agents the narrowing keeps, in list order: by group, then the
    /// group's hand-made order, then start order, except Completed, newest
    /// ending first.
    ///
    /// Every axis uses this one order, so turning the axis never changes a
    /// row's neighbours.
    fn ordered(&self) -> Vec<usize> {
        let mut order: Vec<usize> = (0..self.views.len()).filter(|&n| self.keeps(n)).collect();
        // Keyed once per agent: `family` walks a subtree and `seat` scans the
        // group's order, too much to repeat on every comparison.
        order.sort_by_cached_key(|&n| {
            let view = &self.views[n];
            let ending = match self.family(n) {
                Group::Completed => Some((Reverse(ended(view)), view.id())),
                Group::Shelved => self.shelved_at(n).map(|at| (Reverse(at), view.id())),
                // The sort is stable, so other groups keep reading order,
                // which is start order.
                _ => None,
            };
            (self.rank(n), self.seat(n), ending)
        });
        order
    }

    /// Top-level agents per non-empty group. Children are left out because
    /// they are drawn under their parent's heading.
    fn counted(&self, order: &[usize]) -> Vec<(Group, usize)> {
        Group::ALL
            .into_iter()
            .filter_map(|group| {
                let count = order
                    .iter()
                    .filter(|&&n| self.parents[n].is_none() && self.family(n) == group)
                    .count();
                (count > 0).then_some((group, count))
            })
            .collect()
    }

    fn tops_in(&self, order: &[usize]) -> Vec<usize> {
        order
            .iter()
            .copied()
            .filter(|&n| self.parents[n].is_none())
            .collect()
    }

    /// The lines for the state axis: one heading per non-empty group.
    fn by_state(&self, order: &[usize], keeping: Option<&str>) -> Vec<Item> {
        let mut items = Vec::new();
        for group in Group::ALL {
            let roots: Vec<usize> = order
                .iter()
                .copied()
                .filter(|&n| self.parents[n].is_none() && self.family(n) == group)
                .collect();
            if roots.is_empty() {
                continue;
            }
            let members = self.nested(&roots);
            // A narrowing reaches into the shelf, or it could find nothing
            // there to show.
            let shut = self.shut.contains(&Key::Group(group))
                && !(group == Group::Shelved && self.filters.label().is_some());
            if !items.is_empty() {
                items.push(Item::Blank);
            }
            let under = Under::Group(group);
            items.push(Item::Heading(under, self.tally(&roots, shut)));
            if shut {
                continue;
            }
            self.tree(
                under,
                &Key::Group(group),
                &roots,
                &members,
                keeping,
                &mut items,
            );
        }
        items
    }

    /// Push a heading's rows: each top-level agent and its subtree, folding
    /// past [`FOLD_AT`] rows unless the heading was unfolded.
    ///
    /// Takes both `under` and `key` because `projects` is still being built,
    /// so [`Self::key`] would read the previous reading's table.
    fn tree(
        &self,
        under: Under,
        key: &Key,
        roots: &[usize],
        members: &[usize],
        keeping: Option<&str>,
        items: &mut Vec<Item>,
    ) {
        let shown: HashSet<usize> = match members.len() > FOLD_AT && !self.unfolded.contains(key) {
            true => self
                .worth_the_room(members, FOLD_AT, keeping)
                .into_iter()
                .collect(),
            false => members.iter().copied().collect(),
        };
        for &root in roots {
            if shown.contains(&root) {
                self.subtree_rows(root, &shown, items);
            }
        }
        // Hidden top-level agents, counted with their whole families. Children
        // hidden under a drawn parent are counted by that parent's `Sub` line.
        let more: usize = roots
            .iter()
            .filter(|root| !shown.contains(root))
            .map(|&root| 1 + self.descendants(root))
            .sum();
        if more > 0 {
            items.push(Item::Fold(under, more));
        }
    }

    /// Push a row, its kept descendants, and a `Sub` line for any it hides.
    fn subtree_rows(&self, n: usize, shown: &HashSet<usize>, items: &mut Vec<Item>) {
        items.push(Item::Agent(n));
        for &child in &self.children[n] {
            if shown.contains(&child) {
                self.subtree_rows(child, shown, items);
            }
        }
        let hidden = self.hidden_under(n, shown);
        if hidden > 0 {
            items.push(Item::Sub(n, hidden));
        }
    }

    fn hidden_under(&self, n: usize, shown: &HashSet<usize>) -> usize {
        self.children[n]
            .iter()
            .map(|&child| match shown.contains(&child) {
                true => self.hidden_under(child, shown),
                false => 1 + self.descendants(child),
            })
            .sum()
    }

    fn descendants(&self, n: usize) -> usize {
        self.children[n]
            .iter()
            .map(|&child| 1 + self.descendants(child))
            .sum()
    }

    /// The rows a folded heading keeps, in `members` order.
    ///
    /// Priority: the cursor's row (even past `room`, so the cursor and any
    /// open card stay put), then failures and rows with a pull request, then
    /// the rest. A kept row's ancestors are kept too, even past `room`.
    ///
    /// Read state is deliberately ignored: opening a card marks its row read,
    /// and a fold that used it reshuffled the group under the cursor.
    fn worth_the_room(&self, members: &[usize], room: usize, keeping: Option<&str>) -> Vec<usize> {
        let cursor = |n: usize| keeping == Some(self.views[n].id());
        let scanned = |n: usize| {
            self.views[n].phase() == Phase::Failed || !self.requests(&self.views[n]).is_empty()
        };
        let room = room.max(members.iter().filter(|&&n| cursor(n)).count());
        let mut chosen: HashSet<usize> = members
            .iter()
            .copied()
            .filter(|&n| cursor(n))
            .chain(
                members
                    .iter()
                    .copied()
                    .filter(|&n| !cursor(n) && scanned(n)),
            )
            .chain(
                members
                    .iter()
                    .copied()
                    .filter(|&n| !cursor(n) && !scanned(n)),
            )
            .take(room)
            .collect();
        for &n in &chosen.clone() {
            let mut here = n;
            while let Some(parent) = self.parents[here] {
                chosen.insert(parent);
                here = parent;
            }
        }
        members
            .iter()
            .copied()
            .filter(|n| chosen.contains(n))
            .collect()
    }

    /// The [`Tally`] for a heading over these roots.
    ///
    /// Failures are counted whether the heading is open or shut, and a failed
    /// child counts under the heading its parent is drawn under.
    fn tally(&self, roots: &[usize], shut: bool) -> Tally {
        let under = self.nested(roots);
        let mut states = [0; Group::ALL.len()];
        for &n in &under {
            let group = self.group(n);
            if let Some(at) = Group::ALL.iter().position(|other| *other == group) {
                states[at] += 1;
            }
        }
        Tally {
            members: roots.len(),
            failures: under
                .iter()
                .filter(|&&n| self.views[n].phase() == Phase::Failed)
                .count(),
            states,
            shut,
        }
    }

    /// The lines for a path axis: one heading per project, and the project
    /// table the headings index.
    ///
    /// Projects sort by their most urgent agent's group, then by path. A
    /// project past [`FOLD_AT`] rows folds like a group.
    fn by_project(&self, order: &[usize], keeping: Option<&str>) -> (Vec<PathBuf>, Vec<Item>) {
        let mut projects: Vec<(PathBuf, Vec<usize>)> = Vec::new();
        for &n in &self.tops_in(order) {
            let root = self.root_of(n);
            match projects.iter_mut().find(|(at, _)| at == &root) {
                Some((_, members)) => members.push(n),
                None => projects.push((root, vec![n])),
            }
        }

        // `order` is list order, so a project's first agent is its most urgent.
        projects.sort_by(|(here, ours), (there, theirs)| {
            self.rank(ours[0])
                .cmp(&self.rank(theirs[0]))
                .then_with(|| here.cmp(there))
        });

        let mut roots = Vec::new();
        let mut items = Vec::new();
        for (root, tops) in projects {
            let key = Key::Project(root.clone());
            let shut = self.shut.contains(&key);
            let members = self.nested(&tops);
            if !items.is_empty() {
                items.push(Item::Blank);
            }
            let under = Under::Project(roots.len());
            items.push(Item::Heading(under, self.tally(&tops, shut)));
            if !shut {
                self.tree(under, &key, &tops, &members, keeping, &mut items);
            }
            roots.push(root);
        }
        (roots, items)
    }

    /// The project an agent is drawn under: its cached root, else its `dir`.
    fn root_of(&self, n: usize) -> PathBuf {
        self.roots
            .get(self.views[n].id())
            .cloned()
            .unwrap_or_else(|| self.views[n].meta.dir.clone())
    }

    /// The stable [`Key`] for a heading.
    pub fn key(&self, under: Under) -> Option<Key> {
        match under {
            Under::Group(group) => Some(Key::Group(group)),
            Under::Project(n) => self.projects.get(n).cloned().map(Key::Project),
        }
    }

    fn on(&self) -> On {
        self.on_line(self.cursor)
    }

    /// What is on line `at`, by stable identity.
    fn on_line(&self, at: usize) -> On {
        match self.items.get(at) {
            Some(Item::Agent(n)) => match self.views.get(*n) {
                Some(view) => On::Agent(view.id().to_string()),
                None => On::Nothing,
            },
            Some(Item::Heading(under, _)) => match self.key(*under) {
                Some(key) => On::Heading(key),
                None => On::Nothing,
            },
            _ => On::Nothing,
        }
    }

    /// Put the cursor back on what it was on, if that is still drawn.
    fn follow(&mut self, held: &On) {
        let found = match held {
            On::Agent(id) => self.row_of(id),
            On::Heading(key) => self.items.iter().position(|item| match item {
                Item::Heading(under, _) => self.key(*under).as_ref() == Some(key),
                _ => false,
            }),
            On::Nothing => None,
        };
        match (found, held) {
            (Some(at), _) => self.cursor = at,
            // The user removed it, and meant to: no key was aimed past it.
            (None, On::Agent(id)) if self.let_go.as_ref() == Some(id) && !self.items.is_empty() => {
                let at = self.cursor.min(self.items.len() - 1);
                let agent = |item: &Item| matches!(item, Item::Agent(_));
                let next = self.items[at..].iter().position(agent).map(|n| at + n);
                if let Some(to) = next.or_else(|| self.items[..at].iter().rposition(agent)) {
                    self.cursor = to;
                }
            }
            // The agent left the list. Its old line now holds another agent,
            // which a key meant for the gone one would hit, so move to the
            // heading above instead. No single key on a heading is destructive.
            (None, On::Agent(_)) if !self.items.is_empty() => {
                let over = self.items[..=self.cursor.min(self.items.len() - 1)]
                    .iter()
                    .rposition(|item| matches!(item, Item::Heading(..)))
                    .or_else(|| {
                        self.items
                            .iter()
                            .position(|item| matches!(item, Item::Heading(..)))
                    });
                if let Some(at) = over {
                    self.cursor = at;
                }
            }
            _ => {}
        }
    }

    /// Clamp the cursor to a valid, non-blank line.
    fn settle(&mut self) {
        if self.items.is_empty() {
            self.cursor = 0;
            // The next non-empty rebuild places the cursor as if newly opened.
            self.landed = false;
            return;
        }
        if !self.landed {
            self.landed = true;
            // The first placement is on the first agent, not its heading.
            self.cursor = self
                .items
                .iter()
                .position(|item| matches!(item, Item::Agent(_)))
                .unwrap_or(0);
            return;
        }
        self.cursor = self.cursor.min(self.items.len() - 1);
        // A blank always sits above a heading, so step onto the heading.
        if matches!(self.items[self.cursor], Item::Blank) {
            self.cursor = (self.cursor + 1).min(self.items.len() - 1);
        }
    }
}

/// Every agent as `(group, id)` in list order, for verbs such as `attach`.
///
/// Builds a [`List`] with the saved arrangement, always on the state axis,
/// with no narrowing and no folding. Pull requests come from what was last
/// written down ([`pr::written`]), since a verb exits before a forge answers.
/// The rules judge idleness at `now`, so the verb sees the shelf the view
/// draws.
pub fn wall_order(views: &[View], arrangement: &Arrangement, now: u64) -> Vec<(Group, String)> {
    let mut list = List {
        asks: pr::written,
        now,
        ..List::default()
    };
    list.arrange(Arrangement {
        axis: Axis::State,
        ..arrangement.clone()
    });
    list.show(views.to_vec());
    list.ordered()
        .into_iter()
        .map(|n| (list.group(n), list.views[n].id().to_string()))
        .collect()
}

/// The first agent needing the user in a list-ordered `(group, id)` slice:
/// the first NeedsInput, else the first Review, else the first Completed.
///
/// Shared by the view's key and `amx attach --waiting` so the two agree.
pub fn needing_you(order: &[(Group, String)]) -> Option<String> {
    let first_of = |group: Group| {
        order
            .iter()
            .find(|(on, _)| *on == group)
            .map(|(_, id)| id.clone())
    };
    first_of(Group::NeedsInput)
        .or_else(|| first_of(Group::Review))
        .or_else(|| first_of(Group::Completed))
}

/// Case-insensitive substring match. Tasks are free text with capitals.
fn holds(said: &str, want: &str) -> bool {
    said.to_lowercase().contains(&want.to_lowercase())
}

/// The name a row shows: the user's rename, else the vendor's session title,
/// else the id.
///
/// The session title beats the id because the vendor keeps it current as the
/// work moves. The id stays reachable through `ls`, every verb, and find.
pub fn called(view: &View) -> &str {
    view.state
        .name
        .as_deref()
        .or(view.state.session_title.as_deref())
        .unwrap_or_else(|| view.id())
}

/// The vendor column text: program, model and effort, space separated.
///
/// Only parts the record holds are shown. An unset dial is the vendor's
/// default, which amx does not know, so it is left out rather than guessed.
/// `sh` for a command row (`!cmd` or `--exec`), which has no vendor.
pub fn vendor_words(meta: &Meta) -> String {
    let Some(agent) = meta.agent.as_deref() else {
        return "sh".to_string();
    };
    [
        Some(crate::registry::program(agent)),
        meta.model.as_deref(),
        meta.effort.as_deref(),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<&str>>()
    .join(" ")
}

/// When the agent was last heard from.
fn said(view: &View) -> u64 {
    view.state.last_event.max(view.state.since)
}

/// When the run ended: the recorded end, else the last event. Records from
/// older amx versions, or whose pane vanished, have no end stamp.
fn ended(view: &View) -> u64 {
    match view.state.ended {
        0 => said(view),
        at => at,
    }
}

/// The question an agent is showing, and its position in the call.
///
/// Read from the recorded payload because the pane cannot supply it: as of
/// claude 2.1.240 the `AskUserQuestion` tab strip elides headers as the pane
/// narrows (at 24 columns the tab name is only an ellipsis). The question
/// count, headers and multi-select flag exist only in the payload
/// (`docs/question-shapes.md`).
#[derive(Clone, Copy)]
pub struct Showing<'a> {
    pub ask: &'a Ask,
    /// 1-based index of this question in the call.
    pub at: usize,
    /// Number of questions in the call.
    pub of: usize,
}

impl Showing<'_> {
    /// The tab header, if the payload has a non-empty one.
    pub fn header(&self) -> Option<&str> {
        self.ask.header.as_deref().filter(|word| !word.is_empty())
    }
}

/// The question the agent is stopped on, if its call was recorded.
///
/// That is the first unanswered question, the same rule the record uses:
/// answering one question does not end the call, so the vendor moves to the
/// next tab.
pub fn showing(view: &View) -> Option<Showing<'_>> {
    let at = view
        .state
        .asking
        .iter()
        .position(|ask| ask.answer.is_none())?;
    Some(Showing {
        ask: &view.state.asking[at],
        at: at + 1,
        of: view.state.asking.len(),
    })
}

/// Whether a pull request is open for review: not merged, closed or draft,
/// whatever its checks say.
fn asking(standing: Standing) -> bool {
    match standing {
        Standing::Merged | Standing::Closed | Standing::Draft => false,
        Standing::Open
        | Standing::Ready
        | Standing::Running
        | Standing::Changes
        | Standing::Failing => true,
    }
}

/// The project an agent runs in, for the project axis.
fn project_of(meta: &Meta, probe: fn(&Path) -> bool) -> PathBuf {
    // An amx worktree names its repository by its path alone. Any other
    // worktree path (moved, or a hand-edited record) groups by `dir`.
    if let Some(tree) = &meta.worktree {
        return crate::worktree::repo_of(tree).unwrap_or_else(|| meta.dir.clone());
    }

    // `dir` is often a subdirectory, so walk up to the repository top, or
    // `<repo>/src` and a worktree of `<repo>` would head separate projects.
    meta.dir
        .ancestors()
        // A relative path ends at "", which would probe the view's own cwd.
        .filter(|dir| !dir.as_os_str().is_empty())
        .find(|dir| probe(dir))
        .map(Path::to_path_buf)
        .unwrap_or_else(|| meta.dir.clone())
}

/// The repository an agent belongs to, for the repo axis.
///
/// Asks git for the shared repository, so any linked worktree (including
/// ones `workflow run` cut) groups with its main checkout. Falls back to
/// [`project_of`] when git cannot place the directory.
fn repo_root_of(
    meta: &Meta,
    probe: fn(&Path) -> bool,
    repo_of: fn(&Path) -> Option<PathBuf>,
) -> PathBuf {
    let dir = meta.worktree.as_deref().unwrap_or(&meta.dir);
    repo_of(dir).unwrap_or_else(|| project_of(meta, probe))
}

/// The heading root for an agent on this path axis.
fn root_of(
    axis: Axis,
    meta: &Meta,
    probe: fn(&Path) -> bool,
    repo_of: fn(&Path) -> Option<PathBuf>,
) -> PathBuf {
    match axis {
        Axis::Repo => repo_root_of(meta, probe, repo_of),
        _ => project_of(meta, probe),
    }
}

fn main_repo_of(dir: &Path) -> Option<PathBuf> {
    crate::worktree::main_repo(dir).ok()
}

/// Whether `dir` has a `.git` entry. A linked worktree's `.git` is a file.
fn holds_a_repository(dir: &Path) -> bool {
    dir.join(".git").exists()
}

/// `path` with `home` abbreviated to `~`. Shared with the header and the
/// composer so every surface writes paths the same way.
pub(super) fn shorten(path: &Path, home: Option<&Path>) -> String {
    let under = home
        .filter(|home| !home.as_os_str().is_empty())
        .and_then(|home| path.strip_prefix(home).ok());
    match under {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::derive::{Evidence, Verdict};
    use crate::pr::Standing;
    use crate::store::{Meta, State};
    use crate::tmux::{PaneId, Socket};
    use std::path::PathBuf;
    use tempfile::TempDir;

    /// A view of one agent in `phase`, created and last heard from at `at`.
    fn view(id: &str, phase: Phase, at: u64) -> View {
        View {
            meta: Meta {
                role: None,
                parent: None,
                depth: 0,
                id: id.to_string(),
                task: "fix the login bug".to_string(),
                agent: None,
                model: None,
                effort: None,
                dir: PathBuf::from("/srv/app"),
                worktree: None,
                branch: None,
                base: None,
                socket: Socket::Name("amx".to_string()),
                pane: PaneId::new("%1").unwrap(),
                bg: false,
                session: None,
                transcript: None,
                created: at,
            },
            state: State {
                state: phase,
                since: at,
                last_event: at,
                ..State::default()
            },
            verdict: Verdict {
                phase,
                evidence: Evidence::Hooks,
                rule: None,
                age: 1,
                worked: 1,
            },
            doing: None,
        }
    }

    fn at(mut view: View, dir: &str) -> View {
        view.meta.dir = PathBuf::from(dir);
        view
    }

    fn on_a_branch(mut view: View, branch: &str) -> View {
        view.meta.branch = Some(branch.to_string());
        view
    }

    fn child_of(mut view: View, parent: &str) -> View {
        view.meta.parent = Some(parent.to_string());
        view.meta.depth = 1;
        view
    }

    /// A fake forge: two branches with an open pull request, one merged and
    /// one draft.
    fn a_forge(meta: &Meta) -> Vec<Pr> {
        match meta.branch.as_deref() {
            Some("amx/fix-login-a1b") => vec![Pr {
                number: 12,
                standing: Standing::Failing,
            }],
            Some("amx/port-importer-b2c") => vec![Pr {
                number: 3,
                standing: Standing::Ready,
            }],
            Some("amx/merged-f6g") => vec![Pr {
                number: 7,
                standing: Standing::Merged,
            }],
            Some("amx/draft-g7h") => vec![Pr {
                number: 8,
                standing: Standing::Draft,
            }],
            _ => Vec::new(),
        }
    }

    /// Pin `id` by walking the cursor to it and pressing the key.
    fn pinning(mut list: List, id: &str) -> List {
        list.top();
        while list.selected().is_none_or(|view| view.id() != id) {
            let at = list.cursor();
            list.down();
            assert_ne!(list.cursor(), at, "no row for {id} to put the cursor on");
        }
        assert!(list.hold_or_let_go());
        list
    }

    /// Walk the cursor to `id`.
    fn to(list: &mut List, id: &str) {
        list.top();
        while list.selected().is_none_or(|view| view.id() != id) {
            let at = list.cursor();
            list.down();
            assert_ne!(list.cursor(), at, "no row for {id} to put the cursor on");
        }
    }

    /// Walk the cursor to the heading for `group`.
    fn to_heading(list: &mut List, group: Group) {
        let (at, _) = list.heading_at(&Key::Group(group)).expect("a heading");
        assert!(list.land(at));
    }

    /// Put `id` to sleep by walking the cursor to it and pressing the key.
    fn sleeping(mut list: List, id: &str) -> List {
        list.top();
        while list.selected().is_none_or(|view| view.id() != id) {
            let at = list.cursor();
            list.down();
            assert_ne!(list.cursor(), at, "no row for {id} to put the cursor on");
        }
        assert!(list.sleep_or_wake());
        list
    }

    fn over_the_forge(views: Vec<View>) -> List {
        let mut list = List::default();
        list.asking(a_forge);
        list.show(views);
        list
    }

    fn in_a_worktree(mut view: View, tree: &str) -> View {
        view.meta.dir = PathBuf::from(tree);
        view.meta.worktree = Some(PathBuf::from(tree));
        view
    }

    /// The list as text lines: headings with member counts, agents by id
    /// with their gutter, folds, and blanks.
    fn lines(list: &List) -> Vec<String> {
        list.items()
            .iter()
            .map(|item| match item {
                Item::Heading(under, tally) => format!(
                    "{} ({}){}",
                    list.title(*under),
                    tally.members,
                    if tally.shut { " shut" } else { "" }
                ),
                Item::Agent(_) => {
                    format!("{}{}", list.gutter(*item), list.agent(*item).unwrap().id())
                }
                Item::Fold(_, hidden) => format!("… {hidden} more"),
                Item::Sub(n, hidden) => {
                    format!("{}… {hidden} sub", list.gutter(Item::Sub(*n, *hidden)))
                }
                Item::Blank => String::new(),
            })
            .collect()
    }

    /// Each agent's gutter, sorted by id.
    fn gutters(list: &List) -> Vec<(String, String)> {
        let mut cells: Vec<(String, String)> = list
            .items()
            .iter()
            .filter(|item| matches!(item, Item::Agent(_)))
            .map(|item| {
                (
                    list.agent(*item).unwrap().id().to_string(),
                    list.gutter(*item),
                )
            })
            .collect();
        cells.sort();
        cells
    }

    fn listed(views: Vec<View>) -> List {
        let mut list = List::default();
        list.show(views);
        list
    }

    /// A `Tally::states` array built from named group counts.
    fn states(counts: &[(Group, usize)]) -> [usize; Group::ALL.len()] {
        let mut states = [0; Group::ALL.len()];
        for (group, count) in counts {
            let at = Group::ALL
                .iter()
                .position(|other| other == group)
                .expect("a group in the table");
            states[at] = *count;
        }
        states
    }

    /// [`wall_order`] output as `"<group title> <id>"` lines.
    fn walled(order: &[(Group, String)]) -> Vec<String> {
        order
            .iter()
            .map(|(group, id)| format!("{} {id}", group.title()))
            .collect()
    }

    /// `count` done agents, `done-0` ending first and the last ending newest.
    fn a_history(count: usize) -> Vec<View> {
        (0..count)
            .map(|n| view(&format!("done-{n}"), Phase::Done, 10 * n as u64))
            .collect()
    }

    // Every directory probed, in order. Thread-local because tests run in
    // parallel.
    thread_local! {
        static ASKED: std::cell::RefCell<Vec<PathBuf>> = const { std::cell::RefCell::new(Vec::new()) };
    }

    /// A fake probe where `/src/api` and `/src/web` are repositories. Records
    /// each call in `ASKED` so tests can count disk access.
    fn a_disk_with_repos(dir: &Path) -> bool {
        ASKED.with_borrow_mut(|asked| asked.push(dir.to_path_buf()));
        dir == Path::new("/src/api") || dir == Path::new("/src/web")
    }

    fn asked() -> Vec<PathBuf> {
        ASKED.with_borrow(|asked| asked.clone())
    }

    /// A list on the project axis over `a_disk_with_repos`, home `/home/dev`.
    fn over_the_disk(views: Vec<View>) -> List {
        ASKED.with_borrow_mut(|asked| asked.clear());
        let mut list = List::probing(a_disk_with_repos, Some(PathBuf::from("/home/dev")));
        list.turn();
        list.show(views);
        list
    }

    /// A fake git: everything under `/work/repo` belongs to it, nothing else
    /// belongs to any repository.
    fn a_disk_that_answers_git(dir: &Path) -> Option<PathBuf> {
        dir.starts_with("/work/repo")
            .then(|| PathBuf::from("/work/repo"))
    }

    /// A fake git: `/work/repo` is on `main`, other roots on no branch.
    fn a_branch_at(root: &Path) -> Option<String> {
        (root == Path::new("/work/repo")).then(|| "main".to_string())
    }

    /// A list on the repo axis (three turns) over the fake git.
    fn over_the_repos(views: Vec<View>) -> List {
        let mut list = List::probing_repos(
            a_disk_that_answers_git,
            a_branch_at,
            Some(PathBuf::from("/home/dev")),
        );
        list.turn();
        list.turn();
        list.turn();
        list.show(views);
        list
    }

    #[test]
    fn view_gathers_agents_under_what_they_need() {
        let list = listed(vec![
            view("busy-a1b", Phase::Working, 10),
            view("done-b2c", Phase::Done, 20),
            view("ask-c3d", Phase::Waiting, 30),
            view("idle-d4e", Phase::Idle, 40),
            view("starting-e5f", Phase::Starting, 50),
        ]);

        assert_eq!(
            lines(&list),
            [
                "Needs input (1)",
                "ask-c3d",
                "",
                "Working (2)",
                "busy-a1b",
                "starting-e5f",
                "",
                "Completed (2)",
                "idle-d4e",
                "done-b2c",
            ],
            "and inside a group, the order they were started in"
        );
        assert_eq!(
            list.counts(),
            [
                (Group::NeedsInput, 1),
                (Group::Working, 2),
                (Group::Completed, 2)
            ]
        );
    }

    #[test]
    fn view_counts_the_fleet_once_for_however_many_frames_read_it() {
        // The header and the terminal title read the counts on every frame, so
        // they are computed once per rebuild and read back after that.
        let mut list = listed(vec![
            view("ask-a1b", Phase::Waiting, 10),
            view("busy-b2c", Phase::Working, 20),
            view("busy-c3d", Phase::Working, 30),
            view("done-d4e", Phase::Done, 40),
        ]);
        assert_eq!(
            list.counts(),
            [
                (Group::NeedsInput, 1),
                (Group::Working, 2),
                (Group::Completed, 1)
            ]
        );
        assert!(
            std::ptr::eq(list.counts(), list.counts()),
            "the second look is the first answer rather than a second walk"
        );

        // The counts follow the narrowing and each new reading.
        list.narrow(vec![Narrow::State(Some("working".to_string()))]);
        assert_eq!(list.counts(), [(Group::Working, 2)]);
        list.narrow(vec![Narrow::State(None)]);
        list.show(vec![view("ask-a1b", Phase::Waiting, 10)]);
        assert_eq!(list.counts(), [(Group::NeedsInput, 1)]);

        // The counts do not depend on the axis.
        let gathered = over_the_disk(vec![
            at(view("ask-a1b", Phase::Waiting, 10), "/src/api"),
            at(view("busy-b2c", Phase::Working, 20), "/src/web"),
        ]);
        assert_eq!(
            gathered.counts(),
            [(Group::NeedsInput, 1), (Group::Working, 1)]
        );
    }

    #[test]
    fn view_puts_an_agent_it_cannot_account_for_among_the_turns_that_are_over() {
        // `unknown` claims nothing is happening, so it is not grouped as working.
        let list = listed(vec![view("puzzling-a1b", Phase::Unknown, 10)]);
        assert_eq!(lines(&list), ["Completed (1)", "puzzling-a1b"]);
    }

    #[test]
    fn view_puts_an_ended_turn_whose_request_is_open_in_front_of_a_reviewer() {
        let list = over_the_forge(vec![
            on_a_branch(view("fix-login-a1b", Phase::Done, 10), "amx/fix-login-a1b"),
            on_a_branch(view("ask-b2c", Phase::Waiting, 20), "amx/port-importer-b2c"),
            on_a_branch(
                view("busy-c3d", Phase::Working, 30),
                "amx/port-importer-b2c",
            ),
            on_a_branch(view("merged-d4e", Phase::Done, 40), "amx/merged-f6g"),
            view("done-e5f", Phase::Done, 50),
        ]);

        assert_eq!(
            lines(&list),
            [
                "Ready for review (1)",
                "fix-login-a1b",
                "",
                "Needs input (1)",
                "ask-b2c",
                "",
                "Working (1)",
                "busy-c3d",
                "",
                "Completed (1)",
                "done-e5f",
                "",
                "Shelved (1) shut",
            ],
            "an agent that is asking or working has something of its own left \
             to do, whatever its branch has open, and a request that was \
             merged is asking nobody for anything: it is shelved"
        );
    }

    #[test]
    fn view_reads_every_ending_as_completed() {
        let list = listed(vec![
            view("done-a1b", Phase::Done, 10),
            view("failed-b2c", Phase::Failed, 20),
            view("stopped-c3d", Phase::Stopped, 30),
        ]);
        assert_eq!(
            lines(&list),
            ["Completed (3)", "stopped-c3d", "failed-b2c", "done-a1b"],
            "newest ending first"
        );
    }

    #[test]
    fn view_draws_a_child_under_its_parent_with_a_connector() {
        // Children are drawn under their parent whatever their own group, newest
        // first, so the oldest gets the closing connector.
        let list = listed(vec![
            view("parent-a1b", Phase::Working, 10),
            child_of(view("scout-b2c", Phase::Done, 20), "parent-a1b"),
            child_of(view("review-c3d", Phase::Done, 30), "parent-a1b"),
        ]);
        assert_eq!(
            lines(&list),
            ["Working (1)", "parent-a1b", "├─review-c3d", "└─scout-b2c"],
            "the children hang from the parent, newest first like the wall's \
             own rows, each connector under the glyph it hangs from"
        );
        assert_eq!(
            list.counts(),
            [(Group::Working, 1)],
            "the heading and the header count the top-level agent once"
        );
    }

    #[test]
    fn view_leaves_a_root_where_it_was_when_a_family_is_drawn() {
        // A root keeps its column when a family appears, and a child's connector
        // starts in the parent's glyph column.
        let alone = listed(vec![view("other-d4e", Phase::Working, 40)]);
        let family = listed(vec![
            view("other-d4e", Phase::Working, 40),
            view("parent-a1b", Phase::Working, 10),
            child_of(view("scout-b2c", Phase::Working, 20), "parent-a1b"),
        ]);
        assert_eq!(
            lines(&alone)[1],
            "other-d4e",
            "a root on a wall with no family stands at the wall's own column"
        );
        assert_eq!(family.root_pad(), 0, "and no root is padded for a family");
        assert_eq!(
            gutters(&family),
            [
                ("other-d4e", String::new()),
                ("parent-a1b", String::new()),
                ("scout-b2c", "└─".to_string()),
            ]
            .map(|(id, cells)| (id.to_string(), cells)),
            "both roots are padded nothing, and the child's connector starts \
             in the column of its parent's glyph"
        );
    }

    #[test]
    fn a_parent_waits_for_its_family_before_it_is_one_of_the_completed() {
        // A done parent with a working child must not sit under Completed, where
        // `c` would clear the family with the running child unseen.
        let working = listed(vec![
            view("parent-a1b", Phase::Done, 10),
            child_of(view("review-b2c", Phase::Working, 20), "parent-a1b"),
        ]);
        assert_eq!(
            lines(&working),
            ["Working (1)", "parent-a1b", "└─review-b2c"],
            "a family with work still in it stands where the work is"
        );
        assert_eq!(working.counts(), [(Group::Working, 1)]);

        // The family takes its most urgent member's group, so a child waiting on a
        // question puts it under Needs input.
        let asking = listed(vec![
            view("parent-a1b", Phase::Done, 10),
            child_of(view("review-b2c", Phase::Waiting, 20), "parent-a1b"),
        ]);
        assert_eq!(
            lines(&asking),
            ["Needs input (1)", "parent-a1b", "└─review-b2c"]
        );

        // Once the child finishes, the family is Completed.
        let done = listed(vec![
            view("parent-a1b", Phase::Done, 10),
            child_of(view("review-b2c", Phase::Done, 20), "parent-a1b"),
        ]);
        assert_eq!(
            lines(&done),
            ["Completed (1)", "parent-a1b", "└─review-b2c"]
        );
    }

    #[test]
    fn view_draws_a_child_of_a_missing_parent_as_a_root() {
        // The parent's record is gone, so the child is a root with no gutter.
        let list = listed(vec![child_of(
            view("scout-b2c", Phase::Working, 20),
            "gone-a1b",
        )]);
        assert_eq!(lines(&list), ["Working (1)", "scout-b2c"]);
    }

    #[test]
    fn view_nests_a_grandchild_under_the_row_it_hangs_from() {
        // Each level indents two cells, and a connector starts under the glyph it
        // hangs from.
        let mut grandchild = child_of(view("scout-b2c", Phase::Working, 20), "parent-a1b");
        grandchild.meta.depth = 2;
        let list = listed(vec![
            view("parent-a1b", Phase::Working, 10),
            child_of(view("helper-f6g", Phase::Done, 30), "parent-a1b"),
            child_of(grandchild, "helper-f6g"),
        ]);
        assert_eq!(
            lines(&list),
            ["Working (1)", "parent-a1b", "└─helper-f6g", "  └─scout-b2c"],
            "the root is padded nothing and its own levels indent each row"
        );
        assert_eq!(
            list.gutter(Item::Sub(0, 1)),
            "  ",
            "a fold stands one level under the parent it holds children back \
             from, at the column their glyphs take"
        );
    }

    #[test]
    fn view_folds_a_family_whole_and_says_how_many_went_with_it() {
        // Children past the fold are counted on a `Sub` line under the parent.
        let mut views = vec![view("parent-a1b", Phase::Working, 10)];
        for n in 0..FOLD_AT + 2 {
            views.push(child_of(
                view(&format!("kid-{n}"), Phase::Working, 20 + n as u64),
                "parent-a1b",
            ));
        }
        let list = listed(views);
        let drawn = lines(&list);
        assert_eq!(drawn[0], "Working (1)");
        assert_eq!(drawn[1], "parent-a1b");
        assert_eq!(
            drawn.last().map(String::as_str),
            Some("  … 3 sub"),
            "the parent says how many of its own the fold held back"
        );
        assert_eq!(
            drawn.iter().filter(|line| line.contains("kid-")).count(),
            FOLD_AT - 1,
            "the parent and the children that fit are what the fold kept"
        );
        assert!(
            !drawn.iter().any(|line| line.contains("more")),
            "no top-level row was hidden"
        );
    }

    #[test]
    fn view_asks_the_disk_for_a_childs_project() {
        // A child is grouped with its parent's project and never heads its own.
        let list = over_the_disk(vec![
            at(view("parent-a1b", Phase::Working, 10), "/src/api"),
            at(
                child_of(view("scout-b2c", Phase::Working, 20), "parent-a1b"),
                "/src/api",
            ),
        ]);
        assert_eq!(
            lines(&list),
            ["/src/api (1)", "parent-a1b", "└─scout-b2c"],
            "one project heading for the family"
        );
    }

    #[test]
    fn view_orders_the_finished_agents_by_when_their_run_ended() {
        // Events can land after the exit is recorded (a late hook or answer). The
        // group sorts by the recorded end, not the last event.
        let mut early = view("done-a1b", Phase::Done, 100);
        early.state.ended = 100;
        early.state.last_event = 400;
        let mut late = view("done-b2c", Phase::Done, 300);
        late.state.ended = 300;

        let list = listed(vec![early, late]);
        assert_eq!(lines(&list), ["Completed (2)", "done-b2c", "done-a1b"]);
    }

    #[test]
    fn view_folds_a_group_past_thirty_rows_behind_a_count() {
        // Two more than the fold: the heading, the newest `FOLD_AT`, and the fold
        // line. No screen height is involved.
        let mut list = listed(a_history(FOLD_AT + 2));
        let mut standing = vec![format!("Completed ({})", FOLD_AT + 2)];
        standing.extend((0..FOLD_AT).map(|n| format!("done-{}", FOLD_AT + 1 - n)));
        standing.push("… 2 more".to_string());
        assert_eq!(lines(&list), standing);

        for _ in 0..FOLD_AT {
            list.down();
        }
        list.unfold();
        assert_eq!(
            lines(&list).len(),
            FOLD_AT + 3,
            "the fold line is gone with the fold"
        );
        assert!(lines(&list).contains(&"done-0".to_string()));

        // It stays open as more agents finish.
        list.show(a_history(FOLD_AT + 3));
        assert!(lines(&list).contains(&"done-0".to_string()));
    }

    #[test]
    fn view_folds_the_thirty_first_row_of_a_group_and_leaves_thirty_standing() {
        let whole = listed(a_history(FOLD_AT));
        assert_eq!(
            lines(&whole).len(),
            FOLD_AT + 1,
            "a heading and {FOLD_AT} rows, with nothing held back: {:?}",
            lines(&whole)
        );

        let one_over = listed(a_history(FOLD_AT + 1));
        assert_eq!(
            lines(&one_over).last().map(String::as_str),
            Some("… 1 more"),
            "{:?}",
            lines(&one_over)
        );
    }

    #[test]
    fn view_folds_every_group_and_not_only_the_finished_ones() {
        // Every group folds, not only Completed.
        let mut views: Vec<View> = (0..FOLD_AT + 2)
            .map(|n| view(&format!("ask-{n}"), Phase::Waiting, 10 * n as u64))
            .collect();
        views.push(view("done-a1b", Phase::Done, 5));

        let mut standing = vec![format!("Needs input ({})", FOLD_AT + 2)];
        standing.extend((0..FOLD_AT).map(|n| format!("ask-{n}")));
        standing.extend([
            "… 2 more".to_string(),
            String::new(),
            "Completed (1)".to_string(),
            "done-a1b".to_string(),
        ]);
        assert_eq!(lines(&listed(views)), standing);
    }

    #[test]
    fn view_keeps_failures_and_requests_ahead_of_the_plainly_done() {
        // Four past the fold. The failure and the row with a pull request are kept
        // over the oldest plain rows, in group order. Read state plays no part.
        let mut list = List::default();
        list.asking(a_forge);
        let mut views: Vec<View> = (1..=FOLD_AT + 2)
            .map(|n| view(&format!("done-{n:02}"), Phase::Done, 10 * n as u64))
            .collect();
        views.push(view("broke-e5f", Phase::Failed, 2));
        views.push(on_a_branch(
            view("draft-g7h", Phase::Done, 1),
            "amx/draft-g7h",
        ));
        list.show(views);

        let mut standing = vec![format!("Completed ({})", FOLD_AT + 4)];
        standing.extend((0..FOLD_AT - 2).map(|n| format!("done-{:02}", FOLD_AT + 2 - n)));
        standing.extend([
            "broke-e5f".to_string(),
            "draft-g7h".to_string(),
            "… 4 more".to_string(),
        ]);
        assert_eq!(lines(&list), standing);
    }

    #[test]
    fn view_never_folds_the_row_out_from_under_the_cursor() {
        let mut list = listed(a_history(FOLD_AT + 1));
        for _ in 0..FOLD_AT - 1 {
            list.down();
        }
        assert_eq!(list.selected().unwrap().id(), "done-1");

        // A new ending pushes the cursor's row past the fold. It is kept anyway,
        // and an older row folds in its place.
        let mut views = a_history(FOLD_AT + 1);
        views.push(view(
            &format!("done-{}", FOLD_AT + 1),
            Phase::Done,
            10 * (FOLD_AT + 1) as u64,
        ));
        list.show(views);

        let mut standing = vec![format!("Completed ({})", FOLD_AT + 2)];
        standing.extend((0..FOLD_AT - 1).map(|n| format!("done-{}", FOLD_AT + 1 - n)));
        standing.extend(["done-1".to_string(), "… 2 more".to_string()]);
        assert_eq!(lines(&list), standing);
        assert_eq!(list.selected().unwrap().id(), "done-1");
    }

    #[test]
    fn view_reaches_the_top_and_the_foot_of_the_list_in_one_move() {
        let mut list = listed(vec![
            view("ask-a1b", Phase::Waiting, 10),
            view("busy-b2c", Phase::Working, 20),
            view("done-c3d", Phase::Done, 30),
        ]);
        // Both ends of the list are headings or agents, never blanks.
        assert_eq!(
            lines(&list),
            [
                "Needs input (1)",
                "ask-a1b",
                "",
                "Working (1)",
                "busy-b2c",
                "",
                "Completed (1)",
                "done-c3d",
            ]
        );

        list.bottom();
        assert_eq!(list.selected().unwrap().id(), "done-c3d");
        list.top();
        assert_eq!(
            list.cursor(),
            0,
            "the first line there is, which is a heading"
        );

        // From anywhere, and never onto a blank.
        list.down();
        list.bottom();
        list.bottom();
        assert_eq!(list.selected().unwrap().id(), "done-c3d", "and stays there");
    }

    #[test]
    fn view_keeps_the_cursor_on_the_agent_when_the_list_moves_under_it() {
        let mut list = listed(vec![
            view("ask-a1b", Phase::Waiting, 10),
            view("busy-b2c", Phase::Working, 20),
        ]);
        list.down();
        list.down();
        assert_eq!(list.selected().unwrap().id(), "busy-b2c");

        // The agent above answers and changes group, so the line index shifts.
        list.show(vec![
            view("ask-a1b", Phase::Idle, 10),
            view("busy-b2c", Phase::Working, 20),
        ]);
        assert_eq!(list.selected().unwrap().id(), "busy-b2c");

        // When the agent leaves, the cursor goes to the heading above, not to the
        // agent that moved into its line.
        list.show(vec![view("ask-a1b", Phase::Idle, 10)]);
        assert!(list.on_heading(), "a heading, not the agent below it");
        assert!(list.selected().is_none());
    }

    #[test]
    fn view_puts_the_cursor_on_the_next_agent_when_the_user_removed_its_own() {
        let fleet = || {
            vec![
                view("busy-a1b", Phase::Working, 10),
                view("busy-b2c", Phase::Working, 20),
                view("done-c3d", Phase::Idle, 30),
            ]
        };
        let without = |gone: &str| -> Vec<View> {
            fleet()
                .into_iter()
                .filter(|view| view.id() != gone)
                .collect()
        };

        // The row that takes the removed one's line.
        let mut list = listed(fleet());
        assert!(list.land_on("busy-a1b"));
        list.let_go("busy-a1b");
        list.show(without("busy-a1b"));
        assert_eq!(list.selected().unwrap().id(), "busy-b2c");

        // The last of its group: the next agent, under the next heading.
        let mut list = listed(fleet());
        assert!(list.land_on("busy-b2c"));
        list.let_go("busy-b2c");
        list.show(without("busy-b2c"));
        assert_eq!(list.selected().unwrap().id(), "done-c3d");

        // The last of all: the one above.
        let mut list = listed(fleet());
        assert!(list.land_on("done-c3d"));
        list.let_go("done-c3d");
        list.show(without("done-c3d"));
        assert_eq!(list.selected().unwrap().id(), "busy-b2c");

        // Only the next reading: a later one that loses the cursor's agent
        // goes to the heading as before.
        list.show(vec![view("busy-a1b", Phase::Working, 10)]);
        assert!(list.on_heading());
    }

    #[test]
    fn view_puts_the_cursor_on_an_agent_the_caller_names() {
        let mut list = listed(vec![
            view("ask-a1b", Phase::Waiting, 10),
            view("busy-b2c", Phase::Working, 20),
        ]);
        assert_eq!(list.selected().unwrap().id(), "ask-a1b");

        assert!(list.land_on("busy-b2c"));
        assert_eq!(list.selected().unwrap().id(), "busy-b2c");

        // An agent with no line: returns false and the cursor stays.
        assert!(!list.land_on("port-c3d"));
        assert_eq!(list.selected().unwrap().id(), "busy-b2c");
    }

    #[test]
    fn view_names_the_first_agent_that_needs_you() {
        // A waiting question comes first.
        let fleet = || {
            vec![
                view("busy-a1b", Phase::Working, 10),
                on_a_branch(view("review-b2c", Phase::Done, 20), "amx/fix-login-a1b"),
                view("ask-c3d", Phase::Waiting, 30),
                view("done-d4e", Phase::Done, 40),
            ]
        };
        assert_eq!(
            over_the_forge(fleet()).first_needing().as_deref(),
            Some("ask-c3d")
        );

        // Then a turn ready for review.
        let mut without_the_question = fleet();
        without_the_question.remove(2);
        assert_eq!(
            over_the_forge(without_the_question)
                .first_needing()
                .as_deref(),
            Some("review-b2c")
        );

        // Then the most recent ending, which heads Completed.
        let ended = |id: &str, at: u64| {
            let mut view = view(id, Phase::Done, at);
            view.state.ended = at;
            view
        };
        assert_eq!(
            listed(vec![
                view("busy-a1b", Phase::Working, 10),
                ended("early-b2c", 20),
                ended("late-c3d", 30),
            ])
            .first_needing()
            .as_deref(),
            Some("late-c3d")
        );

        // Working and sleeping agents need nothing.
        let list = sleeping(
            listed(vec![
                view("busy-a1b", Phase::Working, 10),
                view("nap-b2c", Phase::Waiting, 20),
            ]),
            "nap-b2c",
        );
        assert_eq!(list.first_needing(), None);
    }

    #[test]
    fn view_names_a_row_it_is_showing_for_the_cursor_to_land_on() {
        let fleet = || {
            vec![
                at(view("ask-a1b", Phase::Waiting, 10), "/src/web/app"),
                at(view("busy-b2c", Phase::Working, 20), "/src/api"),
                at(view("done-c3d", Phase::Done, 30), "/src/api"),
            ]
        };

        // The answer does not depend on the axis.
        let mut list = over_the_disk(fleet());
        assert_eq!(list.first_needing().as_deref(), Some("ask-a1b"));
        assert!(list.land_on("ask-a1b"));

        // An agent hidden by a narrowing is skipped, since the cursor cannot reach
        // it.
        let mut list = listed(fleet());
        list.narrow(vec![Narrow::State(Some("done".to_string()))]);
        assert_eq!(list.first_needing().as_deref(), Some("done-c3d"));
        assert!(list.land_on("done-c3d"));

        list.narrow(vec![Narrow::State(Some("working".to_string()))]);
        assert_eq!(
            list.first_needing(),
            None,
            "a screen of work in flight has nothing on it to land on"
        );

        // So is an agent under a shut heading.
        let mut list = listed(fleet());
        list.top();
        list.shut_or_open();
        assert_eq!(
            lines(&list),
            [
                "Needs input (1) shut",
                "",
                "Working (1)",
                "busy-b2c",
                "",
                "Completed (1)",
                "done-c3d",
            ]
        );
        assert_eq!(list.first_needing().as_deref(), Some("done-c3d"));
    }

    #[test]
    fn view_can_reach_the_fold_and_open_it_where_it_stands() {
        let mut list = listed(a_history(FOLD_AT + 2));

        for _ in 0..FOLD_AT {
            list.down();
        }
        assert!(list.on_fold());
        assert!(list.selected().is_none(), "a fold is not an agent");

        list.unfold();
        assert!(!list.on_fold());
        assert_eq!(
            list.selected().unwrap().id(),
            "done-1",
            "the cursor stays where the fold was, which is now an agent"
        );
    }

    #[test]
    fn view_keeps_the_cursor_on_its_agent_when_a_fold_above_it_opens() {
        let mut views: Vec<View> = (0..FOLD_AT + 2)
            .map(|n| view(&format!("busy-{n:02}"), Phase::Working, n as u64))
            .collect();
        views.push(view("done-a1b", Phase::Done, 100));
        let mut list = listed(views);
        list.bottom();
        assert_eq!(list.selected().unwrap().id(), "done-a1b");

        let fold = list
            .items()
            .iter()
            .position(|item| matches!(item, Item::Fold(..)))
            .expect("the working group folds");
        list.unfold_at(fold);
        assert!(
            !list
                .items()
                .iter()
                .any(|item| matches!(item, Item::Fold(..)))
        );
        assert_eq!(list.selected().unwrap().id(), "done-a1b");
    }

    #[test]
    fn axis_gathers_the_agents_under_the_project_they_run_in() {
        let list = over_the_disk(vec![
            at(view("busy-a1b", Phase::Working, 10), "/src/web/app"),
            at(view("ask-b2c", Phase::Waiting, 20), "/src/api"),
            at(view("done-c3d", Phase::Done, 30), "/src/api/cmd/serve"),
            at(view("loose-d4e", Phase::Idle, 40), "/tmp/scratch"),
        ]);

        assert_eq!(
            lines(&list),
            [
                "/src/api (2)",
                "ask-b2c",
                "done-c3d",
                "",
                "/src/web (1)",
                "busy-a1b",
                "",
                "/tmp/scratch (1)",
                "loose-d4e",
            ],
            "a subdirectory belongs to the repository over it, and a directory \
             under no repository at all is its own project"
        );
    }

    #[test]
    fn axis_says_which_project_the_cursor_is_standing_in() {
        // A heading and every row under it answer with the heading's project.
        let mut list = over_the_disk(vec![
            at(view("ask-a1b", Phase::Waiting, 10), "/src/api"),
            at(view("done-b2c", Phase::Done, 20), "/src/api/cmd/serve"),
            at(view("loose-c3d", Phase::Idle, 30), "/tmp/scratch"),
        ]);

        list.top();
        assert!(list.on_heading());
        assert_eq!(list.project_under_cursor(), Some(PathBuf::from("/src/api")));
        list.down();
        assert_eq!(list.project_under_cursor(), Some(PathBuf::from("/src/api")));
        list.down();
        assert_eq!(
            list.project_under_cursor(),
            Some(PathBuf::from("/src/api")),
            "the row of an agent started in a subdirectory answers with the \
             repository its heading stands for"
        );
        list.bottom();
        assert_eq!(
            list.project_under_cursor(),
            Some(PathBuf::from("/tmp/scratch"))
        );

        // The repo axis answers too. Only the state axis has no project.
        list.turn();
        assert_eq!(list.project_under_cursor(), None);
        list.turn();
        assert_eq!(
            list.project_under_cursor(),
            Some(PathBuf::from("/tmp/scratch"))
        );
    }

    #[test]
    fn axis_puts_the_project_whose_agent_is_waiting_first() {
        // Projects sort by their most urgent agent, then by path.
        let list = over_the_disk(vec![
            at(view("busy-a1b", Phase::Working, 10), "/src/api"),
            at(view("ask-b2c", Phase::Waiting, 20), "/src/web"),
            at(view("idle-c3d", Phase::Idle, 30), "/aaa"),
            at(view("quiet-d4e", Phase::Idle, 40), "/bbb"),
        ]);

        assert_eq!(
            lines(&list),
            [
                "/src/web (1)",
                "ask-b2c",
                "",
                "/src/api (1)",
                "busy-a1b",
                "",
                "/aaa (1)",
                "idle-c3d",
                "",
                "/bbb (1)",
                "quiet-d4e",
            ]
        );
    }

    #[test]
    fn repo_axis_gathers_every_worktree_under_the_repository_it_shares() {
        // A `workflow run` tree, an amx worktree and a subdirectory of the checkout
        // share one heading because git says they share a repository.
        let list = over_the_repos(vec![
            at(
                view("worker-a1b", Phase::Working, 10),
                "/work/repo/workflow/plan/t1",
            ),
            in_a_worktree(
                view("amx-b2c", Phase::Working, 20),
                "/work/repo/.amx/worktrees/amx-b2c",
            ),
            at(view("plain-c3d", Phase::Idle, 30), "/work/repo/src"),
            at(view("elsewhere-d4e", Phase::Idle, 40), "/tmp/scratch"),
        ]);

        assert_eq!(
            lines(&list),
            [
                "/work/repo (main) (3)",
                "worker-a1b",
                "amx-b2c",
                "plain-c3d",
                "",
                "/tmp/scratch (1)",
                "elsewhere-d4e",
            ],
            "a workflow worker, an amx worktree and a subdirectory are one \
             repository, and a directory outside one is its own place"
        );
    }

    #[test]
    fn repo_axis_names_the_branch_each_repository_has_checked_out() {
        // The branch tells a repo heading from a project heading on the same path.
        let mut list = over_the_repos(vec![
            at(view("busy-a1b", Phase::Working, 10), "/work/repo/src"),
            at(view("loose-b2c", Phase::Idle, 20), "/tmp/scratch"),
        ]);

        assert_eq!(
            lines(&list),
            [
                "/work/repo (main) (1)",
                "busy-a1b",
                "",
                "/tmp/scratch (1)",
                "loose-b2c",
            ],
            "a root git cannot name a branch at is the bare path"
        );

        // The project axis shows no branch.
        list.turn();
        list.turn();
        assert_eq!(list.axis(), Axis::Project);
        assert_eq!(lines(&list)[0], "/work/repo/src (1)");
    }

    #[test]
    fn axis_reads_a_worktree_back_to_the_repository_it_was_cut_from() {
        // An agent in an amx worktree is grouped under the repository it was cut
        // from.
        let list = over_the_disk(vec![
            in_a_worktree(
                view("fix-login-a1b", Phase::Working, 10),
                "/src/api/.amx/worktrees/fix-login-a1b",
            ),
            at(view("plain-b2c", Phase::Idle, 20), "/src/api"),
            in_a_worktree(view("astray-c3d", Phase::Idle, 30), "/elsewhere"),
        ]);

        assert_eq!(
            lines(&list),
            [
                "/src/api (2)",
                "fix-login-a1b",
                "plain-b2c",
                "",
                "/elsewhere (1)",
                "astray-c3d"
            ],
            "and a worktree of the wrong shape is grouped by where it runs"
        );
    }

    #[test]
    fn axis_says_where_a_project_is_the_way_a_person_writes_it() {
        let list = over_the_disk(vec![at(
            view("busy-a1b", Phase::Working, 10),
            "/home/dev/code/amx",
        )]);
        assert_eq!(lines(&list)[0], "~/code/amx (1)");
    }

    #[test]
    fn axis_asks_the_disk_once_for_an_agent_however_often_it_is_read() {
        let mut list = over_the_disk(vec![at(view("busy-a1b", Phase::Working, 10), "/src/api")]);
        let first = asked();
        assert!(!first.is_empty(), "the walk happened at all");

        for _ in 0..3 {
            list.show(vec![at(view("busy-a1b", Phase::Working, 10), "/src/api")]);
        }
        assert_eq!(
            asked(),
            first,
            "a reading every second may not walk the same agent's ancestors again"
        );
    }

    #[test]
    fn axis_forgets_the_project_of_an_agent_that_left_the_wall() {
        let mut list = over_the_disk(vec![
            at(view("ask-a1b", Phase::Waiting, 10), "/src/api"),
            at(view("busy-b2c", Phase::Working, 20), "/src/web"),
        ]);
        assert_eq!(list.roots.len(), 2);

        list.show(vec![at(view("busy-b2c", Phase::Working, 20), "/src/web")]);
        assert_eq!(
            list.roots.keys().collect::<Vec<_>>(),
            ["busy-b2c"],
            "a view left open for days does not keep every agent it ever saw"
        );
    }

    #[test]
    fn repo_axis_forgets_the_branch_of_a_repository_that_left_the_wall() {
        let mut list = over_the_repos(vec![
            at(view("busy-a1b", Phase::Working, 10), "/work/repo/src"),
            at(view("loose-b2c", Phase::Idle, 20), "/tmp/scratch"),
        ]);
        assert_eq!(list.branches.len(), 2);

        list.show(vec![at(
            view("busy-a1b", Phase::Working, 10),
            "/work/repo/src",
        )]);
        assert_eq!(
            list.branches.keys().collect::<Vec<_>>(),
            [Path::new("/work/repo")]
        );
    }

    #[test]
    fn axis_turns_between_what_they_need_where_they_are_and_which_repo() {
        let mut list = List::probing(a_disk_with_repos, Some(PathBuf::from("/home/dev")));
        list.show(vec![
            at(view("ask-a1b", Phase::Waiting, 10), "/src/api"),
            at(view("busy-b2c", Phase::Working, 20), "/src/web"),
        ]);
        assert_eq!(list.axis(), Axis::State);

        // The state axis sits between the two path axes, since a direct turn
        // between them often changes nothing visible.
        let walk: Vec<Axis> = (0..6)
            .map(|_| {
                list.turn();
                list.axis()
            })
            .collect();
        assert_eq!(
            walk,
            [
                Axis::Project,
                Axis::State,
                Axis::Repo,
                Axis::State,
                Axis::Project,
                Axis::State,
            ]
        );
        assert_eq!(
            lines(&list),
            ["Needs input (1)", "ask-a1b", "", "Working (1)", "busy-b2c"]
        );

        // The next turn shows the path axis not shown last.
        list.turn();
        assert_eq!(list.axis(), Axis::Repo);
        assert_eq!(lines(&list)[0], "/src/api (1)");
    }

    #[test]
    fn axis_keeps_the_cursor_on_its_agent_when_the_axis_turns() {
        let mut list = over_the_disk(vec![
            at(view("ask-a1b", Phase::Waiting, 10), "/src/api"),
            at(view("busy-b2c", Phase::Working, 20), "/src/web"),
        ]);
        list.down();
        list.down();
        assert_eq!(list.selected().unwrap().id(), "busy-b2c");

        list.turn();
        assert_eq!(
            list.selected().unwrap().id(),
            "busy-b2c",
            "the agent somebody was looking at is the one they are still on"
        );
    }

    #[test]
    fn axis_folds_a_project_past_thirty_rows_and_opens_that_heading_alone() {
        let views: Vec<View> = (0..FOLD_AT + 2)
            .map(|n| {
                at(
                    view(&format!("done-{n}"), Phase::Done, 10 * n as u64),
                    "/src/api",
                )
            })
            .collect();
        let mut list = over_the_disk(views);

        assert_eq!(
            lines(&list).len(),
            FOLD_AT + 2,
            "a path holds as many rows as a group does: {:?}",
            lines(&list)
        );
        assert!(lines(&list).contains(&"… 2 more".to_string()));

        for _ in 0..FOLD_AT {
            list.down();
        }
        list.unfold();
        assert_eq!(lines(&list).len(), FOLD_AT + 3, "{:?}", lines(&list));

        // Unfolding is per heading, so the state axis is still folded.
        list.turn();
        assert!(
            lines(&list).contains(&"… 2 more".to_string()),
            "{:?}",
            lines(&list)
        );
    }

    #[test]
    fn axis_narrows_the_list_to_the_agents_a_line_named() {
        let mut list = listed(vec![
            view("ask-a1b", Phase::Waiting, 10),
            view("busy-b2c", Phase::Working, 20),
            view("busy-c3d", Phase::Working, 30),
            view("done-d4e", Phase::Done, 40),
        ]);

        list.narrow(vec![Narrow::State(Some("working".to_string()))]);
        assert_eq!(
            lines(&list),
            ["Working (2)", "busy-b2c", "busy-c3d"],
            "a hidden agent heads nothing, counts for nothing and is drawn nowhere"
        );
        assert_eq!(list.counts(), [(Group::Working, 2)]);
        assert_eq!(list.narrowing().as_deref(), Some("s:working"));

        // Several states keep any of them.
        list.narrow(vec![
            Narrow::State(Some("waiting".to_string())),
            Narrow::State(Some("working".to_string())),
        ]);
        assert_eq!(
            lines(&list),
            [
                "Needs input (1)",
                "ask-a1b",
                "",
                "Working (2)",
                "busy-b2c",
                "busy-c3d",
            ],
            "both words keep their group and everything else is gone"
        );
        assert_eq!(
            list.narrowing().as_deref(),
            Some("s:waiting s:working"),
            "and the header reads the whole line back as it was typed"
        );

        list.narrow(vec![Narrow::State(Some("working".to_string()))]);
        assert_eq!(
            lines(&list),
            ["Working (2)", "busy-b2c", "busy-c3d"],
            "a shorter line replaces the states rather than adding to them"
        );

        list.narrow(vec![Narrow::Name(Some("c3d".to_string()))]);
        assert_eq!(
            lines(&list),
            ["Working (1)", "busy-c3d"],
            "and a line only changes the narrowing it names"
        );
        assert_eq!(
            list.narrowing().as_deref(),
            Some("s:working /c3d"),
            "the name reads back as the find line it came off"
        );

        // A half-typed `s:waiting s` parses as a name, so a batch of states drops
        // the name.
        list.narrow(vec![
            Narrow::State(Some("waiting".to_string())),
            Narrow::State(Some("working".to_string())),
        ]);
        assert_eq!(
            list.narrowing().as_deref(),
            Some("s:waiting s:working"),
            "a line of state words is not a line with a name on it"
        );

        list.narrow(vec![Narrow::State(None), Narrow::Name(None)]);
        assert_eq!(lines(&list).len(), 9);
        assert_eq!(list.narrowing(), None);
    }

    #[test]
    fn axis_narrows_the_project_headings_with_the_agents_under_them() {
        let mut list = over_the_disk(vec![
            at(view("ask-a1b", Phase::Waiting, 10), "/src/api"),
            at(view("busy-b2c", Phase::Working, 20), "/src/web"),
        ]);

        list.narrow(vec![Narrow::State(Some("waiting".to_string()))]);
        assert_eq!(
            lines(&list),
            ["/src/api (1)", "ask-a1b"],
            "a project whose last agent was hidden is not a project any more"
        );
    }

    #[test]
    fn axis_narrowed_to_nothing_leaves_the_cursor_somewhere_it_can_rest() {
        let mut list = listed(vec![
            view("ask-a1b", Phase::Waiting, 10),
            view("busy-b2c", Phase::Working, 20),
        ]);
        list.down();

        list.narrow(vec![Narrow::Name(Some("nobody".to_string()))]);
        assert!(list.is_empty());
        assert!(list.selected().is_none());
        assert_eq!(list.cursor(), 0);

        list.narrow(vec![Narrow::Name(None)]);
        assert!(list.selected().is_some(), "and it comes back on an agent");
    }

    #[test]
    fn the_first_reading_that_saw_the_panes_places_the_cursor() {
        // The records alone read a gone pane as its last phase, so the agent
        // heading the first frame may belong lower down once a reading has
        // looked. Nobody has moved the cursor yet, so it opens on the first
        // agent that reading draws, not wherever the stale one went.
        let mut list = listed(vec![
            view("gone-a1b", Phase::Working, 10),
            view("done-b2c", Phase::Idle, 20),
            view("done-c3d", Phase::Idle, 30),
        ]);
        assert_eq!(list.selected().unwrap().id(), "gone-a1b");

        list.land_again();
        list.show(vec![
            view("gone-a1b", Phase::Stopped, 10),
            view("done-b2c", Phase::Idle, 20),
            view("done-c3d", Phase::Idle, 30),
        ]);
        assert_eq!(list.selected().unwrap().id(), "done-c3d");

        // From then on the cursor stays with its agent as rows move.
        list.show(vec![
            view("gone-a1b", Phase::Stopped, 10),
            view("done-b2c", Phase::Working, 20),
            view("done-c3d", Phase::Idle, 30),
        ]);
        assert_eq!(list.selected().unwrap().id(), "done-c3d");
    }

    #[test]
    fn headings_are_stops_the_cursor_walks_like_any_other_line() {
        let mut list = listed(vec![
            view("ask-a1b", Phase::Waiting, 10),
            view("busy-b2c", Phase::Working, 20),
        ]);

        // The list opens on the first agent, not its heading.
        assert_eq!(list.cursor(), 1);
        assert_eq!(list.selected().unwrap().id(), "ask-a1b");

        list.up();
        assert!(list.on_heading(), "the heading over it is a stop");
        assert!(list.selected().is_none(), "a heading is not an agent");
        assert_eq!(list.cursor(), 0);

        for want in [1, 3, 4] {
            list.down();
            assert_eq!(
                list.cursor(),
                want,
                "and the walk down takes every stop, straight over the blank"
            );
        }
        list.down();
        assert_eq!(list.cursor(), 4, "the end of the list is the end");
    }

    #[test]
    fn headings_shut_the_group_under_them_and_open_it_again() {
        let mut list = listed(vec![
            view("ask-a1b", Phase::Waiting, 10),
            view("busy-b2c", Phase::Working, 20),
        ]);
        list.up();

        list.shut_or_open();
        assert_eq!(
            lines(&list),
            ["Needs input (1) shut", "", "Working (1)", "busy-b2c"],
            "the rows go and the heading stays"
        );
        assert!(list.on_heading(), "with the cursor still on it");

        list.shut_or_open();
        assert_eq!(
            lines(&list),
            ["Needs input (1)", "ask-a1b", "", "Working (1)", "busy-b2c"]
        );
    }

    #[test]
    fn headings_stay_shut_while_the_fleet_moves_under_them() {
        let mut list = listed(vec![
            view("done-a1b", Phase::Done, 10),
            view("busy-b2c", Phase::Working, 20),
        ]);
        // Onto the Completed heading.
        list.down();
        list.shut_or_open();

        list.show(vec![
            view("done-a1b", Phase::Done, 10),
            view("busy-b2c", Phase::Working, 20),
            view("ask-c3d", Phase::Waiting, 30),
        ]);
        assert_eq!(
            lines(&list),
            [
                "Needs input (1)",
                "ask-c3d",
                "",
                "Working (1)",
                "busy-b2c",
                "",
                "Completed (1) shut",
            ],
            "a group somebody shut stays shut while the reading moves under it"
        );
        assert!(
            list.on_heading(),
            "and the cursor is on the heading it was on, not the line it was at"
        );
    }

    #[test]
    fn headings_count_the_agents_a_shut_group_is_holding_back() {
        let mut list = listed(vec![
            view("done-a1b", Phase::Done, 10),
            view("failed-b2c", Phase::Failed, 20),
            view("stopped-c3d", Phase::Stopped, 30),
        ]);
        let heading = |list: &List| match list.items()[0] {
            Item::Heading(_, tally) => tally,
            item => panic!("no heading: {item:?}"),
        };
        list.up();

        // Three finished rows, one failed.
        assert_eq!(
            heading(&list),
            Tally {
                members: 3,
                failures: 1,
                states: states(&[(Group::Completed, 3)]),
                shut: false
            }
        );

        list.shut_or_open();
        assert_eq!(
            heading(&list),
            Tally {
                members: 3,
                failures: 1,
                states: states(&[(Group::Completed, 3)]),
                shut: true
            },
            "and it answers for the same agents when they are behind it"
        );

        list.narrow(vec![Narrow::Name(Some("done-a1b".to_string()))]);
        assert_eq!(
            heading(&list).members,
            1,
            "a heading may not claim members opening it could not reach"
        );
    }

    #[test]
    fn headings_on_the_project_axis_shut_the_project_rather_than_a_place_in_the_list() {
        let mut list = over_the_disk(vec![
            at(view("ask-a1b", Phase::Waiting, 10), "/src/api"),
            at(view("busy-b2c", Phase::Working, 20), "/src/web"),
        ]);
        list.up();
        list.shut_or_open();
        assert_eq!(
            lines(&list),
            ["/src/api (1) shut", "", "/src/web (1)", "busy-b2c"]
        );

        // The waiting agent answers and its project moves down. The shut state
        // follows the project, not the line.
        list.show(vec![
            at(view("ask-a1b", Phase::Idle, 10), "/src/api"),
            at(view("busy-b2c", Phase::Working, 20), "/src/web"),
        ]);
        assert_eq!(
            lines(&list),
            ["/src/web (1)", "busy-b2c", "", "/src/api (1) shut"]
        );
    }

    #[test]
    fn arranged_a_write_lays_only_this_views_change_over_the_file() {
        // Two views share one file. This view's changes since its last read are
        // applied over the file, so both views' pins survive.
        let published = Arrangement {
            held: ["gone-c3d".to_string()].into_iter().collect(),
            ..Arrangement::default()
        };
        let local = Arrangement {
            held: ["mine-a1b".to_string()].into_iter().collect(),
            ..Arrangement::default()
        };
        let disk = Arrangement {
            held: ["gone-c3d".to_string(), "theirs-b2c".to_string()]
                .into_iter()
                .collect(),
            ..Arrangement::default()
        };

        let merged = Arrangement::merged(&published, &local, &disk);
        assert_eq!(
            merged.held.iter().collect::<Vec<_>>(),
            ["mine-a1b", "theirs-b2c"],
            "this view unpinned gone-c3d and pinned mine-a1b; theirs stays"
        );

        // A changed group order is taken whole; untouched orders stay as on disk.
        let published = Arrangement::default();
        let mut local = Arrangement::default();
        local
            .order
            .insert(Group::Working, vec!["b".into(), "a".into()]);
        let mut disk = Arrangement::default();
        disk.order.insert(Group::Completed, vec!["c".into()]);
        let merged = Arrangement::merged(&published, &local, &disk);
        assert_eq!(merged.order[&Group::Working], ["b", "a"]);
        assert_eq!(merged.order[&Group::Completed], ["c"]);
    }

    #[test]
    fn arranged_a_shelve_in_one_view_and_a_pin_in_another_both_survive() {
        // Two views share one file: this one shelves busy-a1b while the other
        // has pinned the same agent and shelved done-b2c.
        let published = Arrangement::default();
        let local = Arrangement {
            shelved: [("busy-a1b".to_string(), Shelf::by_hand(50))].into(),
            ..Arrangement::default()
        };
        let disk = Arrangement {
            held: ["busy-a1b".to_string()].into_iter().collect(),
            shelved: [("done-b2c".to_string(), Shelf::by_hand(40))].into(),
            ..Arrangement::default()
        };

        let merged = Arrangement::merged(&published, &local, &disk);
        assert!(merged.has_pinned("busy-a1b"), "their pin stays");
        assert_eq!(
            merged.shelved,
            [
                ("busy-a1b".to_string(), Shelf::by_hand(50)),
                ("done-b2c".to_string(), Shelf::by_hand(40)),
            ]
            .into(),
            "this view's shelve joins theirs"
        );

        // This view returning done-b2c takes it off the file's shelf.
        let returned = Arrangement::merged(&disk, &Arrangement::default(), &disk);
        assert!(returned.shelved.is_empty(), "{returned:?}");

        // On the wall the pin wins.
        let mut list = listed(vec![view("busy-a1b", Phase::Done, 10)]);
        list.arrange(merged);
        assert_eq!(lines(&list), ["Pinned (1)", "busy-a1b"]);
    }

    #[test]
    fn arranged_a_shelve_holds_until_the_agent_next_changes_phase() {
        let mark = Shelf::by_hand(100);
        assert!(
            shelves(Some(&mark), Phase::Done, 99),
            "set after it finished"
        );
        assert!(
            shelves(Some(&mark), Phase::Idle, 100),
            "set the same second"
        );
        assert!(
            !shelves(Some(&mark), Phase::Idle, 101),
            "older than its last phase change"
        );
        assert!(
            !shelves(Some(&mark), Phase::Working, 99),
            "working again, before the record says so"
        );
        assert!(!shelves(None, Phase::Done, 99), "never shelved");
        for phase in [Phase::Idle, Phase::Done, Phase::Failed, Phase::Stopped] {
            assert!(
                shelves(Some(&mark), phase, 0),
                "{phase:?} is a finished turn"
            );
        }
        for phase in [
            Phase::Starting,
            Phase::Working,
            Phase::Waiting,
            Phase::Unknown,
        ] {
            assert!(!shelves(Some(&mark), phase, 0), "{phase:?} is not");
        }
    }

    #[test]
    fn arranged_a_mark_from_before_keep_reads_as_a_hand_shelve() {
        let old: Shelf = serde_json::from_str(r#"{"at": 50}"#).unwrap();
        assert_eq!(old, Shelf::by_hand(50));
        assert_eq!(
            serde_json::to_string(&old).unwrap(),
            r#"{"at":50}"#,
            "an unset field is not written"
        );
        let kept: Shelf = serde_json::from_str(r#"{"keep": 70}"#).unwrap();
        assert_eq!(
            kept,
            Shelf {
                at: None,
                keep: Some(70)
            }
        );
        assert!(!shelves(Some(&kept), Phase::Done, 0), "keep is no shelve");
    }

    #[test]
    fn arranged_a_pin_then_a_sleep_then_a_shelve_win_over_the_phase() {
        assert_eq!(
            Group::of(Phase::Done, true, true, true, true),
            Group::Pinned
        );
        assert_eq!(
            Group::of(Phase::Done, false, true, true, true),
            Group::Asleep
        );
        assert_eq!(
            Group::of(Phase::Done, false, false, true, true),
            Group::Shelved,
            "over an open pull request too"
        );
        assert_eq!(
            Group::of(Phase::Done, false, false, false, false),
            Group::Completed
        );
    }

    #[test]
    fn arranged_a_shelved_agent_is_listed_last_under_shelved() {
        let mut list = listed(vec![
            view("done-a1b", Phase::Done, 10),
            view("done-b2c", Phase::Done, 20),
            view("nap-c3d", Phase::Done, 30),
        ]);
        list.arrange(Arrangement {
            asleep: ["nap-c3d".to_string()].into_iter().collect(),
            shelved: [("done-a1b".to_string(), Shelf::by_hand(50))].into(),
            ..Arrangement::default()
        });
        assert_eq!(
            lines(&list),
            [
                "Completed (1)",
                "done-b2c",
                "",
                "Asleep (1)",
                "nap-c3d",
                "",
                "Shelved (1) shut",
            ]
        );
        assert_eq!(Group::ALL.last(), Some(&Group::Shelved));
        assert_eq!(Group::Shelved.state(), "shelved");
    }

    /// Three finished rows listed a, b, c: Completed lists the newest ending
    /// first.
    fn three_done() -> Vec<View> {
        vec![
            view("done-a1b", Phase::Done, 30),
            view("done-b2c", Phase::Done, 20),
            view("done-c3d", Phase::Done, 10),
        ]
    }

    #[test]
    fn shelving_a_finished_row_puts_it_under_a_shut_heading_and_moves_on() {
        let mut list = listed(three_done());
        to(&mut list, "done-b2c");

        assert_eq!(list.shelve_or_return(100), Shelving::Shelved);
        assert_eq!(list.selected().unwrap().id(), "done-c3d");
        assert_eq!(
            lines(&list),
            [
                "Completed (2)",
                "done-a1b",
                "done-c3d",
                "",
                "Shelved (1) shut"
            ]
        );
        assert_eq!(
            list.counts(),
            [(Group::Completed, 2), (Group::Shelved, 1)],
            "the header counts it as shelved"
        );

        to_heading(&mut list, Group::Shelved);
        list.shut_or_open();
        assert_eq!(lines(&list)[4..], ["Shelved (1)", "done-b2c"]);

        // A view opened afterwards has the heading shut again.
        let mut again = listed(three_done());
        again.arrange(list.arrangement());
        assert_eq!(lines(&again)[4..], ["Shelved (1) shut"]);
    }

    #[test]
    fn shelving_lists_the_newest_shelved_first() {
        let mut list = listed(three_done());
        to(&mut list, "done-b2c");
        list.shelve_or_return(100);
        to(&mut list, "done-a1b");
        list.shelve_or_return(101);
        to_heading(&mut list, Group::Shelved);
        list.shut_or_open();
        assert_eq!(
            lines(&list),
            [
                "Completed (1)",
                "done-c3d",
                "",
                "Shelved (2)",
                "done-a1b",
                "done-b2c"
            ]
        );
    }

    #[test]
    fn shelving_a_pinned_or_sleeping_row_lets_go_of_the_pin_or_the_sleep() {
        let mut list = sleeping(pinning(listed(three_done()), "done-a1b"), "done-c3d");
        to(&mut list, "done-a1b");
        assert_eq!(list.shelve_or_return(100), Shelving::Shelved);
        to(&mut list, "done-c3d");
        assert_eq!(list.shelve_or_return(100), Shelving::Shelved);

        let arranged = list.arrangement();
        assert!(!arranged.has_pinned("done-a1b"));
        assert!(!arranged.has_asleep("done-c3d"));
        assert_eq!(list.counts(), [(Group::Completed, 1), (Group::Shelved, 2)]);
    }

    #[test]
    fn shelving_a_shelved_row_puts_it_back() {
        let mut list = listed(three_done());
        to(&mut list, "done-b2c");
        list.shelve_or_return(100);
        to_heading(&mut list, Group::Shelved);
        list.shut_or_open();
        to(&mut list, "done-b2c");

        assert_eq!(list.shelve_or_return(101), Shelving::Returned);
        assert_eq!(list.counts(), [(Group::Completed, 3)]);
        assert_eq!(
            lines(&list),
            ["Completed (3)", "done-a1b", "done-b2c", "done-c3d"]
        );
    }

    #[test]
    fn shelving_comes_undone_when_the_agent_moves_on() {
        let mut list = listed(three_done());
        to(&mut list, "done-b2c");
        list.shelve_or_return(100);

        // A reply: the agent works again, and the record says so later.
        let mut replied = three_done();
        replied[1] = view("done-b2c", Phase::Working, 100);
        list.show(replied);
        assert_eq!(list.counts(), [(Group::Working, 1), (Group::Completed, 2)]);

        let mut finished = three_done();
        finished[1] = view("done-b2c", Phase::Idle, 140);
        list.show(finished);
        assert_eq!(
            list.counts(),
            [(Group::Completed, 3)],
            "finished again, it stays out: its mark is older than the change"
        );
    }

    #[test]
    fn shelving_refuses_a_row_that_is_not_finished() {
        let fleet = vec![
            view("ask-a1b", Phase::Waiting, 10),
            view("busy-b2c", Phase::Working, 20),
            view("odd-c3d", Phase::Unknown, 30),
        ];
        let mut list = listed(fleet);
        let before = lines(&list);
        for (id, said) in [
            ("ask-a1b", "waiting on you; z sleeps it"),
            ("busy-b2c", "still working; z sleeps it"),
            ("odd-c3d", "cannot tell it finished; z sleeps it"),
        ] {
            to(&mut list, id);
            assert_eq!(list.shelve_or_return(100), Shelving::Refused(said));
            assert_eq!(list.selected().unwrap().id(), id, "the cursor stays");
        }
        assert_eq!(lines(&list), before);

        to_heading(&mut list, Group::Working);
        assert_eq!(list.shelve_or_return(100), Shelving::Nothing);
        assert_eq!(lines(&list), before);
    }

    #[test]
    fn shelving_a_parent_waits_for_its_family() {
        // A parent whose turn is over while its child works sits under
        // Working, and s says why it stays there.
        let mut list = listed(vec![
            view("lead-a1b", Phase::Done, 10),
            child_of(view("busy-b2c", Phase::Working, 20), "lead-a1b"),
        ]);
        to(&mut list, "lead-a1b");
        assert_eq!(
            list.shelve_or_return(100),
            Shelving::Refused("still working; z sleeps it")
        );

        // Shelved while the child was done, the family goes where a child
        // that starts working again pulls it.
        let mut list = listed(vec![
            view("lead-a1b", Phase::Done, 10),
            child_of(view("kid-b2c", Phase::Done, 20), "lead-a1b"),
        ]);
        to(&mut list, "lead-a1b");
        assert_eq!(list.shelve_or_return(100), Shelving::Shelved);
        assert_eq!(list.counts(), [(Group::Shelved, 1)], "child and all");
        list.show(vec![
            view("lead-a1b", Phase::Done, 10),
            child_of(view("kid-b2c", Phase::Working, 101), "lead-a1b"),
        ]);
        assert_eq!(list.counts(), [(Group::Working, 1)]);
    }

    #[test]
    fn shelving_a_parent_waits_for_a_pinned_or_sleeping_child_too() {
        // A child's pin or sleep keeps it out of the family's group, but a
        // shelve would still hide it while it works or asks.
        for phase in [Phase::Working, Phase::Waiting] {
            let mut list = sleeping(
                listed(vec![
                    view("lead-a1b", Phase::Done, 10),
                    child_of(view("kid-b2c", phase, 20), "lead-a1b"),
                ]),
                "kid-b2c",
            );
            to(&mut list, "lead-a1b");
            assert!(
                matches!(list.shelve_or_return(100), Shelving::Refused(_)),
                "{phase:?}"
            );
        }

        // Shelved while the pinned child was done, the family comes back
        // when that child works again.
        let mut list = pinning(
            listed(vec![
                view("lead-a1b", Phase::Done, 10),
                child_of(view("kid-b2c", Phase::Done, 20), "lead-a1b"),
            ]),
            "kid-b2c",
        );
        to(&mut list, "lead-a1b");
        assert_eq!(list.shelve_or_return(100), Shelving::Shelved);
        list.show(vec![
            view("lead-a1b", Phase::Done, 10),
            child_of(view("kid-b2c", Phase::Working, 101), "lead-a1b"),
        ]);
        assert_eq!(list.counts(), [(Group::Working, 1)]);
    }

    /// A list judging idleness at `now`, with `after` as every agent's
    /// `shelve_after`.
    fn idling(after: fn(&Meta) -> u64, now: u64, views: Vec<View>) -> List {
        let mut list = List::default();
        list.shelving_after(after);
        list.time_is(now);
        list.show(views);
        list
    }

    const DAY: u64 = 86_400;

    #[test]
    fn auto_a_finished_row_idle_past_shelve_after_is_shelved_without_a_key() {
        let mut list = idling(
            |_| 60,
            1061,
            vec![
                view("old-a1b", Phase::Done, 1000),
                view("new-b2c", Phase::Done, 1002),
            ],
        );
        assert_eq!(list.counts(), [(Group::Completed, 1), (Group::Shelved, 1)]);
        to_heading(&mut list, Group::Shelved);
        list.shut_or_open();
        assert_eq!(
            lines(&list),
            ["Completed (1)", "new-b2c", "", "Shelved (1)", "old-a1b"],
            "61 s idle is shelved, 59 s is not"
        );
    }

    #[test]
    fn auto_shelve_after_zero_never_shelves_for_idling() {
        let list = idling(|_| 0, 30 * DAY, vec![view("old-a1b", Phase::Done, 10)]);
        assert_eq!(list.counts(), [(Group::Completed, 1)]);
    }

    #[test]
    fn auto_the_default_shelve_after_is_three_days() {
        let list = idling(
            |_| crate::config::Config::default().shelve_after,
            1000 + 259_201,
            vec![
                view("old-a1b", Phase::Idle, 1000),
                view("new-b2c", Phase::Idle, 1002),
            ],
        );
        assert_eq!(list.counts(), [(Group::Completed, 1), (Group::Shelved, 1)]);
    }

    #[test]
    fn auto_a_working_or_waiting_row_is_never_shelved_however_old() {
        let list = idling(
            |_| 60,
            10 * DAY,
            vec![
                view("busy-a1b", Phase::Working, 10),
                view("ask-b2c", Phase::Waiting, 10),
                view("odd-c3d", Phase::Unknown, 10),
            ],
        );
        assert_eq!(
            list.counts(),
            [
                (Group::NeedsInput, 1),
                (Group::Working, 1),
                (Group::Completed, 1)
            ]
        );
    }

    #[test]
    fn auto_a_pinned_row_is_never_shelved() {
        let mut list = pinning(
            idling(|_| 60, 20, vec![view("done-a1b", Phase::Done, 10)]),
            "done-a1b",
        );
        list.time_is(10 * DAY);
        list.show(vec![view("done-a1b", Phase::Done, 10)]);
        assert_eq!(list.counts(), [(Group::Pinned, 1)]);
    }

    #[test]
    fn auto_a_phase_change_in_the_future_is_not_idle() {
        // A clock set back, or a record from a machine ahead of this one.
        let list = idling(|_| 60, 1000, vec![view("ahead-a1b", Phase::Done, 5000)]);
        assert_eq!(list.counts(), [(Group::Completed, 1)]);
        let list = idling(
            |_| u64::MAX,
            u64::MAX,
            vec![view("far-a1b", Phase::Done, 10)],
        );
        assert_eq!(list.counts(), [(Group::Completed, 1)], "no overflow");
    }

    #[test]
    fn auto_a_record_with_no_phase_change_time_is_not_idle() {
        // A state file without `since` reads as 0, which is no time at all
        // rather than 1970.
        let list = idling(|_| 60, 10 * DAY, vec![view("odd-a1b", Phase::Stopped, 0)]);
        assert_eq!(list.counts(), [(Group::Completed, 1)]);
    }

    /// A fake forge with a request of every settled and unsettled kind.
    fn a_settled_forge(meta: &Meta) -> Vec<Pr> {
        let one = |number, standing| vec![Pr { number, standing }];
        match meta.branch.as_deref() {
            Some("amx/merged-a1b") => one(1, Standing::Merged),
            Some("amx/closed-b2c") => one(2, Standing::Closed),
            Some("amx/open-c3d") => one(3, Standing::Open),
            Some("amx/draft-d4e") => one(4, Standing::Draft),
            Some("amx/both-e5f") => vec![
                Pr {
                    number: 5,
                    standing: Standing::Merged,
                },
                Pr {
                    number: 6,
                    standing: Standing::Failing,
                },
            ],
            _ => Vec::new(),
        }
    }

    /// [`idling`] over [`a_settled_forge`], each view on `amx/<its id>`.
    fn settling(now: u64, views: Vec<View>) -> List {
        let mut list = List::default();
        list.asking(a_settled_forge);
        list.shelving_after(|_| 259_200);
        list.time_is(now);
        list.show(
            views
                .into_iter()
                .map(|view| {
                    let branch = format!("amx/{}", view.id());
                    on_a_branch(view, &branch)
                })
                .collect(),
        );
        list
    }

    #[test]
    fn auto_a_merged_or_closed_request_shelves_a_row_whatever_its_age() {
        let list = settling(
            1005,
            vec![
                view("merged-a1b", Phase::Done, 1000),
                view("closed-b2c", Phase::Idle, 1000),
                view("plain-f6g", Phase::Done, 1000),
            ],
        );
        assert_eq!(list.counts(), [(Group::Completed, 1), (Group::Shelved, 2)]);
    }

    #[test]
    fn auto_an_open_request_keeps_a_row_in_review_and_a_draft_does_not() {
        let list = settling(
            1000 + 4 * DAY,
            vec![
                view("open-c3d", Phase::Done, 1000),
                view("draft-d4e", Phase::Done, 1000),
                view("both-e5f", Phase::Done, 1000),
            ],
        );
        assert_eq!(
            list.counts(),
            [(Group::Review, 2), (Group::Shelved, 1)],
            "open-c3d and both-e5f, whose open request outweighs its merged one"
        );
    }

    #[test]
    fn auto_with_no_request_a_young_row_stays_under_completed() {
        let list = settling(1005, vec![view("plain-f6g", Phase::Done, 1000)]);
        assert_eq!(list.counts(), [(Group::Completed, 1)]);
    }

    #[test]
    fn auto_a_merged_row_still_at_work_is_not_shelved() {
        // Replied to after its request merged: working again, it is back at
        // work, and once idle the merged request shelves it again.
        let list = settling(1005, vec![view("merged-a1b", Phase::Working, 1000)]);
        assert_eq!(list.counts(), [(Group::Working, 1)]);
        let list = settling(1010, vec![view("merged-a1b", Phase::Idle, 1009)]);
        assert_eq!(list.counts(), [(Group::Shelved, 1)]);
    }

    #[test]
    fn auto_shelved_rows_sort_by_when_they_went_on_the_shelf() {
        let mut list = settling(
            1000 + 4 * DAY,
            vec![
                view("plain-f6g", Phase::Done, 1000),
                view("merged-a1b", Phase::Done, 500 + 4 * DAY),
                view("hand-g7h", Phase::Done, 1000),
            ],
        );
        list.shelved
            .insert("hand-g7h".to_string(), Shelf::by_hand(900 + 4 * DAY));
        list.time_is(1000 + 4 * DAY);
        let views = list.views.clone();
        list.show(views);
        to_heading(&mut list, Group::Shelved);
        list.shut_or_open();
        assert_eq!(
            lines(&list),
            ["Shelved (3)", "hand-g7h", "merged-a1b", "plain-f6g"],
            "the press, then the merge as of its last phase change, then the \
             idle row as of when it passed shelve_after"
        );
    }

    #[test]
    fn auto_s_on_a_row_a_rule_shelved_keeps_it_out_until_it_next_moves() {
        let done = || vec![view("old-a1b", Phase::Done, 1000)];
        let mut list = idling(|_| 60, 1061, done());
        to_heading(&mut list, Group::Shelved);
        list.shut_or_open();
        to(&mut list, "old-a1b");
        assert_eq!(list.shelve_or_return(1061), Shelving::Returned);
        assert_eq!(list.counts(), [(Group::Completed, 1)]);
        assert_eq!(
            list.arrangement().shelved.get("old-a1b"),
            Some(&Shelf {
                at: None,
                keep: Some(1061)
            }),
            "persisted, so another view keeps it out too"
        );

        list.time_is(1061 + 120);
        list.show(done());
        assert_eq!(list.counts(), [(Group::Completed, 1)], "still out");

        // Its next turn lets the rule have it again once that turn idles.
        list.time_is(1300);
        list.show(vec![view("old-a1b", Phase::Done, 1250)]);
        assert_eq!(list.counts(), [(Group::Completed, 1)], "49 s idle");
        list.time_is(1312);
        list.show(vec![view("old-a1b", Phase::Done, 1250)]);
        assert_eq!(list.counts(), [(Group::Shelved, 1)], "61 s idle");
    }

    #[test]
    fn auto_s_on_a_row_shelved_by_hand_and_by_a_rule_keeps_it_out() {
        let done = || vec![view("old-a1b", Phase::Done, 1000)];
        let mut list = idling(|_| 60, 1010, done());
        to(&mut list, "old-a1b");
        assert_eq!(list.shelve_or_return(1010), Shelving::Shelved);
        list.time_is(1100);
        list.show(done());
        to_heading(&mut list, Group::Shelved);
        list.shut_or_open();
        to(&mut list, "old-a1b");
        assert_eq!(list.shelve_or_return(1100), Shelving::Returned);
        assert_eq!(list.counts(), [(Group::Completed, 1)]);
    }

    #[test]
    fn auto_a_parent_waits_for_its_family_to_finish() {
        let family = |kid: Phase, at: u64| {
            vec![
                view("lead-a1b", Phase::Done, 10),
                child_of(view("kid-b2c", kid, at), "lead-a1b"),
            ]
        };
        let mut list = idling(|_| 60, 10 * DAY, family(Phase::Working, 20));
        assert_eq!(list.counts(), [(Group::Working, 1)]);
        to(&mut list, "lead-a1b");
        assert_eq!(
            list.shelve_or_return(10 * DAY),
            Shelving::Refused("still working; z sleeps it"),
            "the parent is not on the shelf, so s has nothing to take back"
        );
        assert!(list.arrangement().shelved.is_empty());

        list.show(family(Phase::Waiting, 20));
        assert_eq!(list.counts(), [(Group::NeedsInput, 1)]);

        list.show(family(Phase::Done, 20));
        assert_eq!(list.counts(), [(Group::Shelved, 1)]);
        to_heading(&mut list, Group::Shelved);
        list.shut_or_open();
        assert_eq!(
            lines(&list),
            ["Shelved (1)", "lead-a1b", "└─kid-b2c"],
            "parent and child together"
        );
    }

    #[test]
    fn auto_shelve_after_is_read_once_per_agent_however_often_the_wall_is_read() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static READ: AtomicUsize = AtomicUsize::new(0);
        let mut list = List::default();
        list.shelving_after(|_| {
            READ.fetch_add(1, Ordering::SeqCst);
            60
        });
        for now in 1000..1100 {
            list.time_is(now);
            list.show(three_done());
        }
        assert_eq!(READ.load(Ordering::SeqCst), 3, "one read per agent");
    }

    #[test]
    fn auto_a_verb_stepping_the_wall_sees_the_shelf_the_view_draws() {
        let fleet = || {
            vec![
                view("old-a1b", Phase::Done, 1000),
                view("new-b2c", Phase::Done, 1000 + 4 * DAY),
            ]
        };
        let order = wall_order(&fleet(), &Arrangement::default(), 1001 + 4 * DAY);
        assert_eq!(
            walled(&order),
            ["Completed new-b2c", "Shelved old-a1b"],
            "the default three days have passed for one of them"
        );
        assert_eq!(needing_you(&order).as_deref(), Some("new-b2c"));
    }

    #[test]
    fn shelving_back_the_last_shelved_row_follows_it_off_the_shelf() {
        // Nothing below it, and the line above is the shelf's heading, which
        // goes with it: the cursor must not land on a row of another group.
        let mut list = sleeping(listed(three_done()), "done-c3d");
        to(&mut list, "done-b2c");
        list.shelve_or_return(100);
        to_heading(&mut list, Group::Shelved);
        list.shut_or_open();
        to(&mut list, "done-b2c");

        assert_eq!(list.shelve_or_return(101), Shelving::Returned);
        assert_eq!(list.selected().unwrap().id(), "done-b2c");
    }

    #[test]
    fn shelving_the_row_over_a_shut_shelf_lands_on_its_heading() {
        let mut list = listed(three_done());
        to(&mut list, "done-c3d");
        list.shelve_or_return(100);
        assert_eq!(list.selected().unwrap().id(), "done-b2c", "the line above");
        to(&mut list, "done-b2c");

        list.shelve_or_return(101);
        assert_eq!(list.heading(), Some(Under::Group(Group::Shelved)));
        assert_eq!(lines(&list)[list.cursor()], "Shelved (2) shut");
    }

    #[test]
    fn shelving_opens_the_shelf_to_a_narrowing() {
        let mut list = listed(three_done());
        to(&mut list, "done-b2c");
        list.shelve_or_return(100);
        list.narrow(vec![Narrow::Name(Some("b2c".to_string()))]);
        assert_eq!(lines(&list), ["Shelved (1)", "done-b2c"]);
        list.narrow(vec![Narrow::Name(None)]);
        assert_eq!(lines(&list)[3..], ["", "Shelved (1) shut"]);
    }

    #[test]
    fn arranged_the_axis_is_taken_only_where_this_view_turned_it() {
        let published = Arrangement {
            axis: Axis::Project,
            ..Arrangement::default()
        };
        let local = Arrangement {
            axis: Axis::Repo,
            ..Arrangement::default()
        };
        let disk = Arrangement {
            axis: Axis::State,
            ..Arrangement::default()
        };

        assert_eq!(
            Arrangement::merged(&published, &local, &disk).axis,
            Axis::Repo,
            "this view turned it, so the file's older answer gives way"
        );
        let unturned = published.clone();
        assert_eq!(
            Arrangement::merged(&published, &unturned, &disk).axis,
            Axis::State,
            "and where this view did not, the file's answer stands"
        );
    }

    #[test]
    fn arranged_a_pinned_agent_stands_over_the_groups_whatever_it_is_doing() {
        let mut list = listed(vec![
            view("ask-a1b", Phase::Waiting, 10),
            view("busy-b2c", Phase::Working, 20),
            view("busy-c3d", Phase::Working, 30),
            view("busy-d4e", Phase::Working, 40),
        ]);
        for _ in 0..3 {
            list.down();
        }
        assert_eq!(list.selected().unwrap().id(), "busy-c3d");

        assert!(list.hold_or_let_go());
        assert_eq!(
            lines(&list),
            [
                "Pinned (1)",
                "busy-c3d",
                "",
                "Needs input (1)",
                "ask-a1b",
                "",
                "Working (2)",
                "busy-b2c",
                "busy-d4e",
            ]
        );
        assert!(list.holding(list.agent_by_id("busy-c3d").unwrap()));

        // A pinned agent stays pinned when its phase changes.
        list.show(vec![
            view("ask-a1b", Phase::Waiting, 10),
            view("busy-b2c", Phase::Working, 20),
            view("busy-c3d", Phase::Waiting, 30),
            view("busy-d4e", Phase::Working, 40),
        ]);
        assert_eq!(
            lines(&list),
            [
                "Pinned (1)",
                "busy-c3d",
                "",
                "Needs input (1)",
                "ask-a1b",
                "",
                "Working (2)",
                "busy-b2c",
                "busy-d4e",
            ]
        );

        // The same key unpins it.
        assert!(list.hold_or_let_go());
        assert_eq!(
            lines(&list),
            [
                "Needs input (2)",
                "ask-a1b",
                "busy-c3d",
                "",
                "Working (2)",
                "busy-b2c",
                "busy-d4e",
            ]
        );
    }

    #[test]
    fn arranged_a_sleeping_agent_goes_under_every_group_whatever_it_is_doing() {
        let mut list = listed(vec![
            view("ask-a1b", Phase::Waiting, 10),
            view("busy-b2c", Phase::Working, 20),
            view("done-c3d", Phase::Done, 30),
        ]);
        assert_eq!(list.selected().unwrap().id(), "ask-a1b");

        assert!(list.sleep_or_wake());
        assert_eq!(
            lines(&list),
            [
                "Working (1)",
                "busy-b2c",
                "",
                "Completed (1)",
                "done-c3d",
                "",
                "Asleep (1)",
                "ask-a1b",
            ],
            "under everything, though it is the one agent asking"
        );
        assert!(list.sleeping(list.agent_by_id("ask-a1b").unwrap()));
        assert_eq!(
            list.waiting(),
            1,
            "and still counted among the ones waiting on somebody"
        );

        // A sleeping agent stays asleep while its turn runs.
        list.show(vec![
            view("ask-a1b", Phase::Working, 10),
            view("busy-b2c", Phase::Working, 20),
            view("done-c3d", Phase::Done, 30),
        ]);
        assert_eq!(
            lines(&list),
            [
                "Working (1)",
                "busy-b2c",
                "",
                "Completed (1)",
                "done-c3d",
                "",
                "Asleep (1)",
                "ask-a1b",
            ]
        );

        // The same key wakes it.
        assert!(list.sleep_or_wake());
        assert_eq!(
            lines(&list),
            [
                "Working (2)",
                "ask-a1b",
                "busy-b2c",
                "",
                "Completed (1)",
                "done-c3d",
            ]
        );
    }

    #[test]
    fn arranged_the_two_marks_a_person_puts_on_a_row_undo_each_other() {
        let mut list = listed(vec![
            view("busy-a1b", Phase::Working, 10),
            view("busy-b2c", Phase::Working, 20),
        ]);

        assert!(list.sleep_or_wake());
        assert_eq!(
            lines(&list),
            ["Working (1)", "busy-b2c", "", "Asleep (1)", "busy-a1b"]
        );

        // Pinning a sleeping agent wakes it.
        assert!(list.hold_or_let_go());
        assert_eq!(
            lines(&list),
            ["Pinned (1)", "busy-a1b", "", "Working (1)", "busy-b2c"]
        );
        assert!(!list.sleeping(list.agent_by_id("busy-a1b").unwrap()));

        // Sleeping a pinned agent unpins it.
        assert!(list.sleep_or_wake());
        assert_eq!(
            lines(&list),
            ["Working (1)", "busy-b2c", "", "Asleep (1)", "busy-a1b"]
        );
        assert!(!list.holding(list.agent_by_id("busy-a1b").unwrap()));

        // A heading cannot be put to sleep or pinned.
        list.top();
        assert!(list.on_heading());
        assert!(!list.sleep_or_wake());
        assert_eq!(
            lines(&list),
            ["Working (1)", "busy-b2c", "", "Asleep (1)", "busy-a1b"]
        );
    }

    #[test]
    fn arranged_an_order_somebody_put_a_group_in_outlives_the_readings_after_it() {
        let mut list = listed(vec![
            view("busy-a1b", Phase::Working, 10),
            view("busy-b2c", Phase::Working, 20),
            view("busy-c3d", Phase::Working, 30),
        ]);

        assert!(list.move_by(1));
        assert_eq!(
            lines(&list),
            ["Working (3)", "busy-b2c", "busy-a1b", "busy-c3d"]
        );
        assert_eq!(
            list.selected().unwrap().id(),
            "busy-a1b",
            "the cursor goes with the agent it moved"
        );

        list.show(vec![
            view("busy-a1b", Phase::Working, 10),
            view("busy-b2c", Phase::Working, 20),
            view("busy-c3d", Phase::Working, 30),
            view("busy-e5f", Phase::Working, 40),
        ]);
        assert_eq!(
            lines(&list),
            [
                "Working (4)",
                "busy-b2c",
                "busy-a1b",
                "busy-c3d",
                "busy-e5f"
            ],
            "and one started since joins the bottom of a group somebody \
             arranged, rather than being sorted into the middle of it"
        );
    }

    #[test]
    fn arranged_a_move_stops_at_the_ends_of_a_group_and_at_the_pinned_rows() {
        let mut list = listed(vec![
            view("busy-a1b", Phase::Working, 10),
            view("busy-b2c", Phase::Working, 20),
        ]);
        assert!(!list.move_by(-1), "nothing is above the first of a group");
        list.down();
        assert!(!list.move_by(1), "and nothing is under the last");

        // A pinned agent leaves its group, so it is out of a move's reach.
        assert!(list.hold_or_let_go());
        assert_eq!(
            lines(&list),
            ["Pinned (1)", "busy-b2c", "", "Working (1)", "busy-a1b"]
        );
        for _ in 0..2 {
            list.down();
        }
        assert_eq!(list.selected().unwrap().id(), "busy-a1b");
        assert!(!list.move_by(-1), "and the pinned row is not one of them");
        assert_eq!(
            lines(&list),
            ["Pinned (1)", "busy-b2c", "", "Working (1)", "busy-a1b"]
        );

        // A heading cannot be moved or pinned.
        list.up();
        assert!(list.on_heading());
        assert!(!list.move_by(1));
        assert!(!list.hold_or_let_go());
    }

    #[test]
    fn arranged_a_move_reaches_the_rows_on_the_screen_and_not_the_folded_ones() {
        let mut list = listed(a_history(FOLD_AT + 2));
        // To the last row the fold leaves.
        for _ in 0..FOLD_AT - 1 {
            list.down();
        }
        assert_eq!(list.selected().unwrap().id(), "done-2");

        assert!(
            !list.move_by(1),
            "the row under it is the fold, and behind that is history"
        );
        assert_eq!(lines(&list).last().map(String::as_str), Some("… 2 more"));

        // Once unfolded, every row can be moved.
        list.down();
        list.unfold();
        assert!(list.move_by(1));
        let after = lines(&list);
        assert_eq!(
            &after[FOLD_AT..],
            ["done-2", "done-0", "done-1"],
            "{after:?}"
        );
    }

    #[test]
    fn arranged_a_list_opens_on_the_arrangement_the_last_one_was_left_in() {
        let fleet = || {
            vec![
                at(view("busy-a1b", Phase::Working, 10), "/src/api"),
                at(view("busy-b2c", Phase::Working, 20), "/src/api"),
                at(view("busy-c3d", Phase::Working, 30), "/src/api"),
            ]
        };
        let mut list = over_the_disk(fleet());
        assert!(list.move_by(1));
        for _ in 0..2 {
            list.down();
        }
        assert!(list.hold_or_let_go());

        let left = lines(&list);
        assert_eq!(
            left,
            ["/src/api (3)", "busy-c3d", "busy-b2c", "busy-a1b"],
            "the one being held, and under it the order they were put in"
        );

        let mut opened = List::probing(a_disk_with_repos, Some(PathBuf::from("/home/dev")));
        opened.arrange(list.arrangement());
        opened.show(fleet());
        assert_eq!(
            lines(&opened),
            left,
            "another view, gathered the same way and holding the same agent"
        );
    }

    #[test]
    fn arranged_what_is_pinned_is_read_off_the_view_file_from_outside_the_view() {
        let state = TempDir::new().unwrap();
        let root = state.path().join("agents");
        std::fs::create_dir_all(&root).unwrap();

        assert_eq!(
            Arrangement::from_disk(&root),
            Arrangement::default(),
            "a fleet nobody has opened the view over has nobody pinned"
        );

        let list = sleeping(
            pinning(
                listed(vec![
                    view("fix-login-a1b", Phase::Idle, 10),
                    view("port-import-b2c", Phase::Idle, 20),
                    view("later-c3d", Phase::Idle, 30),
                ]),
                "fix-login-a1b",
            ),
            "later-c3d",
        );
        let kept = crate::paths::view_file(&root).expect("somewhere to keep it");
        crate::tui::Remembered {
            statusline: true,
            arrangement: list.arrangement(),
            vendor: false,
            sent: Default::default(),
        }
        .write(&kept)
        .unwrap();

        let read = Arrangement::from_disk(&root);
        assert_eq!(
            read,
            list.arrangement(),
            "the view's own file, the way the view left it"
        );
        assert!(read.has_pinned("fix-login-a1b"), "the one somebody pinned");
        assert!(
            !read.has_pinned("port-import-b2c"),
            "and not the row that was under it"
        );
        assert!(
            read.has_asleep("later-c3d"),
            "the one somebody put to sleep"
        );
        assert!(
            !read.has_asleep("fix-login-a1b"),
            "and not the one they pinned"
        );

        // A file saved before the asleep field existed reads with nobody asleep.
        std::fs::write(
            &kept,
            br#"{"arrangement":{"axis":"state","held":["fix-login-a1b"],"order":{}}}"#,
        )
        .unwrap();
        let older = Arrangement::from_disk(&root);
        assert!(older.has_pinned("fix-login-a1b"));
        assert!(!older.has_asleep("fix-login-a1b"));

        std::fs::write(&kept, b"{\"arrangement\":").unwrap();
        assert_eq!(
            Arrangement::from_disk(&root),
            Arrangement::default(),
            "a half-written file is nobody pinned rather than a refusal"
        );
    }

    #[test]
    fn wall_reads_from_the_pinned_row_down_to_the_sleeping_one() {
        let fleet = || {
            vec![
                view("ask-a1b", Phase::Waiting, 10),
                view("busy-b2c", Phase::Working, 20),
                on_a_branch(view("done-c3d", Phase::Done, 30), "amx/done-c3d"),
                view("busy-d4e", Phase::Working, 40),
            ]
        };
        let list = sleeping(pinning(listed(fleet()), "busy-d4e"), "ask-a1b");
        assert_eq!(
            lines(&list),
            [
                "Pinned (1)",
                "busy-d4e",
                "",
                "Working (1)",
                "busy-b2c",
                "",
                "Completed (1)",
                "done-c3d",
                "",
                "Asleep (1)",
                "ask-a1b",
            ],
            "the wall the view draws under this arrangement"
        );

        assert_eq!(
            walled(&wall_order(&fleet(), &list.arrangement(), 0)),
            [
                "Pinned busy-d4e",
                "Working busy-b2c",
                // No pull request is written down and a verb asks no forge, so this is
                // Completed, not Review.
                "Completed done-c3d",
                "Asleep ask-a1b",
            ],
            "the same rows, as the group each was drawn under and its id"
        );
    }

    #[test]
    fn wall_puts_the_newest_ending_first_among_the_finished() {
        let fleet = || {
            let mut early = view("done-a1b", Phase::Done, 100);
            early.state.ended = 100;
            let mut late = view("done-b2c", Phase::Done, 300);
            late.state.ended = 300;
            vec![early, late]
        };
        assert_eq!(
            lines(&listed(fleet())),
            ["Completed (2)", "done-b2c", "done-a1b"]
        );
        assert_eq!(
            walled(&wall_order(&fleet(), &Arrangement::default(), 0)),
            ["Completed done-b2c", "Completed done-a1b"],
            "history reads newest first outside the view as it does in it"
        );
    }

    #[test]
    fn wall_keeps_the_order_somebody_put_a_group_in() {
        let fleet = || {
            vec![
                view("busy-a1b", Phase::Working, 10),
                view("busy-b2c", Phase::Working, 20),
                view("busy-c3d", Phase::Working, 30),
            ]
        };
        let mut list = listed(fleet());
        assert!(list.move_by(1));
        assert_eq!(
            lines(&list),
            ["Working (3)", "busy-b2c", "busy-a1b", "busy-c3d"]
        );

        assert_eq!(
            walled(&wall_order(&fleet(), &list.arrangement(), 0)),
            ["Working busy-b2c", "Working busy-a1b", "Working busy-c3d"],
            "the order a hand put the group in, not the order they started in"
        );
    }

    #[test]
    fn wall_answers_the_state_order_for_a_view_left_on_the_project_axis() {
        let fleet = || {
            vec![
                at(view("busy-a1b", Phase::Working, 10), "/src/web/app"),
                at(view("ask-b2c", Phase::Waiting, 20), "/src/api"),
                at(view("done-c3d", Phase::Done, 30), "/src/api"),
            ]
        };
        let list = over_the_disk(fleet());
        assert_eq!(
            lines(&list),
            [
                "/src/api (2)",
                "ask-b2c",
                "done-c3d",
                "",
                "/src/web (1)",
                "busy-a1b",
            ],
            "the axis the view was left on"
        );

        assert_eq!(
            walled(&wall_order(&fleet(), &list.arrangement(), 0)),
            [
                "Needs input ask-b2c",
                "Working busy-a1b",
                "Completed done-c3d",
            ],
            "a verb stepping the wall is asking about the fleet, not about the \
             screen somebody closed"
        );
    }

    #[test]
    fn a_fleet_nobody_has_started_is_the_state_axis_with_nothing_narrowed() {
        let mut list = listed(Vec::new());
        assert!(list.unstarted());

        list.turn();
        assert!(
            !list.unstarted(),
            "the project axis is a list of places, and nobody arrives at one \
             without agents to arrange"
        );

        list.turn();
        list.narrow(vec![Narrow::Name(Some("nobody".to_string()))]);
        assert!(
            !list.unstarted(),
            "a fleet somebody narrowed to nothing is not a fleet nobody has started"
        );

        list.narrow(vec![Narrow::Name(None)]);
        list.show(vec![view("done-a1b", Phase::Done, 10)]);
        assert!(!list.unstarted(), "and one agent is a fleet");
    }

    #[test]
    fn a_group_titles_itself_in_the_words_its_heading_reads() {
        // The heading text is decided here, not by the painter.
        assert_eq!(
            Group::ALL.map(Group::title),
            [
                "Pinned",
                "Ready for review",
                "Needs input",
                "Working",
                "Completed",
                "Asleep",
                "Shelved"
            ]
        );
    }

    #[test]
    fn header_counts_a_group_in_a_word_the_list_can_be_narrowed_by() {
        // Every counter word is a word `s:` accepts, so the header documents the
        // filter.
        let fleet = || {
            vec![
                view("pinned-a1b", Phase::Working, 10),
                on_a_branch(view("review-b2c", Phase::Done, 20), "amx/fix-login-a1b"),
                view("ask-c3d", Phase::Waiting, 30),
                view("busy-d4e", Phase::Working, 40),
                view("done-e5f", Phase::Done, 50),
                view("nap-f6g", Phase::Working, 60),
                view("away-g7h", Phase::Done, 70),
            ]
        };
        let one_of_each = [
            (Group::Pinned, "pinned-a1b"),
            (Group::Review, "review-b2c"),
            (Group::NeedsInput, "ask-c3d"),
            (Group::Working, "busy-d4e"),
            (Group::Completed, "done-e5f"),
            (Group::Asleep, "nap-f6g"),
            (Group::Shelved, "away-g7h"),
        ];
        assert_eq!(
            one_of_each.len(),
            Group::ALL.len(),
            "a group with nobody in it here is a group this proves nothing about"
        );

        for (group, id) in one_of_each {
            let mut list = sleeping(pinning(over_the_forge(fleet()), "pinned-a1b"), "nap-f6g");
            list.arrange(Arrangement {
                shelved: [("away-g7h".to_string(), Shelf::by_hand(80))].into(),
                ..list.arrangement()
            });
            list.narrow(vec![Narrow::State(Some(group.state().to_string()))]);
            assert_eq!(
                lines(&list),
                [format!("{} (1)", group.title()), id.to_string()],
                "narrowing to {} must leave the group its own counter names",
                group.state()
            );
        }
    }

    #[test]
    fn a_state_word_still_narrows_to_the_rows_in_that_state() {
        // Phase words that are not group words (such as `failed`) still narrow to
        // their rows, so Completed can be narrowed inside.
        let fleet = || {
            vec![
                view("done-a1b", Phase::Done, 10),
                view("failed-b2c", Phase::Failed, 20),
                view("idle-c3d", Phase::Idle, 30),
                view("stopped-d4e", Phase::Stopped, 40),
            ]
        };

        let mut list = listed(fleet());
        list.narrow(vec![Narrow::State(Some("failed".to_string()))]);
        assert_eq!(
            lines(&list),
            ["Completed (1)", "failed-b2c"],
            "a state word keeps the rows in that state and nothing else"
        );

        let mut list = listed(fleet());
        list.narrow(vec![Narrow::State(Some("idle".to_string()))]);
        assert_eq!(lines(&list), ["Completed (1)", "idle-c3d"]);

        let mut list = listed(fleet());
        list.narrow(vec![Narrow::State(Some("done".to_string()))]);
        assert_eq!(
            lines(&list),
            [
                "Completed (4)",
                "stopped-d4e",
                "idle-c3d",
                "failed-b2c",
                "done-a1b"
            ],
            "and the group's own word still keeps the whole group"
        );
    }

    #[test]
    fn header_counts_a_pinned_agent_that_is_asking_among_the_ones_asking() {
        let mut list = listed(vec![
            view("ask-a1b", Phase::Waiting, 10),
            view("busy-b2c", Phase::Working, 20),
        ]);
        assert_eq!(list.waiting(), 1);

        // The list opens on the waiting agent. Pinning it moves the row but not
        // the waiting count.
        assert!(list.hold_or_let_go());
        assert_eq!(
            lines(&list),
            ["Pinned (1)", "ask-a1b", "", "Working (1)", "busy-b2c"]
        );
        assert_eq!(list.counts(), [(Group::Pinned, 1), (Group::Working, 1)]);
        assert_eq!(
            list.waiting(),
            1,
            "where a row is drawn is not what it is waiting for"
        );
    }

    #[test]
    fn header_counts_the_agents_that_hold_a_slot_against_the_gate() {
        let mut list = listed(vec![
            view("busy-a1b", Phase::Working, 10),
            view("ask-b2c", Phase::Waiting, 20),
            view("done-c3d", Phase::Done, 30),
            view("stopped-d4e", Phase::Stopped, 40),
        ]);
        assert_eq!(
            list.live(),
            2,
            "an agent whose command has ended holds none"
        );

        list.narrow(vec![Narrow::State(Some("waiting".to_string()))]);
        assert_eq!(
            list.live(),
            2,
            "and the gate counts the fleet rather than what is on the screen"
        );

        // A gone pane already reads as stopped, which the spawn gate skips too.
        let mut gone = view("gone-e5f", Phase::Working, 50);
        gone.verdict.phase = Phase::Stopped;
        gone.verdict.evidence = Evidence::Gone;
        list.narrow(vec![Narrow::State(None)]);
        list.show(vec![view("busy-a1b", Phase::Working, 10), gone]);
        assert_eq!(list.live(), 1);

        // A parked agent keeps its phase but holds no pane, so the gate skips it.
        let mut parked = view("parked-f6a", Phase::Idle, 60);
        parked.verdict.evidence = Evidence::LetGo;
        list.show(vec![view("busy-a1b", Phase::Working, 10), parked]);
        assert_eq!(
            list.live(),
            1,
            "an agent whose pane amx took holds no slot either"
        );
    }

    #[test]
    fn a_heading_answers_for_its_agents_whether_or_not_they_are_drawn() {
        let mut list = listed(a_history(FOLD_AT + 2));
        list.up();

        let under = list.heading().expect("the cursor is on the heading");
        let members = |list: &List, under| -> Vec<String> {
            list.members(under)
                .iter()
                .map(|view| view.id().to_string())
                .collect()
        };
        assert_eq!(
            members(&list, under).len(),
            FOLD_AT + 2,
            "the fold decides how many rows are drawn, not how many there are"
        );

        list.shut_or_open();
        assert_eq!(
            members(&list, under).len(),
            FOLD_AT + 2,
            "and a group somebody shut is still standing for them"
        );

        list.narrow(vec![Narrow::Name(Some("done-4".to_string()))]);
        assert_eq!(
            members(&list, under),
            ["done-4"],
            "a heading may not answer for agents a narrowing put out of reach"
        );
    }

    #[test]
    fn a_heading_on_the_project_axis_answers_for_the_agents_under_it() {
        let mut list = over_the_disk(vec![
            at(view("ask-a1b", Phase::Waiting, 10), "/src/api"),
            at(view("done-b2c", Phase::Done, 20), "/src/api/cmd/serve"),
            at(view("busy-c3d", Phase::Working, 30), "/src/web"),
        ]);
        list.up();

        let under = list.heading().expect("the cursor is on the heading");
        assert_eq!(list.title(under), "/src/api");
        assert_eq!(
            list.members(under)
                .iter()
                .map(|view| view.id())
                .collect::<Vec<_>>(),
            ["ask-a1b", "done-b2c"],
            "a project stands for what runs in it, subdirectory and all"
        );
    }

    #[test]
    fn a_row_says_the_program_then_the_dials_the_spawn_turned() {
        let mut agent = view("fix-login-a1b", Phase::Working, 10);
        // The record holds the launch command; the column names its program.
        agent.meta.agent = Some("claude --dangerously-skip-permissions".to_string());
        agent.meta.model = Some("opus".to_string());
        agent.meta.effort = Some("high".to_string());
        assert_eq!(vendor_words(&agent.meta), "claude opus high");

        agent.meta.effort = None;
        assert_eq!(vendor_words(&agent.meta), "claude opus");

        agent.meta.model = None;
        assert_eq!(
            vendor_words(&agent.meta),
            "claude",
            "a dial nobody turned is the vendor's own and amx never saw it"
        );

        // Parts are listed in order as recorded, not in fixed slots.
        agent.meta.effort = Some("low".to_string());
        assert_eq!(vendor_words(&agent.meta), "claude low");
    }

    #[test]
    fn a_row_running_a_command_says_sh_where_the_vendor_would_be() {
        // `!cmd` and `--exec` records have no agent.
        let command = view("build-b2c", Phase::Working, 10);
        assert_eq!(command.meta.agent, None);
        assert_eq!(vendor_words(&command.meta), "sh");
    }

    #[test]
    fn a_row_takes_the_rename_then_the_sessions_title_then_the_id() {
        // All three names on one agent, to check precedence on a single row.
        let mut named = view("fix-login-a1b", Phase::Idle, 10);
        named.state.session_title = Some("Login timeout".to_string());
        named.state.name = Some("auth".to_string());
        let mut titled = view("port-importer-b2c", Phase::Idle, 20);
        titled.state.session_title = Some("Importer clock".to_string());
        let plain = view("old-job-c3d", Phase::Idle, 30);

        assert_eq!(called(&named), "auth");
        assert_eq!(
            called(&titled),
            "Importer clock",
            "and the title the session goes under where nobody has renamed it"
        );
        assert_eq!(
            called(&plain),
            "old-job-c3d",
            "and its id until there is either"
        );

        let mut list = listed(vec![named, titled, plain]);
        list.narrow(vec![Narrow::Name(Some("auth".to_string()))]);
        assert_eq!(
            lines(&list),
            ["Completed (1)", "fix-login-a1b"],
            "and a narrowing takes the name off the row as readily as the id"
        );
        list.narrow(vec![Narrow::Name(Some("clock".to_string()))]);
        assert_eq!(
            lines(&list),
            ["Completed (1)", "port-importer-b2c"],
            "and a word of the title reaches the row wearing it"
        );
    }

    #[test]
    fn find_looks_in_the_task_an_agent_was_started_on() {
        let mut porting = view("a1b", Phase::Idle, 10);
        porting.meta.task = "Port the importer to the new shape".to_string();
        let mut logging = view("b2c", Phase::Idle, 20);
        logging.meta.task = "fix the login bug".to_string();

        // The task is the one string on the record the user typed.
        let mut list = listed(vec![porting, logging]);
        list.narrow(vec![Narrow::Name(Some("importer".to_string()))]);
        assert_eq!(lines(&list), ["Completed (1)", "a1b"]);

        // Case-insensitive.
        list.narrow(vec![Narrow::Name(Some("PORT".to_string()))]);
        assert_eq!(lines(&list), ["Completed (1)", "a1b"]);

        list.narrow(vec![Narrow::Name(Some("LOGIN".to_string()))]);
        assert_eq!(lines(&list), ["Completed (1)", "b2c"]);
    }

    #[test]
    fn find_ignores_case_in_the_id_and_the_name_as_well() {
        let mut named = view("fix-login-a1b", Phase::Idle, 10);
        named.state.name = Some("Auth".to_string());
        let list = |want: &str| {
            let mut list = listed(vec![named.clone(), view("port-b2c", Phase::Idle, 20)]);
            list.narrow(vec![Narrow::Name(Some(want.to_string()))]);
            lines(&list)
        };

        assert_eq!(list("AUTH"), ["Completed (1)", "fix-login-a1b"]);
        assert_eq!(list("auth"), ["Completed (1)", "fix-login-a1b"]);
        assert_eq!(list("FIX-LOGIN"), ["Completed (1)", "fix-login-a1b"]);
    }

    #[test]
    fn pr_the_row_carries_what_the_agents_branch_has_open() {
        let list = over_the_forge(vec![
            on_a_branch(view("fix-login-a1b", Phase::Done, 10), "amx/fix-login-a1b"),
            view("no-branch-c3d", Phase::Done, 20),
        ]);

        let labelled = list.agent_by_id("fix-login-a1b").unwrap();
        assert_eq!(list.requests(labelled)[0].label(), "#12");
        assert_eq!(list.requests(labelled)[0].standing, Standing::Failing);
        assert!(
            list.requests(list.agent_by_id("no-branch-c3d").unwrap())
                .is_empty(),
            "an agent amx cut no branch for has nothing to label"
        );
    }

    #[test]
    fn pr_narrows_the_list_to_the_request_a_line_named() {
        let mut list = over_the_forge(vec![
            on_a_branch(view("fix-login-a1b", Phase::Idle, 10), "amx/fix-login-a1b"),
            on_a_branch(
                view("port-importer-b2c", Phase::Idle, 20),
                "amx/port-importer-b2c",
            ),
        ]);

        // The pull request number may be the only name the user has for the agent.
        list.narrow(vec![Narrow::Name(Some("#12".to_string()))]);
        assert_eq!(lines(&list), ["Ready for review (1)", "fix-login-a1b"]);
        assert_eq!(list.counts(), [(Group::Review, 1)]);

        list.narrow(vec![Narrow::Name(Some("#3".to_string()))]);
        assert_eq!(lines(&list), ["Ready for review (1)", "port-importer-b2c"]);

        list.narrow(vec![Narrow::Name(Some("#99".to_string()))]);
        assert!(
            list.is_empty(),
            "and a number nobody's branch wears finds nobody"
        );
    }

    #[test]
    fn pr_is_read_again_with_every_reading() {
        // Pull request status changes while the row is on screen, so it is read
        // on every reading.
        let mut list = over_the_forge(vec![view("fix-login-a1b", Phase::Working, 10)]);
        assert!(
            list.requests(list.agent_by_id("fix-login-a1b").unwrap())
                .is_empty()
        );

        list.show(vec![on_a_branch(
            view("fix-login-a1b", Phase::Working, 10),
            "amx/fix-login-a1b",
        )]);
        assert_eq!(
            list.requests(list.agent_by_id("fix-login-a1b").unwrap())[0].number,
            12
        );

        // A departed agent's entry is dropped.
        list.show(vec![view("port-importer-b2c", Phase::Working, 20)]);
        assert!(list.agent_by_id("fix-login-a1b").is_none());
        assert_eq!(list.prs.len(), 1, "{:?}", list.prs);
    }

    #[test]
    fn view_with_nothing_in_it_has_nothing_to_select() {
        let list = listed(Vec::new());
        assert!(list.is_empty());
        assert!(list.items().is_empty());
        assert!(list.selected().is_none());
        assert!(!list.on_fold());
    }
}
