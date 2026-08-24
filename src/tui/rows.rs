//! What the view lists, and where the cursor is in it.
//!
//! A list of agents is not a table with a sort order. What somebody opens this
//! for is one question — *is anything waiting on me?* — so the agents are
//! gathered under the answer: the ones that have stopped on a question first,
//! then the ones mid-turn, then the ones sitting at their prompt, then the ones
//! whose command has ended.
//!
//! Inside a group the order is the order agents were started in, which is the
//! one order that does not move under a cursor while somebody is reading. The
//! exception is the finished group, where the newest ending comes first and the
//! rest fold away behind a count: a week of finished agents is history, and
//! history belongs under a fold rather than in front of the live ones.
//!
//! There is a second question a wall of agents gets asked — *what is running in
//! this repository?* — and it is the same agents gathered a different way, so
//! it is an axis rather than a screen. Under it the headings are projects and
//! every row carries the state the heading used to say. Both axes draw the
//! agents in one order, so turning the axis never changes who a row's
//! neighbours are.
//!
//! Either axis can be narrowed to part of the fleet. A hidden agent is not a
//! member of anything: nothing counts it, no heading is drawn for a group it
//! was the last of, and the cursor cannot land on it.
//!
//! A heading is a line of the list like the rows under it: the cursor stops on
//! one, and shutting it puts its agents away and leaves the heading standing
//! for them. What was shut is remembered against the group itself rather than
//! against a line number, because the list is laid out again every second and
//! line four is somebody else's by then.
//!
//! An order the list works out is an order somebody may disagree with, so two
//! things are theirs to say: which agent is held at the top of its group, and
//! what order the rest of that group goes in. Both are said against the agents
//! and the group rather than against the screen, which is what lets them
//! outlive the view they were said in.

use crate::derive::View;
use crate::pr::{self, Pr};
use crate::store::{Ask, Meta, Phase};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

/// How many finished agents are shown before the rest fold into a count.
pub const FOLD: usize = 3;

/// What an agent is, to somebody deciding what to do next.
///
/// Written down as the word it is titled with, because an order somebody put a
/// group in is kept against the group and read back by a later view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Group {
    /// Stopped on a question: nothing happens until somebody answers it.
    NeedsInput,
    /// Mid-turn. Nothing to do but let it work.
    Working,
    /// Sitting at its prompt with the turn over — and, with it, the agent amx
    /// cannot account for. Both are quiet, and only one of them is quiet for a
    /// reason anybody has vouched for, which is why the row says which.
    Idle,
    /// The command ended, one way or another.
    Completed,
}

impl Group {
    /// Every group, in the order a person reads them.
    pub const ALL: [Group; 4] = [
        Group::NeedsInput,
        Group::Working,
        Group::Idle,
        Group::Completed,
    ];

    /// Which group a state belongs to.
    pub fn of(phase: Phase) -> Group {
        match phase {
            Phase::Waiting => Group::NeedsInput,
            Phase::Starting | Phase::Working => Group::Working,
            Phase::Idle | Phase::Unknown => Group::Idle,
            Phase::Done | Phase::Failed | Phase::Stopped => Group::Completed,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Group::NeedsInput => "needs input",
            Group::Working => "working",
            Group::Idle => "idle",
            Group::Completed => "completed",
        }
    }

    /// The state that stands for the group where a count of it is being read
    /// rather than a heading over rows.
    ///
    /// Two words for one group, and the second earns its keep: a heading says
    /// what the group means to somebody scanning the list, and a counter says
    /// the word `s:` takes for it, so the header teaches the language the list
    /// is narrowed in by existing. Every one of these is a state an agent can
    /// actually be in — a counter naming a word nothing matches would send
    /// somebody to an empty list.
    pub fn state(self) -> &'static str {
        match self {
            Group::NeedsInput => "waiting",
            Group::Working => "working",
            Group::Idle => "idle",
            Group::Completed => "done",
        }
    }
}

/// Which way the agents are gathered.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Axis {
    /// Under what they need, which is what somebody opens the view for.
    #[default]
    State,
    /// Under the project they are running in.
    Project,
}

/// How somebody has arranged the list, in terms that outlive the view they
/// arranged it in: which way it is gathered, the agents held at the top of
/// their group, and the order a group was put in.
///
/// Agents by id and groups by name, because that is what a later view has to
/// find them by. An id in here that no longer names an agent costs a lookup
/// that misses, which is what a view opened on a fleet that has moved on
/// should cost.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Arrangement {
    axis: Axis,
    held: BTreeSet<String>,
    order: BTreeMap<Group, Vec<String>>,
}

/// What a heading stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Under {
    /// A state, on the state axis.
    Group(Group),
    /// The project at this place in the list's own table of them. An index
    /// rather than the path itself, so a line of the list stays a small copied
    /// value.
    Project(usize),
}

/// What a heading is answerable for: the agents gathered under it, the
/// failures among them, and whether they are on the screen or put away behind
/// it.
///
/// The counts are what a narrowing left, always: a heading may not claim
/// members that opening it could not reach.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tally {
    pub members: usize,
    pub failures: usize,
    pub shut: bool,
}

/// One line of the list. Every one of them but the blank is a place the
/// cursor can stop; the blank is spacing, and the cursor walks over it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Item {
    /// A heading, and what it answers for.
    Heading(Under, Tally),
    /// The agent at this position of the reading behind the list.
    Agent(usize),
    /// How many finished agents the fold is holding back.
    Fold(usize),
    /// The line that stands a heading off from the group above it.
    Blank,
}

/// A heading in terms that outlive the next reading. `Under` holds a project's
/// place in a table that is built again every second, and what somebody shut
/// has to be remembered against something that does not move under them.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Key {
    Group(Group),
    Project(PathBuf),
}

/// What the cursor is on, in the same terms and for the same reason.
enum On {
    Agent(String),
    Heading(Key),
    Nothing,
}

/// One narrowing, as the change it makes. A line only changes what it names,
/// so `a:port` on its own leaves the state narrowing where it was, and `s:`
/// with nothing after it drops that one and leaves the rest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Narrow {
    State(Option<String>),
    Name(Option<String>),
}

/// What the list is narrowed to. Every one that is set has to match, and
/// nothing set keeps everything.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Filters {
    state: Option<String>,
    name: Option<String>,
}

impl Filters {
    fn keeps(&self, view: &View, prs: &[Pr]) -> bool {
        let state = self
            .state
            .as_ref()
            .is_none_or(|want| view.phase().as_str() == want);
        // Every word for the agent that is on the screen somebody is typing
        // at: the id every other surface uses, the name a person gave it
        // because the id was not what they call it, and the `#12` its branch
        // wears — which is routinely the only one of the three a person has in
        // front of them, because they came to the wall from the pull request.
        let name = self.name.as_ref().is_none_or(|want| {
            view.id().contains(want)
                || called(view).contains(want)
                || prs.iter().any(|pr| pr.label().contains(want))
        });
        state && name
    }

    /// What was typed, read back.
    fn label(&self) -> Option<String> {
        let said: Vec<String> = [
            self.state.as_ref().map(|want| format!("s:{want}")),
            self.name.as_ref().map(|want| format!("a:{want}")),
        ]
        .into_iter()
        .flatten()
        .collect();
        (!said.is_empty()).then(|| said.join(" "))
    }
}

/// The agents, as lines with a cursor on one of them.
#[derive(Debug)]
pub struct List {
    views: Vec<View>,
    items: Vec<Item>,
    cursor: usize,
    /// Whether the cursor has been put on anything yet, which is what tells a
    /// view that has just opened from one somebody is reading.
    landed: bool,
    unfolded: bool,
    /// The groups somebody has shut, by what they stand for.
    shut: HashSet<Key>,
    /// The agents somebody is holding at the top of their group.
    held: BTreeSet<String>,
    /// The order somebody put a group in, as the ids of the agents that were
    /// under it when they said so.
    order: BTreeMap<Group, Vec<String>>,
    axis: Axis,
    filters: Filters,
    /// The projects the headings name, in the order they are drawn.
    projects: Vec<PathBuf>,
    /// Which project each agent belongs to, worked out once per agent: the
    /// reading is taken again every second, and the walk below reaches a disk.
    /// An agent's directory does not move under it, so one answer per id is one
    /// answer for as long as the view is open.
    roots: HashMap<String, PathBuf>,
    /// Whether a directory holds a repository. A field so that a test can say
    /// what the disk looks like, and count what was asked of it.
    probe: fn(&Path) -> bool,
    /// What each agent's branch has open, by id. Taken with the reading rather
    /// than once per agent, because a check goes green while somebody is
    /// looking at the row — the look itself is a small file beside the record,
    /// and the forge is asked from a thread nobody waits on.
    prs: HashMap<String, Vec<Pr>>,
    /// Where those come from. A field for the same reason `probe` is one: a
    /// test says what the forge holds without one being anywhere near it.
    asks: fn(&Meta) -> Vec<Pr>,
    /// Home as this view knows it, read once: a heading says `~/code/amx` the
    /// way a person writes it, and `$HOME` does not move while they read.
    home: Option<PathBuf>,
}

impl Default for List {
    fn default() -> List {
        List {
            views: Vec::new(),
            items: Vec::new(),
            cursor: 0,
            landed: false,
            unfolded: false,
            shut: HashSet::new(),
            held: BTreeSet::new(),
            order: BTreeMap::new(),
            axis: Axis::default(),
            filters: Filters::default(),
            projects: Vec::new(),
            roots: HashMap::new(),
            probe: holds_a_repository,
            prs: HashMap::new(),
            asks: pr::of,
            home: std::env::home_dir(),
        }
    }
}

impl List {
    /// The same list over a stated disk and home, which is the seam the walk
    /// below and the abbreviation above are proven at.
    #[cfg(test)]
    fn probing(probe: fn(&Path) -> bool, home: Option<PathBuf>) -> List {
        List {
            probe,
            home,
            ..List::default()
        }
    }

    /// The same list over a stated forge, which is the seam the label and the
    /// narrowing that finds it by number are proven at.
    #[cfg(test)]
    pub(super) fn asking(&mut self, asks: fn(&Meta) -> Vec<Pr>) {
        self.asks = asks;
    }

    /// Take a fresh reading.
    ///
    /// The cursor holds onto what it was on rather than the line number it was
    /// at: agents change groups while somebody is looking at them, and a
    /// cursor that stayed on line four would end up on whoever moved into it.
    pub fn show(&mut self, views: Vec<View>) {
        let on = self.on();
        self.remember_the_requests(&views);
        self.views = views;
        self.rebuild();
        self.follow(&on);
    }

    /// What each agent's branch has open, taken again with the reading.
    ///
    /// Every agent every time, unlike the projects: which repository an agent
    /// runs in does not move under it, and what its pull request is doing is
    /// the thing on the row most likely to have changed since the last look.
    fn remember_the_requests(&mut self, views: &[View]) {
        self.prs = views
            .iter()
            .map(|view| (view.id().to_string(), (self.asks)(&view.meta)))
            .collect();
    }

    /// What this agent's branch has open, in the order a surface reads them.
    pub fn requests(&self, view: &View) -> &[Pr] {
        self.prs.get(view.id()).map_or(&[], Vec::as_slice)
    }

    pub fn axis(&self) -> Axis {
        self.axis
    }

    /// Gather them the other way. The cursor holds its agent across the turn,
    /// because turning the axis is a question about the fleet and not about
    /// the one agent somebody was looking at.
    pub fn turn(&mut self) {
        let on = self.on();
        self.axis = match self.axis {
            Axis::State => Axis::Project,
            Axis::Project => Axis::State,
        };
        self.rebuild();
        self.follow(&on);
    }

    /// How the list stands arranged, to be kept and given back to the next
    /// view that opens.
    pub fn arrangement(&self) -> Arrangement {
        Arrangement {
            axis: self.axis,
            held: self.held.clone(),
            order: self.order.clone(),
        }
    }

    /// Put the list back the way it was arranged. The cursor holds what it was
    /// on, for the same reason it does across a turn of the axis.
    pub fn arrange(&mut self, arrangement: Arrangement) {
        let on = self.on();
        self.axis = arrangement.axis;
        self.held = arrangement.held;
        self.order = arrangement.order;
        self.rebuild();
        self.follow(&on);
    }

    /// Whether this agent is one somebody is holding at the top of its group.
    pub fn holding(&self, view: &View) -> bool {
        self.held.contains(view.id())
    }

    /// Hold the agent under the cursor at the top of its group, or let it go.
    ///
    /// About the agent and not about the group: an agent that is held stays
    /// held when it moves group, because what somebody said is that this agent
    /// is the one they want in front of them.
    ///
    /// Answers whether there was an agent to do it to, which is what tells a
    /// key pressed on a heading from a key that changed something.
    pub fn hold_or_let_go(&mut self) -> bool {
        let Some(id) = self.selected().map(|view| view.id().to_string()) else {
            return false;
        };
        if !self.held.remove(&id) {
            self.held.insert(id);
        }
        let on = self.on();
        self.rebuild();
        self.follow(&on);
        true
    }

    /// Move the agent under the cursor a row up or down its own group.
    ///
    /// The whole group's order is written down, not the one move: an order is
    /// a sequence, and half of one would leave the agents nobody moved with
    /// nothing said about where they go. An agent that arrives afterwards is
    /// not in it and sits under the ones that are — a group somebody has
    /// arranged by hand is not a group amx goes on sorting under them.
    ///
    /// What a narrowing was hiding is not in it either, for the same reason it
    /// is not on the screen: an arrangement is made of the agents it was made
    /// among.
    pub fn move_by(&mut self, by: isize) -> bool {
        let Some(view) = self.selected() else {
            return false;
        };
        let id = view.id().to_string();
        let group = Group::of(view.phase());
        let mut members: Vec<String> = self
            .ordered()
            .into_iter()
            .filter(|&n| Group::of(self.views[n].phase()) == group)
            .map(|n| self.views[n].id().to_string())
            .collect();

        let Some(at) = members.iter().position(|other| *other == id) else {
            return false;
        };
        let Some(to) = at.checked_add_signed(by).filter(|to| *to < members.len()) else {
            return false;
        };
        // A held agent is above the rest by the holding, so a move across that
        // line is one the list could not draw: it is refused rather than
        // written down and then ignored.
        if self.held.contains(&members[to]) != self.held.contains(&id) {
            return false;
        }
        // And the rows a move can reach are the rows on the screen. A fold
        // holds history back, and an agent moved behind one would go where the
        // cursor could not follow it, leaving somebody's cursor on whoever
        // came up in its place.
        if !self.drawn(&members[to]) {
            return false;
        }

        members.swap(at, to);
        self.order.insert(group, members);
        let on = self.on();
        self.rebuild();
        self.follow(&on);
        true
    }

    /// Whether this agent has a row on the screen, as against being counted by
    /// a heading that is shut or held back by a fold.
    fn drawn(&self, id: &str) -> bool {
        self.items
            .iter()
            .any(|item| self.agent(*item).is_some_and(|view| view.id() == id))
    }

    /// Where an agent sits in the order somebody put its group in, and past
    /// the end of it for one nobody has placed.
    fn seat(&self, n: usize) -> usize {
        let view = &self.views[n];
        self.order
            .get(&Group::of(view.phase()))
            .and_then(|ids| ids.iter().position(|id| id == view.id()))
            .unwrap_or(usize::MAX)
    }

    /// Narrow the list to part of the fleet, changing only what was named.
    pub fn narrow(&mut self, changes: Vec<Narrow>) {
        for change in changes {
            match change {
                Narrow::State(state) => self.filters.state = state,
                Narrow::Name(name) => self.filters.name = name,
            }
        }
        self.rebuild();
    }

    /// What the list is narrowed to, in the words it was narrowed with, so
    /// somebody who has forgotten why it is short can read why.
    pub fn narrowing(&self) -> Option<String> {
        self.filters.label()
    }

    /// What a heading says.
    pub fn title(&self, under: Under) -> String {
        match under {
            Under::Group(group) => group.title().to_string(),
            Under::Project(n) => match self.projects.get(n) {
                Some(root) => shorten(root, self.home.as_deref()),
                None => String::new(),
            },
        }
    }

    pub fn items(&self) -> &[Item] {
        &self.items
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// The agent a line stands for, if it stands for one.
    pub fn agent(&self, item: Item) -> Option<&View> {
        match item {
            Item::Agent(n) => self.views.get(n),
            _ => None,
        }
    }

    /// The agent the cursor is on.
    pub fn selected(&self) -> Option<&View> {
        self.agent(*self.items.get(self.cursor)?)
    }

    /// Whether the cursor is on the fold rather than on an agent.
    pub fn on_fold(&self) -> bool {
        matches!(self.items.get(self.cursor), Some(Item::Fold(_)))
    }

    /// Whether the cursor is on a heading rather than on anything under one.
    pub fn on_heading(&self) -> bool {
        matches!(self.items.get(self.cursor), Some(Item::Heading(..)))
    }

    /// What the heading under the cursor stands for, where it is on one.
    pub fn heading(&self) -> Option<Under> {
        match self.items.get(self.cursor) {
            Some(Item::Heading(under, _)) => Some(*under),
            _ => None,
        }
    }

    /// The heading of the group holding the cursor — the one the bold section
    /// highlight marks — where there is a second heading for it to be told
    /// apart from. A lone heading is the only place the cursor could be, so
    /// it has nothing to say and nobody is named.
    pub fn section(&self) -> Option<usize> {
        let heading = |item: &Item| matches!(item, Item::Heading(..));
        if self.items.iter().filter(|item| heading(item)).count() < 2 {
            return None;
        }
        self.items.iter().take(self.cursor + 1).rposition(heading)
    }

    /// The agents a heading answers for, in the order they are drawn.
    ///
    /// Whether or not they are on the screen: a group somebody shut is still
    /// standing for them and the fold only decides how many rows are drawn, so
    /// an act on a heading reaches what the heading's own count claims. What a
    /// narrowing put out of reach is not among them, for the same reason it is
    /// not in the count.
    pub fn members(&self, under: Under) -> Vec<&View> {
        self.ordered()
            .into_iter()
            .filter(|&n| self.belongs(n, under))
            .map(|n| &self.views[n])
            .collect()
    }

    /// The reading of one agent by id, for an act decided on one screen and
    /// carried out on the next.
    pub fn agent_by_id(&self, id: &str) -> Option<&View> {
        self.views.iter().find(|view| view.id() == id)
    }

    /// Whether a narrowing left this agent on the screen, with everything on
    /// its row that a narrowing may be written against.
    fn keeps(&self, view: &View) -> bool {
        self.filters.keeps(view, self.requests(view))
    }

    /// Whether an agent is drawn under this heading.
    fn belongs(&self, n: usize, under: Under) -> bool {
        match under {
            Under::Group(group) => Group::of(self.views[n].phase()) == group,
            Under::Project(at) => self
                .projects
                .get(at)
                .is_some_and(|root| *root == self.root_of(n)),
        }
    }

    /// Put the group the cursor is on away, or bring it back. The heading
    /// stays either way: it is what stands for the agents while they are gone,
    /// and what somebody presses again to have them back.
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
        self.rebuild();
        self.follow(&on);
    }

    /// Whether this is a fleet nobody has started, rather than one a narrowing
    /// has emptied or a list of the places nobody is running anything.
    ///
    /// The one case a view has anything of its own to say about an empty
    /// screen. Somebody who narrowed the list to nothing is owed the words
    /// they typed back, and the project axis is a list of places, which nobody
    /// arrives at without agents to arrange.
    pub fn unstarted(&self) -> bool {
        self.axis == Axis::State && self.views.is_empty() && self.filters.label().is_none()
    }

    /// Show the finished agents the fold was holding back, and keep showing
    /// them: somebody who opened it is going through them.
    pub fn unfold(&mut self) {
        self.unfolded = true;
        self.rebuild();
    }

    /// How many agents are in each state that has any, whichever way they are
    /// gathered: what there is does not depend on how it was laid out.
    pub fn counts(&self) -> Vec<(Group, usize)> {
        Group::ALL
            .into_iter()
            .filter_map(|group| {
                let count = self
                    .views
                    .iter()
                    .filter(|view| self.keeps(view))
                    .filter(|view| Group::of(view.phase()) == group)
                    .count();
                (count > 0).then_some((group, count))
            })
            .collect()
    }

    /// Whether there is nothing on the screen — which is not the same as
    /// having no agents, once a narrowing can hide every one of them.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// How many agents are holding a slot against the spawn gate.
    ///
    /// The whole fleet's worth, whatever the list was narrowed to: the gate
    /// counts agents, and an agent somebody has filtered off the screen is
    /// still in the way of the next one.
    ///
    /// Counted off the reading rather than by asking tmux again, and it is the
    /// same answer: the gate skips an agent whose pane has gone, and a reading
    /// lists the panes once per server and has already settled such an agent
    /// as stopped.
    pub fn live(&self) -> usize {
        self.views
            .iter()
            .filter(|view| !view.phase().is_terminal())
            .count()
    }

    pub fn down(&mut self) {
        self.step(1);
    }

    pub fn up(&mut self) {
        self.step(-1);
    }

    /// Move to the next line, staying put at the ends. Every line is a stop,
    /// headings included — a group is a thing somebody does something to —
    /// except the blank over a heading, which the cursor walks straight over.
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

    /// Lay the reading out as lines.
    fn rebuild(&mut self) {
        self.remember_the_roots();
        let order = self.ordered();
        match self.axis {
            Axis::State => {
                self.projects.clear();
                self.items = self.by_state(&order);
            }
            Axis::Project => {
                let (projects, items) = self.by_project(&order);
                self.projects = projects;
                self.items = items;
            }
        }
        self.settle();
    }

    /// Which project each agent belongs to, for the ones not worked out yet.
    /// Only on the axis that asks, because the walk reaches a disk.
    fn remember_the_roots(&mut self) {
        if self.axis != Axis::Project {
            return;
        }
        let fresh: Vec<(String, PathBuf)> = self
            .views
            .iter()
            .filter(|view| !self.roots.contains_key(view.id()))
            .map(|view| (view.id().to_string(), project_of(&view.meta, self.probe)))
            .collect();
        self.roots.extend(fresh);
    }

    /// Every agent a narrowing left, in the one order both axes draw them in:
    /// by what they need, and inside that whatever somebody said — the ones
    /// they are holding at the top, then the order they put the rest in, then
    /// the order the agents were started in, except the finished ones, where
    /// the newest ending comes first because what just finished is what
    /// somebody scanning them came for.
    ///
    /// One order for both axes is what keeps a row's neighbours its own: an
    /// agent does not change who it sits beside merely because the fleet was
    /// gathered a different way. It is also what a hand-made order means here:
    /// somebody arranging the list is arranging the fleet, not one screen of
    /// it.
    fn ordered(&self) -> Vec<usize> {
        let mut order: Vec<usize> = (0..self.views.len())
            .filter(|&n| self.keeps(&self.views[n]))
            .collect();
        order.sort_by(|&a, &b| {
            rank(&self.views[a])
                .cmp(&rank(&self.views[b]))
                // Held first, and held agents among themselves by everything
                // that orders the rest.
                .then_with(|| {
                    self.holding(&self.views[b])
                        .cmp(&self.holding(&self.views[a]))
                })
                .then_with(|| self.seat(a).cmp(&self.seat(b)))
                .then_with(|| match Group::of(self.views[a].phase()) {
                    Group::Completed => ended(&self.views[b])
                        .cmp(&ended(&self.views[a]))
                        .then_with(|| self.views[a].id().cmp(self.views[b].id())),
                    // A stable sort, so everything else keeps the order it was
                    // read in, which is the order the agents were started in.
                    _ => Ordering::Equal,
                })
        });
        order
    }

    /// One heading per state that has anybody under it.
    fn by_state(&self, order: &[usize]) -> Vec<Item> {
        let mut items = Vec::new();
        for group in Group::ALL {
            let members: Vec<usize> = order
                .iter()
                .copied()
                .filter(|&n| Group::of(self.views[n].phase()) == group)
                .collect();
            if members.is_empty() {
                continue;
            }

            let shut = self.shut.contains(&Key::Group(group));
            if !items.is_empty() {
                items.push(Item::Blank);
            }
            items.push(Item::Heading(
                Under::Group(group),
                self.tally(&members, shut),
            ));
            if shut {
                continue;
            }

            let shown = if group == Group::Completed && !self.unfolded {
                FOLD.min(members.len())
            } else {
                members.len()
            };
            items.extend(members[..shown].iter().map(|&n| Item::Agent(n)));
            if shown < members.len() {
                items.push(Item::Fold(members.len() - shown));
            }
        }
        items
    }

    /// What a heading answers for. The failures are counted whether the group
    /// is open or shut: a group says how many of its agents failed even while
    /// their rows are on the screen, because the count is what somebody
    /// scanning a screenful of headings reads instead of the rows.
    fn tally(&self, members: &[usize], shut: bool) -> Tally {
        Tally {
            members: members.len(),
            failures: members
                .iter()
                .filter(|&&n| self.views[n].phase() == Phase::Failed)
                .count(),
            shut,
        }
    }

    /// One heading per project somebody has an agent in.
    ///
    /// Projects are ordered by what their most urgent agent needs and then by
    /// where they are: a question at the bottom of a quiet repository is still
    /// a question, and two equally quiet repositories go by path. Nothing
    /// folds here — the fold holds back history, and history is the completed
    /// group rather than a place on a disk.
    fn by_project(&self, order: &[usize]) -> (Vec<PathBuf>, Vec<Item>) {
        let mut roots: Vec<(PathBuf, Vec<usize>)> = Vec::new();
        for &n in order {
            let root = self.root_of(n);
            match roots.iter_mut().find(|(at, _)| at == &root) {
                Some((_, members)) => members.push(n),
                None => roots.push((root, vec![n])),
            }
        }

        // `order` is already the reading order, so a project's first agent is
        // its most urgent one, and that is what the project sorts by.
        roots.sort_by(|(here, ours), (there, theirs)| {
            rank(&self.views[ours[0]])
                .cmp(&rank(&self.views[theirs[0]]))
                .then_with(|| here.cmp(there))
        });

        let mut projects = Vec::new();
        let mut items = Vec::new();
        for (root, members) in roots {
            let shut = self.shut.contains(&Key::Project(root.clone()));
            if !items.is_empty() {
                items.push(Item::Blank);
            }
            items.push(Item::Heading(
                Under::Project(projects.len()),
                self.tally(&members, shut),
            ));
            if !shut {
                items.extend(members.into_iter().map(Item::Agent));
            }
            projects.push(root);
        }
        (projects, items)
    }

    /// Which project an agent is drawn under: the walk's answer where it has
    /// one, and where the agent runs where it has not.
    fn root_of(&self, n: usize) -> PathBuf {
        self.roots
            .get(self.views[n].id())
            .cloned()
            .unwrap_or_else(|| self.views[n].meta.dir.clone())
    }

    /// What a heading stands for, in terms that outlive the next reading.
    fn key(&self, under: Under) -> Option<Key> {
        match under {
            Under::Group(group) => Some(Key::Group(group)),
            Under::Project(n) => self.projects.get(n).cloned().map(Key::Project),
        }
    }

    /// What the cursor is on now.
    fn on(&self) -> On {
        match self.items.get(self.cursor) {
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

    /// Put the cursor back on what it was on, where that is still drawn.
    fn follow(&mut self, held: &On) {
        let found = match held {
            On::Agent(id) => self
                .items
                .iter()
                .position(|item| self.agent(*item).is_some_and(|view| view.id() == id)),
            On::Heading(key) => self.items.iter().position(|item| match item {
                Item::Heading(under, _) => self.key(*under).as_ref() == Some(key),
                _ => false,
            }),
            On::Nothing => None,
        };
        if let Some(at) = found {
            self.cursor = at;
        }
    }

    /// Put the cursor somewhere there is a line: where it is, else the last
    /// line there is.
    fn settle(&mut self) {
        if self.items.is_empty() {
            self.cursor = 0;
            // Nothing to stand on. Whatever comes next is a view opening
            // again, as far as the cursor is concerned.
            self.landed = false;
            return;
        }
        if !self.landed {
            self.landed = true;
            // A view opens on an agent rather than on the heading over it:
            // somebody who opened it came for the agents, and the heading is
            // one step back up from the first of them.
            self.cursor = self
                .items
                .iter()
                .position(|item| matches!(item, Item::Agent(_)))
                .unwrap_or(0);
            return;
        }
        self.cursor = self.cursor.min(self.items.len() - 1);
        // A blank is not a stop. It only ever stands over a heading, so the
        // heading is what the cursor was nearest to.
        if matches!(self.items[self.cursor], Item::Blank) {
            self.cursor = (self.cursor + 1).min(self.items.len() - 1);
        }
    }
}

/// What a row calls its agent: the name somebody gave it, and the id until
/// somebody does.
///
/// Here rather than on the reading itself, because it is a fact about the row:
/// the record is filed under the id, every verb takes the id, and the name is
/// the word this one screen shows instead.
pub fn called(view: &View) -> &str {
    view.state.name.as_deref().unwrap_or_else(|| view.id())
}

/// Whether this row is holding something nobody has read.
///
/// Two halves, and both are needed. An agent that is starting or mid-turn has
/// nothing for anybody to have read: what it is doing is on the row already
/// and it is different by the next reading. Everything else has stopped —
/// on a question, at its prompt, or for good — and what it stopped with is
/// worth a mark until somebody has been to look at it.
///
/// Read against the clock rather than against a flag, so the mark comes back
/// on its own: an agent that was read at its prompt and then stopped on a
/// question has said something since the last look at it.
///
/// Against the last thing it said and not against the end of its run: an
/// answer routinely lands after the exit is recorded, and a row holding one
/// nobody has read is exactly what the mark is for.
pub fn unread(view: &View) -> bool {
    Group::of(view.phase()) != Group::Working && view.state.seen < said(view)
}

/// When the agent last said anything, as well as the record can say.
fn said(view: &View) -> u64 {
    view.state.last_event.max(view.state.since)
}

/// When an agent's run ended.
///
/// The stamp the ending wrote, where there is one. A record that has none is
/// dated from the last thing the agent said: an older amx wrote it, or the
/// pane went and nothing got to record an exit.
fn ended(view: &View) -> u64 {
    match view.state.ended {
        0 => said(view),
        at => at,
    }
}

/// The question an agent is showing, and where it comes in the call that asked
/// it.
///
/// A fact about the row for the same reason [`called`] is one: it is read off
/// the record and it is what a surface draws. None of it can be read off the
/// pane. `AskUserQuestion` draws its questions as tabs on one screen, and
/// measured against claude 2.1.240 the strip elides its own headers as the
/// pane narrows — at 24 columns the showing tab's name is an ellipsis and
/// nothing else. How many questions there are, what each is called, and
/// whether one takes more than one choice are in the payload and only there
/// (`docs/question-shapes.md`).
#[derive(Clone, Copy)]
pub struct Showing<'a> {
    /// The question on the screen, as the payload wrote it.
    pub ask: &'a Ask,
    /// Which of the call's questions it is, counting from one.
    pub at: usize,
    /// And how many the call holds.
    pub of: usize,
}

impl Showing<'_> {
    /// What the tab strip would call it, where the payload named it.
    pub fn header(&self) -> Option<&str> {
        self.ask.header.as_deref().filter(|word| !word.is_empty())
    }
}

/// What the record holds about the question this agent has stopped on, where
/// the call it came from was ever written down.
///
/// The question showing is the first with no answer on it, which is the same
/// rule the record itself uses: answering one does not end a call, so the tab
/// after it is what the vendor has on the screen.
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

/// Where an agent comes in the reading order.
fn rank(view: &View) -> usize {
    let group = Group::of(view.phase());
    Group::ALL
        .iter()
        .position(|other| *other == group)
        .unwrap_or(Group::ALL.len())
}

/// The shape `new` gives a worktree (`src/worktree.rs`).
const WORKTREES: &str = ".amx/worktrees";

/// Which project an agent is running in.
fn project_of(meta: &Meta, probe: fn(&Path) -> bool) -> PathBuf {
    // A worktree amx made says which repository it was cut from, in the shape
    // the record already holds: string work, and no disk at all. A worktree of
    // any other shape is one somebody moved or a record somebody edited, and
    // it is grouped by where it actually runs rather than by a guess.
    if let Some(tree) = &meta.worktree {
        return repo_of(tree).unwrap_or_else(|| meta.dir.clone());
    }

    // An agent started without a worktree records the directory it was asked
    // for, which is routinely a subdirectory of the repository. Without the
    // walk, an agent started in `<repo>/src` and a worktree agent of the same
    // repository would head two projects, splitting the one thing this axis
    // exists to gather.
    meta.dir
        .ancestors()
        // A relative directory ends its walk at the empty path, and asking
        // about that would ask about wherever the view happens to be running.
        .filter(|dir| !dir.as_os_str().is_empty())
        .find(|dir| probe(dir))
        .map(Path::to_path_buf)
        // Under no repository at all: its own directory, verbatim.
        .unwrap_or_else(|| meta.dir.clone())
}

/// The repository a worktree of amx's own shape was cut from, read backwards.
fn repo_of(tree: &Path) -> Option<PathBuf> {
    let repo = tree.parent()?.parent()?.parent()?;
    // Component-wise, so `<repo>/x.amx/worktrees/<id>` is not the match a
    // comparison of strings would have made it. An empty repository half names
    // nowhere, and a blank heading is worse than saying where it runs.
    let shaped = tree.parent()?.strip_prefix(repo) == Ok(Path::new(WORKTREES));
    (shaped && !repo.as_os_str().is_empty()).then(|| repo.to_path_buf())
}

/// Whether a directory is the top of a repository. An entry rather than a
/// directory, because a worktree's own `.git` is a file.
fn holds_a_repository(dir: &Path) -> bool {
    dir.join(".git").exists()
}

/// A path the way a person writes it, with home as `~`.
///
/// Shared with the header, which says where the next agent will run and must
/// not write a path a different way from the headings under it.
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

    /// A reading of one agent: the state it is in, and when it was last heard
    /// from.
    fn view(id: &str, phase: Phase, at: u64) -> View {
        View {
            meta: Meta {
                id: id.to_string(),
                task: "fix the login bug".to_string(),
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
            },
        }
    }

    /// The same reading, running somewhere else.
    fn at(mut view: View, dir: &str) -> View {
        view.meta.dir = PathBuf::from(dir);
        view
    }

    /// The same reading, on a branch of its own.
    fn on_a_branch(mut view: View, branch: &str) -> View {
        view.meta.branch = Some(branch.to_string());
        view
    }

    /// A forge where two of the branches have a request open, so the number
    /// on the row is read from something rather than made up here.
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
            _ => Vec::new(),
        }
    }

    /// A list reading that forge.
    fn over_the_forge(views: Vec<View>) -> List {
        let mut list = List::default();
        list.asking(a_forge);
        list.show(views);
        list
    }

    /// The same reading, in a worktree amx made for it.
    fn in_a_worktree(mut view: View, tree: &str) -> View {
        view.meta.dir = PathBuf::from(tree);
        view.meta.worktree = Some(PathBuf::from(tree));
        view
    }

    /// The list as a person reads it down the screen.
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
                Item::Agent(_) => list.agent(*item).unwrap().id().to_string(),
                Item::Fold(hidden) => format!("… {hidden} more"),
                Item::Blank => String::new(),
            })
            .collect()
    }

    fn listed(views: Vec<View>) -> List {
        let mut list = List::default();
        list.show(views);
        list
    }

    // Every directory the list asked about, in the order it asked. A thread
    // local, because a test has a thread to itself and the suite runs in
    // parallel.
    thread_local! {
        static ASKED: std::cell::RefCell<Vec<PathBuf>> = const { std::cell::RefCell::new(Vec::new()) };
    }

    /// A disk where two directories are repositories, which writes down what it
    /// was asked: "once per agent" is a claim about how often, and a claim
    /// about I/O that nothing counts is not a claim.
    fn a_disk_with_repos(dir: &Path) -> bool {
        ASKED.with_borrow_mut(|asked| asked.push(dir.to_path_buf()));
        dir == Path::new("/src/api") || dir == Path::new("/src/web")
    }

    fn asked() -> Vec<PathBuf> {
        ASKED.with_borrow(|asked| asked.clone())
    }

    /// A list over that disk, with a home to abbreviate against.
    fn over_the_disk(views: Vec<View>) -> List {
        ASKED.with_borrow_mut(|asked| asked.clear());
        let mut list = List::probing(a_disk_with_repos, Some(PathBuf::from("/home/dev")));
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
                "needs input (1)",
                "ask-c3d",
                "",
                "working (2)",
                "busy-a1b",
                "starting-e5f",
                "",
                "idle (1)",
                "idle-d4e",
                "",
                "completed (1)",
                "done-b2c",
            ],
            "and inside a group, the order they were started in"
        );
        assert_eq!(
            list.counts(),
            [
                (Group::NeedsInput, 1),
                (Group::Working, 2),
                (Group::Idle, 1),
                (Group::Completed, 1)
            ]
        );
    }

    #[test]
    fn view_puts_an_agent_it_cannot_account_for_among_the_quiet_ones() {
        // `unknown` is not a claim that anything is happening, so it does not
        // sit among the agents that are working. Its row says `unknown` and
        // how long it has been out of touch; the group only says nobody is
        // holding it up.
        let list = listed(vec![view("puzzling-a1b", Phase::Unknown, 10)]);
        assert_eq!(lines(&list), ["idle (1)", "puzzling-a1b"]);
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
            ["completed (3)", "stopped-c3d", "failed-b2c", "done-a1b"],
            "newest ending first"
        );
    }

    #[test]
    fn view_orders_the_finished_agents_by_when_their_run_ended() {
        // Something arrives after the exit is recorded: a hook that fired as
        // the pane went, an answer written down late. The newest ending is
        // still the newest ending, so the group goes by the stamp the ending
        // wrote rather than by whatever was written last.
        let mut early = view("done-a1b", Phase::Done, 100);
        early.state.ended = 100;
        early.state.last_event = 400;
        let mut late = view("done-b2c", Phase::Done, 300);
        late.state.ended = 300;

        let list = listed(vec![early, late]);
        assert_eq!(lines(&list), ["completed (2)", "done-b2c", "done-a1b"]);
    }

    #[test]
    fn view_folds_the_finished_agents_behind_a_count() {
        let mut list = listed(
            (0..5)
                .map(|n| view(&format!("done-{n}"), Phase::Done, 10 * n))
                .collect(),
        );

        assert_eq!(
            lines(&list),
            ["completed (5)", "done-4", "done-3", "done-2", "… 2 more"]
        );

        list.unfold();
        assert_eq!(lines(&list).len(), 6, "the fold line is gone with the fold");
        assert!(lines(&list).contains(&"done-0".to_string()));

        // And it stays open while more finish.
        list.show(
            (0..6)
                .map(|n| view(&format!("done-{n}"), Phase::Done, 10 * n))
                .collect(),
        );
        assert!(lines(&list).contains(&"done-0".to_string()));
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

        // The one above it answers and moves group, so line 3 is now somebody
        // else's.
        list.show(vec![
            view("ask-a1b", Phase::Idle, 10),
            view("busy-b2c", Phase::Working, 20),
        ]);
        assert_eq!(list.selected().unwrap().id(), "busy-b2c");

        // And when the agent it was on goes, the cursor lands on a line that
        // is still there.
        list.show(vec![view("ask-a1b", Phase::Idle, 10)]);
        assert_eq!(list.selected().unwrap().id(), "ask-a1b");
    }

    #[test]
    fn view_can_reach_the_fold_and_open_it_where_it_stands() {
        let mut list = listed(
            (0..5)
                .map(|n| view(&format!("done-{n}"), Phase::Done, 10 * n))
                .collect(),
        );

        for _ in 0..3 {
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
    fn axis_puts_the_project_whose_agent_is_waiting_first() {
        // Ordered by what each project's most urgent agent needs, so the
        // question at the bottom of a quiet repo is not buried under a busy
        // one, and projects that are equally quiet go by path.
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
    fn axis_reads_a_worktree_back_to_the_repository_it_was_cut_from() {
        // An agent in a worktree amx made is running in the repository that
        // worktree came out of, which is where somebody looking for "what is
        // happening in this repo" expects to find it.
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
    fn axis_turns_between_what_they_need_and_where_they_are() {
        let mut list = over_the_disk(vec![
            at(view("ask-a1b", Phase::Waiting, 10), "/src/api"),
            at(view("busy-b2c", Phase::Working, 20), "/src/web"),
        ]);
        assert_eq!(list.axis(), Axis::Project);

        list.turn();
        assert_eq!(list.axis(), Axis::State);
        assert_eq!(
            lines(&list),
            ["needs input (1)", "ask-a1b", "", "working (1)", "busy-b2c"]
        );

        list.turn();
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
    fn axis_keeps_the_finished_agents_folded_only_where_there_is_a_group_for_them() {
        let views: Vec<View> = (0..5)
            .map(|n| at(view(&format!("done-{n}"), Phase::Done, 10 * n), "/src/api"))
            .collect();
        let mut list = over_the_disk(views);

        assert_eq!(
            lines(&list).len(),
            6,
            "a project heading is not the completed group, so nothing folds under it: {:?}",
            lines(&list)
        );

        list.turn();
        assert!(lines(&list).contains(&"… 2 more".to_string()));
    }

    #[test]
    fn axis_narrows_the_list_to_the_agents_a_line_named() {
        let mut list = listed(vec![
            view("ask-a1b", Phase::Waiting, 10),
            view("busy-b2c", Phase::Working, 20),
            view("busy-c3d", Phase::Working, 30),
        ]);

        list.narrow(vec![Narrow::State(Some("working".to_string()))]);
        assert_eq!(
            lines(&list),
            ["working (2)", "busy-b2c", "busy-c3d"],
            "a hidden agent heads nothing, counts for nothing and is drawn nowhere"
        );
        assert_eq!(list.counts(), [(Group::Working, 2)]);
        assert_eq!(list.narrowing().as_deref(), Some("s:working"));

        list.narrow(vec![Narrow::Name(Some("c3d".to_string()))]);
        assert_eq!(
            lines(&list),
            ["working (1)", "busy-c3d"],
            "and a line only changes the narrowing it names"
        );
        assert_eq!(list.narrowing().as_deref(), Some("s:working a:c3d"));

        list.narrow(vec![Narrow::State(None), Narrow::Name(None)]);
        assert_eq!(lines(&list).len(), 6);
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
    fn headings_are_stops_the_cursor_walks_like_any_other_line() {
        let mut list = listed(vec![
            view("ask-a1b", Phase::Waiting, 10),
            view("busy-b2c", Phase::Working, 20),
        ]);

        // The view opens on an agent: somebody who opened it came to look at
        // agents, and the heading is one step back up from the first of them.
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
            ["needs input (1) shut", "", "working (1)", "busy-b2c"],
            "the rows go and the heading stays"
        );
        assert!(list.on_heading(), "with the cursor still on it");

        list.shut_or_open();
        assert_eq!(
            lines(&list),
            ["needs input (1)", "ask-a1b", "", "working (1)", "busy-b2c"]
        );
    }

    #[test]
    fn headings_stay_shut_while_the_fleet_moves_under_them() {
        let mut list = listed(vec![
            view("done-a1b", Phase::Done, 10),
            view("busy-b2c", Phase::Working, 20),
        ]);
        // Down off the working agent and onto the completed heading.
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
                "needs input (1)",
                "ask-c3d",
                "",
                "working (1)",
                "busy-b2c",
                "",
                "completed (1) shut",
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

        assert_eq!(
            heading(&list),
            Tally {
                members: 3,
                failures: 1,
                shut: false
            }
        );

        list.shut_or_open();
        assert_eq!(
            heading(&list),
            Tally {
                members: 3,
                failures: 1,
                shut: true
            },
            "and it answers for the same agents when they are behind it"
        );

        list.narrow(vec![Narrow::State(Some("done".to_string()))]);
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

        // The waiting agent answers, so its project is no longer the first one
        // drawn. What was shut is the repository, not the line it was on.
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
    fn arranged_a_held_agent_comes_first_in_its_group_and_is_still_held_in_the_next_one() {
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
                "needs input (1)",
                "ask-a1b",
                "",
                "working (3)",
                "busy-c3d",
                "busy-b2c",
                "busy-d4e",
            ]
        );
        assert!(list.holding(list.agent_by_id("busy-c3d").unwrap()));

        // It stops on a question, and it is at the top of the group it lands
        // in: what somebody said is that this agent is the one they want in
        // front of them, not that the working group has a favourite.
        list.show(vec![
            view("ask-a1b", Phase::Waiting, 10),
            view("busy-b2c", Phase::Working, 20),
            view("busy-c3d", Phase::Waiting, 30),
            view("busy-d4e", Phase::Working, 40),
        ]);
        assert_eq!(
            lines(&list),
            [
                "needs input (2)",
                "busy-c3d",
                "ask-a1b",
                "",
                "working (2)",
                "busy-b2c",
                "busy-d4e",
            ]
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
            ["working (3)", "busy-b2c", "busy-a1b", "busy-c3d"]
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
                "working (4)",
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
    fn arranged_a_move_stops_at_the_ends_of_a_group_and_at_the_ones_being_held() {
        let mut list = listed(vec![
            view("busy-a1b", Phase::Working, 10),
            view("busy-b2c", Phase::Working, 20),
        ]);
        assert!(!list.move_by(-1), "nothing is above the first of a group");
        list.down();
        assert!(!list.move_by(1), "and nothing is under the last");

        list.hold_or_let_go();
        assert_eq!(lines(&list), ["working (2)", "busy-b2c", "busy-a1b"]);
        list.down();
        assert_eq!(list.selected().unwrap().id(), "busy-a1b");
        assert!(
            !list.move_by(-1),
            "a held agent is above the rest by the holding, so the row it is \
             on is not one to be moved into"
        );
        assert_eq!(lines(&list), ["working (2)", "busy-b2c", "busy-a1b"]);

        // And a heading is not an agent either to move or to hold.
        list.up();
        list.up();
        assert!(list.on_heading());
        assert!(!list.move_by(1));
        assert!(!list.hold_or_let_go());
    }

    #[test]
    fn arranged_a_move_reaches_the_rows_on_the_screen_and_not_the_folded_ones() {
        let mut list = listed(
            (0..5)
                .map(|n| view(&format!("done-{n}"), Phase::Done, 10 * n))
                .collect(),
        );
        // Down to the last row the fold leaves standing.
        for _ in 0..2 {
            list.down();
        }
        assert_eq!(list.selected().unwrap().id(), "done-2");

        assert!(
            !list.move_by(1),
            "the row under it is the fold, and behind that is history"
        );
        assert_eq!(
            lines(&list),
            ["completed (5)", "done-4", "done-3", "done-2", "… 2 more"]
        );

        // Opened, every row is a row a move can reach.
        list.unfold();
        assert!(list.move_by(1));
        assert_eq!(
            lines(&list),
            [
                "completed (5)",
                "done-4",
                "done-3",
                "done-1",
                "done-2",
                "done-0"
            ]
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

    /// Every state there is, so a table over them cannot quietly miss one.
    const EVERY: [Phase; 8] = [
        Phase::Starting,
        Phase::Working,
        Phase::Waiting,
        Phase::Idle,
        Phase::Done,
        Phase::Failed,
        Phase::Stopped,
        Phase::Unknown,
    ];

    #[test]
    fn header_counts_a_group_in_a_word_the_list_can_be_narrowed_by() {
        // The heading over the rows says what the group means; the counter at
        // the top says the state that stands for it, and every one of those is
        // a word `s:` takes — so the header teaches the filter language by
        // existing rather than by documenting itself.
        for group in Group::ALL {
            let phase = EVERY
                .into_iter()
                .find(|phase| phase.as_str() == group.state())
                .unwrap_or_else(|| panic!("nothing is ever {}", group.state()));
            assert_eq!(
                Group::of(phase),
                group,
                "narrowing to {} would empty the group its own counter names",
                group.state()
            );
        }
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

        // The reading has already asked tmux, so an agent whose pane went is
        // stopped by the time the list sees it — and that is the agent the
        // gate skips for having no pane, counted the same way here.
        let mut gone = view("gone-e5f", Phase::Working, 50);
        gone.verdict.phase = Phase::Stopped;
        gone.verdict.evidence = Evidence::Gone;
        list.narrow(vec![Narrow::State(None)]);
        list.show(vec![view("busy-a1b", Phase::Working, 10), gone]);
        assert_eq!(list.live(), 1);
    }

    #[test]
    fn acts_a_heading_answers_for_its_agents_whether_or_not_they_are_drawn() {
        let mut list = listed(
            (0..5)
                .map(|n| view(&format!("done-{n}"), Phase::Done, 10 * n))
                .collect(),
        );
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
            5,
            "the fold decides how many rows are drawn, not how many there are"
        );

        list.shut_or_open();
        assert_eq!(
            members(&list, under).len(),
            5,
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
    fn acts_a_heading_on_the_project_axis_answers_for_the_agents_under_it() {
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
    fn acts_a_row_carries_a_mark_until_somebody_has_looked_at_it() {
        let mut ended = view("done-a1b", Phase::Done, 10);
        assert!(
            unread(&ended),
            "an agent that has ended and nobody has been to read"
        );

        ended.state.seen = 10;
        assert!(!unread(&ended), "and it is read once somebody has");

        ended.state.last_event = 20;
        assert!(
            unread(&ended),
            "something said after the look is news again"
        );

        // Including on a record that stamped its ending: the mark is about
        // what the agent has said since somebody looked, and a run that ended
        // at ten can have an answer written down at twenty.
        let mut stamped = view("done-c3d", Phase::Done, 10);
        stamped.state.ended = 10;
        stamped.state.seen = 10;
        assert!(!unread(&stamped));
        stamped.state.last_event = 20;
        assert!(unread(&stamped));

        // An agent still going is not holding anything to read: what it is
        // doing is on the row already, and it changes with every reading.
        for phase in [Phase::Starting, Phase::Working] {
            assert!(!unread(&view("busy-b2c", phase, 30)), "{phase}");
        }
        assert!(
            unread(&view("ask-c3d", Phase::Waiting, 30)),
            "and one stopped on a question is the whole reason for the mark"
        );
    }

    #[test]
    fn acts_a_row_is_called_what_somebody_renamed_it_to() {
        let mut named = view("fix-login-a1b", Phase::Idle, 10);
        named.state.name = Some("auth".to_string());
        let plain = view("port-importer-b2c", Phase::Idle, 20);

        assert_eq!(called(&named), "auth");
        assert_eq!(
            called(&plain),
            "port-importer-b2c",
            "and its id until somebody calls it something else"
        );

        let mut list = listed(vec![named, plain]);
        list.narrow(vec![Narrow::Name(Some("auth".to_string()))]);
        assert_eq!(
            lines(&list),
            ["idle (1)", "fix-login-a1b"],
            "and a narrowing takes the name off the row as readily as the id"
        );
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

        // Somebody has come to the wall from the request itself, and its
        // number is the only word for the agent they have in front of them.
        list.narrow(vec![Narrow::Name(Some("#12".to_string()))]);
        assert_eq!(lines(&list), ["idle (1)", "fix-login-a1b"]);
        assert_eq!(list.counts(), [(Group::Idle, 1)]);

        list.narrow(vec![Narrow::Name(Some("#3".to_string()))]);
        assert_eq!(lines(&list), ["idle (1)", "port-importer-b2c"]);

        list.narrow(vec![Narrow::Name(Some("#99".to_string()))]);
        assert!(
            list.is_empty(),
            "and a number nobody's branch wears finds nobody"
        );
    }

    #[test]
    fn pr_is_read_again_with_every_reading() {
        // What a request is doing is the thing on a row most likely to have
        // moved since the last look: a check goes green while somebody is
        // reading it, and a row that answered from the first reading for as
        // long as the view was open would be a row that never went green.
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

        // And an agent that has gone takes its number with it.
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
