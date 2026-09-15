//! What the view lists, and where the cursor is in it.
//!
//! A list of agents is not a table with a sort order. What somebody opens this
//! for is one question — *is anything waiting on me?* — so the agents are
//! gathered under the answer: the ones somebody pinned there first, then the
//! work standing in front of a reviewer, then the ones that have stopped on a
//! question, then the ones mid-turn, then the turns that are over, and under
//! all of them the ones somebody has put to sleep.
//!
//! Inside a group the order is the order agents were started in, which is the
//! one order that does not move under a cursor while somebody is reading. The
//! exception is the finished group, where the newest ending comes first.
//!
//! A group past [`FOLD_AT`] rows shows that many and folds the rest away
//! behind a count, whichever axis it was gathered on. Ten rows is as much of
//! one group as somebody reads before they scroll, and the fold is the same
//! ten whatever the terminal is: a wall cut to the height of the window moves
//! rows under a reader every time the window changes, and a screen with room
//! to spare is not a reason to put sixty endings in front of somebody.
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
//! An order the list works out is an order somebody may disagree with, so
//! three things are theirs to say: which agent is pinned over the wall, which
//! is asleep under it, and what order a group goes in. All are said against
//! the agents and the group rather than against the screen, which is what lets
//! them outlive the view they were said in.

use crate::derive::{Evidence, View};
use crate::pr::{self, Pr, Standing};
use crate::store::{Ask, Meta, Phase};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

/// How many rows of one group somebody is shown before the rest fold away
/// behind a count.
pub const FOLD_AT: usize = 10;

/// What an agent is, to somebody deciding what to do next.
///
/// Written down as the word it is titled with, because an order somebody put a
/// group in is kept against the group and read back by a later view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Group {
    /// Held here by somebody, whatever it is doing: the one agent they want in
    /// front of them outranks whatever amx would have made of it.
    Pinned,
    /// Its turn is over and its branch has a request still asking for
    /// something. The agent has nothing left to do and a person has.
    Review,
    /// Stopped on a question: nothing happens until somebody answers it.
    NeedsInput,
    /// Mid-turn. Nothing to do but let it work.
    Working,
    /// The turn is over: sitting at its prompt, ended one way or another, or
    /// gone somewhere amx cannot account for. Whether there is still a process
    /// behind it is the row's to say, and the glyph says it.
    Completed,
    /// Put under everything by somebody, whatever it is doing: the agent they
    /// have decided not to look at for now. The other thing a person says
    /// about a row, and the opposite of pinning it.
    Asleep,
}

impl Group {
    /// Every group, in the order a person reads them.
    pub const ALL: [Group; 6] = [
        Group::Pinned,
        Group::Review,
        Group::NeedsInput,
        Group::Working,
        Group::Completed,
        Group::Asleep,
    ];

    /// Which group an agent belongs to: what somebody said about it, then what
    /// it is doing, then what its work is waiting on.
    ///
    /// What a person said wins over everything, because those are the two lines
    /// of this table they wrote themselves: pinned over the wall, or under all
    /// of it. After them the states a person can do nothing about, so a request
    /// standing open never takes an agent out of the group that says it is
    /// asking or working.
    pub fn of(phase: Phase, held: bool, asleep: bool, reviewable: bool) -> Group {
        if held {
            return Group::Pinned;
        }
        if asleep {
            return Group::Asleep;
        }
        match phase {
            Phase::Waiting => Group::NeedsInput,
            Phase::Starting | Phase::Working => Group::Working,
            _ if reviewable => Group::Review,
            _ => Group::Completed,
        }
    }

    /// The words a heading over the group reads, as a person reads them.
    pub fn title(self) -> &'static str {
        match self {
            Group::Pinned => "Pinned",
            Group::Review => "Ready for review",
            Group::NeedsInput => "Needs input",
            Group::Working => "Working",
            Group::Completed => "Completed",
            Group::Asleep => "Asleep",
        }
    }

    /// The word the group is counted and narrowed by, where a count of it is
    /// being read rather than a heading over rows.
    ///
    /// Two words for one group, and the second earns its keep: a heading says
    /// what the group means to somebody scanning the list, and a counter says
    /// the word `s:` takes for it, so the header teaches the language the list
    /// is narrowed in by existing. One word per group and none for anything
    /// else — a counter naming a word the list cannot be narrowed by would
    /// send somebody to an empty list.
    pub fn state(self) -> &'static str {
        match self {
            Group::Pinned => "pinned",
            Group::Review => "review",
            Group::NeedsInput => "waiting",
            Group::Working => "working",
            Group::Completed => "done",
            Group::Asleep => "asleep",
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
/// arranged it in: which way it is gathered, the agents pinned over the wall
/// and the ones asleep under it, and the order a group was put in.
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
    asleep: BTreeSet<String>,
    order: BTreeMap<Group, Vec<String>>,
}

impl Arrangement {
    /// What the last view left written down under this state root, for a
    /// reader that is not a view.
    ///
    /// Nothing else amx runs holds a list, and what somebody pinned is still
    /// theirs to have obeyed: a verb deciding whether to take an idle agent's
    /// pane has to know that the agent is the one they wanted in front of
    /// them. So the file is read where it is written, through the view's own
    /// [`crate::tui::Remembered`], rather than a second account of the same
    /// document.
    ///
    /// The default where there is no file, no room for one, or nothing
    /// readable in it. A verb that failed because a view had never been opened
    /// would be a verb that needs a view.
    pub fn from_disk(root: &Path) -> Arrangement {
        crate::paths::view_file(root)
            .map(|path| super::Remembered::read(&path).arrangement)
            .unwrap_or_default()
    }

    /// Whether this agent is one somebody pinned over the wall.
    ///
    /// By id, because a reader outside the view has an id and not a reading:
    /// see [`List::holding`], which is the same question asked of the list a
    /// view is drawing.
    pub fn has_pinned(&self, id: &str) -> bool {
        self.held.contains(id)
    }

    /// Whether this agent is one somebody has put under the wall.
    ///
    /// The other half of the same question, asked the same way: see
    /// [`List::sleeping`].
    // Nothing outside this module's tests asks it: the reader that steps the
    // wall from a key arranges a whole list from the arrangement rather than
    // asking after one id. Kept beside `has_pinned`, which park reads, for
    // the verb that asks the same question of the other mark.
    #[allow(dead_code)]
    pub fn has_asleep(&self, id: &str) -> bool {
        self.asleep.contains(id)
    }
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
    /// Which heading's rows the fold is holding back, and how many of them.
    /// The heading, because a fold is opened one group at a time and the row
    /// somebody presses is the only thing that says which.
    Fold(Under, usize),
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

impl On {
    /// The agent the cursor is standing on, where it is standing on one.
    fn agent(&self) -> Option<&str> {
        match self {
            On::Agent(id) => Some(id),
            _ => None,
        }
    }
}

/// One narrowing, as the change it makes. A line only changes what it names,
/// so `a:port` on its own leaves the state narrowing where it was, and `s:`
/// with nothing after it drops the states. What a line of state words does to
/// the name is `List::narrow`'s to say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Narrow {
    State(Option<String>),
    Name(Option<String>),
}

/// What the list is narrowed to. Every one that is set has to match, and
/// nothing set keeps everything.
///
/// The states are a list because a line may name several of them, and any one
/// of them keeps a row: `s:waiting s:working` is somebody asking for what needs
/// them beside what is still running, and states that all had to match at once
/// would be a line that always emptied the wall.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Filters {
    state: Vec<String>,
    name: Option<String>,
}

impl Filters {
    fn keeps(&self, view: &View, group: Group, prs: &[Pr]) -> bool {
        // The group the row is drawn under rather than the state on the
        // record. The words a counter says are the words `s:` takes, so a wall
        // gathered five ways is narrowed the same five ways, and what somebody
        // typed leaves the list holding exactly the group they read the count
        // of.
        //
        // But the record knows more states than the wall has groups — failed,
        // idle, stopped, starting and unknown all share a heading with others
        // — and a word no counter says still finds its rows, because
        // `s:failed` is how somebody picks the one that died out of everything
        // that finished. A group's word stays the group's, though: `working`
        // and `done` are both, and reading them as the state as well would
        // put a pinned agent under `s:working` and a row under review under
        // `s:done`, which is the list no longer holding what the counter
        // counted.
        let state = self.state.is_empty()
            || self.state.iter().any(|want| {
                if Group::ALL.iter().any(|group| group.state() == want) {
                    group.state() == want
                } else {
                    view.phase().as_str() == want
                }
            });
        // Every word for the agent that somebody might have in front of them:
        // the id every other surface uses, the name a person gave it because
        // the id was not what they call it, the `#12` its branch wears — which
        // is routinely the only one of those a person has, because they came
        // to the wall from the pull request — and the task it was started on.
        //
        // The task because that is the one string on the record the person
        // wrote themselves. The id is a word amx made up, and what an agent
        // last said is the agent's. A search that could not reach the sentence
        // somebody typed an hour ago is a search that misses the thing they
        // actually remember.
        //
        // Not the summary, though it is the one on the screen. It changes
        // every time the agent speaks, so a wall narrowed by it would drop
        // rows while somebody was reading them.
        let name = self.name.as_ref().is_none_or(|want| {
            holds(view.id(), want)
                || holds(called(view), want)
                || holds(&view.meta.task, want)
                || prs.iter().any(|pr| holds(&pr.label(), want))
        });
        state && name
    }

    /// What was typed, read back — in the words it would be typed in now. The
    /// name came off a find line, so it reads as one: a header naming a token
    /// nobody can type any more is a header that cannot be acted on.
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

/// The agents, as lines with a cursor on one of them.
#[derive(Debug)]
pub struct List {
    views: Vec<View>,
    items: Vec<Item>,
    cursor: usize,
    /// Whether the cursor has been put on anything yet, which is what tells a
    /// view that has just opened from one somebody is reading.
    landed: bool,
    /// The groups somebody has opened the fold of, by what they stand for.
    /// Remembered the way `shut` is and for the same reason: the heading a
    /// fold belongs to has to be the same heading on the next reading.
    unfolded: HashSet<Key>,
    /// The groups somebody has shut, by what they stand for.
    shut: HashSet<Key>,
    /// The agents somebody has pinned over the wall.
    held: BTreeSet<String>,
    /// And the ones somebody has put under it.
    asleep: BTreeSet<String>,
    /// The order somebody put a group in, as the ids of the agents that were
    /// under it when they said so.
    order: BTreeMap<Group, Vec<String>>,
    axis: Axis,
    filters: Filters,
    /// How many agents each group has, worked out where the lines are.
    counts: Vec<(Group, usize)>,
    /// And how many of them have stopped on a question, which is the one count
    /// that is about a state rather than a group.
    waiting: usize,
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
            unfolded: HashSet::new(),
            shut: HashSet::new(),
            held: BTreeSet::new(),
            asleep: BTreeSet::new(),
            order: BTreeMap::new(),
            axis: Axis::default(),
            filters: Filters::default(),
            counts: Vec::new(),
            waiting: 0,
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
        self.rebuild(on.agent());
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
        self.rebuild(on.agent());
        self.follow(&on);
    }

    /// How the list stands arranged, to be kept and given back to the next
    /// view that opens.
    pub fn arrangement(&self) -> Arrangement {
        Arrangement {
            axis: self.axis,
            held: self.held.clone(),
            asleep: self.asleep.clone(),
            order: self.order.clone(),
        }
    }

    /// Put the list back the way it was arranged. The cursor holds what it was
    /// on, for the same reason it does across a turn of the axis.
    pub fn arrange(&mut self, arrangement: Arrangement) {
        let on = self.on();
        self.axis = arrangement.axis;
        self.held = arrangement.held;
        self.asleep = arrangement.asleep;
        self.order = arrangement.order;
        self.rebuild(on.agent());
        self.follow(&on);
    }

    /// Whether this agent is one somebody has pinned over the wall.
    pub fn holding(&self, view: &View) -> bool {
        self.held.contains(view.id())
    }

    /// Whether this agent is one somebody has put under it.
    pub fn sleeping(&self, view: &View) -> bool {
        self.asleep.contains(view.id())
    }

    /// Pin the agent under the cursor to the top of the list, or let it go.
    ///
    /// About the agent and not about the state it is in: a pinned agent stays
    /// pinned as its turn runs and ends, because what somebody said is that
    /// this agent is the one they want in front of them.
    ///
    /// A sleeping agent wakes as it is pinned. The two marks are the same
    /// sentence in opposite directions, and an agent cannot be both the one
    /// somebody wants in front of them and one they have put away.
    ///
    /// Answers whether there was an agent to do it to, which is what tells a
    /// key pressed on a heading from a key that changed something.
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

    /// Put the agent under the cursor under the whole wall, or wake it.
    ///
    /// About the agent for the same reason pinning is: a sleeping agent stays
    /// under everything as its turn runs and ends, because what somebody said
    /// is that this is the agent they are not looking at for now. It goes on
    /// counting among the ones asking, though — where a row is drawn is not an
    /// answer to its question.
    ///
    /// A pinned agent lets go as it goes to sleep, and answers the same way
    /// [`List::hold_or_let_go`] does.
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
        let group = self.group(view);
        let mut members: Vec<String> = self
            .ordered()
            .into_iter()
            .filter(|&n| self.group(&self.views[n]) == group)
            .map(|n| self.views[n].id().to_string())
            .collect();

        let Some(at) = members.iter().position(|other| *other == id) else {
            return false;
        };
        let Some(to) = at.checked_add_signed(by).filter(|to| *to < members.len()) else {
            return false;
        };
        // The rows a move can reach are the rows on the screen. A fold
        // holds history back, and an agent moved behind one would go where the
        // cursor could not follow it, leaving somebody's cursor on whoever
        // came up in its place.
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

    /// Whether this agent has a row on the screen, as against being counted by
    /// a heading that is shut or held back by a fold.
    fn drawn(&self, id: &str) -> bool {
        self.items
            .iter()
            .any(|item| self.agent(*item).is_some_and(|view| view.id() == id))
    }

    /// Where an agent comes in the reading order, which is where its group
    /// comes.
    fn rank(&self, view: &View) -> usize {
        let group = self.group(view);
        Group::ALL
            .iter()
            .position(|other| *other == group)
            .unwrap_or(Group::ALL.len())
    }

    /// Where an agent sits in the order somebody put its group in, and past
    /// the end of it for one nobody has placed.
    fn seat(&self, n: usize) -> usize {
        let view = &self.views[n];
        self.order
            .get(&self.group(view))
            .and_then(|ids| ids.iter().position(|id| id == view.id()))
            .unwrap_or(usize::MAX)
    }

    /// Narrow the list to part of the fleet, changing only what was named.
    ///
    /// One batch is one reading of the line, so the states it names are the
    /// states there now: a line read again on every keystroke that added its
    /// words to the reading before it could never be widened by deleting one.
    /// A batch naming no state at all leaves the states where they were, and
    /// `s:` on its own names none, which is how the last word deleted back to
    /// the token gives the fleet back.
    ///
    /// A batch that names states and no name drops the name with them, because
    /// a line of state words is not a line with a name on it. Half of typing
    /// `s:waiting s:working` reads as a name — `s:waiting s` is a sentence
    /// until the colon lands — and a name left standing from the keystroke
    /// before would narrow the wall to nothing under a header saying states.
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
        matches!(self.items.get(self.cursor), Some(Item::Fold(..)))
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

    /// The project the cursor is standing in, where the list is gathered by
    /// them.
    ///
    /// A heading names one and every row under it runs in it, so the whole
    /// group answers the same path: somebody reading a project's agents is
    /// looking at that project, wherever in it their cursor stopped.
    ///
    /// Nothing on the state axis, where the row above one is somebody else's
    /// repository and a heading is a word rather than a place.
    pub fn project_under_cursor(&self) -> Option<PathBuf> {
        if self.axis != Axis::Project {
            return None;
        }
        match self.items.get(self.cursor)? {
            Item::Heading(Under::Project(at), _) => self.projects.get(*at).cloned(),
            Item::Agent(n) => Some(self.root_of(*n)),
            _ => None,
        }
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
        self.filters
            .keeps(view, self.group(view), self.requests(view))
    }

    /// Which group an agent is drawn under: where somebody put it, what it is
    /// doing, and what its work is waiting on out in the world.
    fn group(&self, view: &View) -> Group {
        Group::of(
            view.phase(),
            self.holding(view),
            self.sleeping(view),
            self.reviewable(view),
        )
    }

    /// Whether this agent's work is standing in front of a reviewer: its turn
    /// is over, and its branch has a request that is still asking somebody for
    /// something.
    ///
    /// An agent amx cannot account for is not among them. The group is a claim
    /// that there is nothing left to do but read the work, and a reading that
    /// cannot say what the agent is doing cannot make it.
    pub fn reviewable(&self, view: &View) -> bool {
        let over = matches!(
            view.phase(),
            Phase::Idle | Phase::Done | Phase::Failed | Phase::Stopped
        );
        over && self.requests(view).iter().any(|pr| asking(pr.standing))
    }

    /// Whether an agent is drawn under this heading.
    fn belongs(&self, n: usize, under: Under) -> bool {
        match under {
            Under::Group(group) => self.group(&self.views[n]) == group,
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
        self.rebuild(on.agent());
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

    /// Show the rows the fold under the cursor was holding back, and keep
    /// showing them: somebody who opened it is going through them.
    ///
    /// That group and no other. A fold is a row of one group, so opening one
    /// says nothing about the rest of the wall, and a press that gave every
    /// group its rows back would be a press nobody could undo.
    pub fn unfold(&mut self) {
        self.unfold_at(self.cursor);
    }

    /// The same for a fold somebody pointed at rather than walked to, which
    /// is a line of its own: a click on a fold opens it and leaves the cursor
    /// where it was.
    pub fn unfold_at(&mut self, at: usize) {
        let Some(Item::Fold(under, _)) = self.items.get(at).copied() else {
            return;
        };
        let Some(key) = self.key(under) else {
            return;
        };
        self.unfolded.insert(key);
        let on = self.on();
        self.rebuild(on.agent());
    }

    /// How many agents have stopped on a question, wherever their rows are.
    ///
    /// The one count that goes by the state rather than by the group: an agent
    /// somebody pinned is drawn over the wall and is still waiting on them,
    /// and the badge that number feeds is the whole of what the view is opened
    /// to read.
    pub fn waiting(&self) -> usize {
        self.waiting
    }

    /// How many agents are in each group that has any, whichever way they are
    /// gathered: what there is does not depend on how it was laid out.
    ///
    /// Read back rather than worked out. The counters along the header and the
    /// name the terminal is given both ask on every frame, and a frame is
    /// drawn many times over a reading that was taken once, so the walk goes
    /// where the lines are laid out and each look after it is a look at that.
    pub fn counts(&self) -> &[(Group, usize)] {
        &self.counts
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
    /// same answer: the gate counts the agents whose pane still answers for
    /// them, and the reading has already asked. An agent whose pane went is
    /// stopped by the time the list sees it, and one whose pane amx took —
    /// idle and unwatched long enough to be parked — keeps its phase and says
    /// so in its evidence, so it is the evidence and not the phase that keeps
    /// it off the count. The gate would let a spawn through over it; a header
    /// counting it against the cap would say the fleet is fuller than the
    /// gate does.
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

    /// The first line of the list, and the last — the two ends one move away
    /// rather than a walk.
    ///
    /// Reached by standing outside the list and stepping inward, so the same
    /// walk that keeps the cursor off a blank line keeps it off one here: the
    /// ends of the list are wherever the step stops, not wherever the items
    /// happen to end.
    pub fn top(&mut self) {
        self.cursor = 0;
        if matches!(self.items.first(), Some(Item::Blank)) {
            self.step(1);
        }
    }

    pub fn bottom(&mut self) {
        self.cursor = self.items.len().saturating_sub(1);
        if matches!(self.items.last(), Some(Item::Blank)) {
            self.step(-1);
        }
    }

    /// Put the cursor on this line, for a pointer that named one: any line
    /// but the blank, which is spacing rather than a stop. Answers whether
    /// the cursor landed.
    pub fn land(&mut self, at: usize) -> bool {
        match self.items.get(at) {
            Some(Item::Blank) | None => false,
            Some(_) => {
                self.cursor = at;
                true
            }
        }
    }

    /// Put the cursor on this agent, for a caller holding an id rather than a
    /// line: an agent just started has no line on the screen for anything to
    /// point at, and its id is the whole of what is known about it. Answers
    /// whether the cursor landed, which a narrowing hiding that agent — or a
    /// reading taken before it — makes false.
    pub fn land_on(&mut self, id: &str) -> bool {
        let Some(at) = self.row_of(id) else {
            return false;
        };
        self.cursor = at;
        true
    }

    /// The first agent on the screen with something on it for the person
    /// reading, for a key that lands the cursor on it.
    ///
    /// The rows the list is showing, which is what a narrowing left and what a
    /// shut heading is not holding back, so [`List::land_on`] can always go
    /// where this says. In their state order whichever axis is drawn, because
    /// what an agent is waiting for is a fact about the agent and not about
    /// the way the fleet was gathered.
    pub fn first_needing(&self) -> Option<String> {
        let showing: Vec<(Group, String)> = self
            .ordered()
            .into_iter()
            .map(|n| (self.group(&self.views[n]), self.views[n].id().to_string()))
            .filter(|(_, id)| self.drawn(id))
            .collect();
        needing_you(&showing)
    }

    /// Which line this agent is drawn on, where the list is drawing it.
    fn row_of(&self, id: &str) -> Option<usize> {
        self.items
            .iter()
            .position(|item| self.agent(*item).is_some_and(|view| view.id() == id))
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
    ///
    /// `keeping` is the agent the cursor was on when the caller decided to
    /// rebuild, taken as an argument rather than read here: half the callers
    /// have already moved the reading the old items point into, and an id
    /// read across that seam could be somebody else's.
    fn rebuild(&mut self, keeping: Option<&str>) {
        self.remember_the_roots();
        let order = self.ordered();
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
            Axis::Project => {
                let (projects, items) = self.by_project(&order, keeping);
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
    /// by what they need, and inside that the order somebody put the group in,
    /// then the order the agents were started in — except the finished ones,
    /// where the newest ending comes first because what just finished is what
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
            self.rank(&self.views[a])
                .cmp(&self.rank(&self.views[b]))
                .then_with(|| self.seat(a).cmp(&self.seat(b)))
                .then_with(|| match self.group(&self.views[a]) {
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

    /// How many agents each state has, off the order the lines are laid out
    /// from.
    ///
    /// The same agents the lines are made of, so a count and the rows under a
    /// heading cannot disagree, and what a narrowing hid is out of both for
    /// the one reason. That order is every agent the narrowing left whichever
    /// way they are about to be gathered, which is what makes the count the
    /// same on either axis.
    fn counted(&self, order: &[usize]) -> Vec<(Group, usize)> {
        Group::ALL
            .into_iter()
            .filter_map(|group| {
                let count = order
                    .iter()
                    .filter(|&&n| self.group(&self.views[n]) == group)
                    .count();
                (count > 0).then_some((group, count))
            })
            .collect()
    }

    /// One heading per state that has anybody under it.
    fn by_state(&self, order: &[usize], keeping: Option<&str>) -> Vec<Item> {
        let mut items = Vec::new();
        for group in Group::ALL {
            let members: Vec<usize> = order
                .iter()
                .copied()
                .filter(|&n| self.group(&self.views[n]) == group)
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

            self.fold(
                Under::Group(group),
                &Key::Group(group),
                &members,
                keeping,
                &mut items,
            );
        }
        items
    }

    /// Put a group's rows on the list, folding the ones past [`FOLD_AT`] away
    /// behind a count on a row of their own.
    ///
    /// Nothing here asks how tall the screen is. A group is as long as it is,
    /// and the list scrolls.
    ///
    /// The heading is handed in both ways round because the two do not answer
    /// each other yet: `Under::Project` is a place in a table this walk is
    /// still building, so the key it would read back is the last reading's.
    fn fold(
        &self,
        under: Under,
        key: &Key,
        members: &[usize],
        keeping: Option<&str>,
        items: &mut Vec<Item>,
    ) {
        let shown = match members.len() > FOLD_AT && !self.unfolded.contains(key) {
            true => self.worth_the_room(members, FOLD_AT, keeping),
            false => members.to_vec(),
        };
        let hidden = members.len() - shown.len();
        items.extend(shown.into_iter().map(Item::Agent));
        if hidden > 0 {
            items.push(Item::Fold(under, hidden));
        }
    }

    /// Which rows a fold keeps: the ones a person came to scan for. A failure
    /// is news however old it is, a row carrying a pull request is work still
    /// moving, and the row the cursor stands on is taken even over the room —
    /// folding it away would land the cursor on whoever came up in its place,
    /// and the card with it. What is left over fills whatever room is left,
    /// and everything kept is drawn in the order the group already reads in.
    ///
    /// Whether anybody has read a row plays no part. It used to, and a card
    /// marks its row read, so reading one made the row under the cursor fall
    /// out of the fold and the rest of the group shuffle up under somebody
    /// mid-scan.
    fn worth_the_room(&self, members: &[usize], room: usize, keeping: Option<&str>) -> Vec<usize> {
        let cursor = |n: usize| keeping == Some(self.views[n].id());
        let scanned = |n: usize| {
            self.views[n].phase() == Phase::Failed || !self.requests(&self.views[n]).is_empty()
        };
        let room = room.max(members.iter().filter(|&&n| cursor(n)).count());
        let chosen: HashSet<usize> = members
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
        members
            .iter()
            .copied()
            .filter(|n| chosen.contains(n))
            .collect()
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
    /// a question, and two equally quiet repositories go by path. A project
    /// past [`FOLD_AT`] agents folds the way a group does: sixty rows under
    /// one path is as long a wall as sixty under one heading.
    fn by_project(&self, order: &[usize], keeping: Option<&str>) -> (Vec<PathBuf>, Vec<Item>) {
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
            self.rank(&self.views[ours[0]])
                .cmp(&self.rank(&self.views[theirs[0]]))
                .then_with(|| here.cmp(there))
        });

        let mut projects = Vec::new();
        let mut items = Vec::new();
        for (root, members) in roots {
            let key = Key::Project(root.clone());
            let shut = self.shut.contains(&key);
            if !items.is_empty() {
                items.push(Item::Blank);
            }
            let under = Under::Project(projects.len());
            items.push(Item::Heading(under, self.tally(&members, shut)));
            if !shut {
                self.fold(under, &key, &members, keeping, &mut items);
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
            On::Agent(id) => self.row_of(id),
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

/// The wall in the order the view draws it, for a reader that is not the view.
///
/// A verb stepping through the fleet has to land where somebody reading the
/// wall would expect it to: the pinned row over everything, the sleeping ones
/// under it, and in between the groups in the order somebody scanning them
/// reads, each group the way they left it. All of that is the list's, so this
/// is the list — built, arranged the way the last view left it, and read back
/// as ids under their groups rather than as lines on a screen.
///
/// The state axis whatever axis the view was left on. The project axis is the
/// same agents gathered a different way, and a verb asked for the next agent
/// is asking about the wall rather than about the screen somebody happened to
/// close. Nothing is narrowed and nothing folds either, for the same reason:
/// a narrowing is a line somebody typed and a fold is a fact about a screen
/// that is not here.
///
/// What a branch has open comes from what the last look wrote down and no
/// forge is asked, because the reader here is gone before one could answer:
/// see [`pr::written`] for what a look started from a verb costs.
pub fn wall_order(views: &[View], arrangement: &Arrangement) -> Vec<(Group, String)> {
    let mut list = List {
        asks: pr::written,
        ..List::default()
    };
    list.arrange(Arrangement {
        axis: Axis::State,
        ..arrangement.clone()
    });
    list.show(views.to_vec());
    list.ordered()
        .into_iter()
        .map(|n| {
            let view = &list.views[n];
            (list.group(view), view.id().to_string())
        })
        .collect()
}

/// Which agent on a wall has something on it for the person reading it.
///
/// What is keeping somebody from getting on is a question nobody has answered,
/// then work standing in front of a reviewer, then the last turn to have
/// ended: an agent that stopped while they were away is what they came back
/// for. The order the wall is already in settles which row of a group that is
/// — each group the way somebody left it, and the finished newest first — so
/// the head of the first group with anybody in it is the answer.
///
/// Here rather than in the verb that first asked, because the key on the list
/// and `amx attach --waiting` are the same question asked of the same wall,
/// and two spellings of it would drift apart a group at a time.
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

/// Whether `said` holds `want`, whatever case either was written in.
///
/// An id and a generated name are lowercase and always were, so folding costs
/// them nothing. A task is a sentence somebody wrote, capitals and all, and a
/// search that missed `Port the importer` because the `p` was typed small is a
/// search nobody would use twice.
fn holds(said: &str, want: &str) -> bool {
    said.to_lowercase().contains(&want.to_lowercase())
}

/// What a row calls its agent: the name somebody gave it, else the title the
/// session goes under, and the id until there is either.
///
/// Here rather than on the reading itself, because it is a fact about the row:
/// the record is filed under the id, every verb takes the id, and these are
/// the words this one screen shows instead.
///
/// The rename comes first because it is the only one of the three a person
/// here wrote. The title after it, because an id says what the agent was
/// started on and says it in the words it was started with, while the vendor
/// has been naming the conversation out of the work all along and writing a
/// new name as the work moves. The id last, and it is not lost anywhere else:
/// the `ls` table prints it, every verb takes it, and a narrowing finds a row
/// by it.
pub fn called(view: &View) -> &str {
    view.state
        .name
        .as_deref()
        .or(view.state.session_title.as_deref())
        .unwrap_or_else(|| view.id())
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

/// Whether a request is still asking somebody for something.
///
/// One that was merged or shut is over, and a draft is not offered to anybody
/// yet. Everything else is work standing between an agent and a person,
/// whatever the checks on it are doing.
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
    use tempfile::TempDir;

    /// A reading of one agent: the state it is in, and when it was last heard
    /// from.
    fn view(id: &str, phase: Phase, at: u64) -> View {
        View {
            meta: Meta {
                id: id.to_string(),
                task: "fix the login bug".to_string(),
                agent: None,
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

    /// A forge where three of the branches have a request on them, so the
    /// number on the row is read from something rather than made up here. Two
    /// are still asking somebody for something and the third is in.
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
            _ => Vec::new(),
        }
    }

    /// The same list with one agent pinned, which is a cursor on its row and
    /// the key.
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

    /// The same list with one agent asleep, which is a cursor on its row and
    /// the other key.
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
                Item::Fold(_, hidden) => format!("… {hidden} more"),
                Item::Blank => String::new(),
            })
            .collect()
    }

    fn listed(views: Vec<View>) -> List {
        let mut list = List::default();
        list.show(views);
        list
    }

    /// The wall as a reader outside the view reads it down: the group each
    /// agent was gathered under, and the agent. What [`lines`] is to a screen.
    fn walled(order: &[(Group, String)]) -> Vec<String> {
        order
            .iter()
            .map(|(group, id)| format!("{} {id}", group.title()))
            .collect()
    }

    /// A run of finished agents, `done-0` the oldest ending and the last of
    /// them the newest, which is the order the group draws them in.
    fn a_history(count: u64) -> Vec<View> {
        (0..count)
            .map(|n| view(&format!("done-{n}"), Phase::Done, 10 * n))
            .collect()
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
        // Two things on a frame ask what the fleet is — the counters along the
        // header and the name the terminal is given — and a frame is drawn
        // many times over a reading that was taken once. So the count is
        // worked out where the lines are, and every look after that reads it.
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

        // And the answer is the reading's own: what a narrowing left, and what
        // the next reading brought.
        list.narrow(vec![Narrow::State(Some("working".to_string()))]);
        assert_eq!(list.counts(), [(Group::Working, 2)]);
        list.narrow(vec![Narrow::State(None)]);
        list.show(vec![view("ask-a1b", Phase::Waiting, 10)]);
        assert_eq!(list.counts(), [(Group::NeedsInput, 1)]);

        // Whichever way they are gathered: what there is does not depend on
        // how it was laid out.
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
        // `unknown` is not a claim that anything is happening, so it does not
        // sit among the agents that are working. How long it has been out of
        // touch is on the row; the group only says nobody is waiting on it.
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
                "Completed (2)",
                "done-e5f",
                "merged-d4e",
            ],
            "an agent that is asking or working has something of its own left \
             to do, whatever its branch has open, and a request that was \
             merged is asking nobody for anything"
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
        assert_eq!(lines(&list), ["Completed (2)", "done-b2c", "done-a1b"]);
    }

    #[test]
    fn view_folds_a_group_past_ten_rows_behind_a_count() {
        // Twelve endings: the heading, the ten newest, and the fold on the
        // row under them. However tall the screen is — nothing here has been
        // told one.
        let mut list = listed(a_history(12));
        assert_eq!(
            lines(&list),
            [
                "Completed (12)",
                "done-11",
                "done-10",
                "done-9",
                "done-8",
                "done-7",
                "done-6",
                "done-5",
                "done-4",
                "done-3",
                "done-2",
                "… 2 more"
            ]
        );

        for _ in 0..10 {
            list.down();
        }
        list.unfold();
        assert_eq!(
            lines(&list).len(),
            13,
            "the fold line is gone with the fold"
        );
        assert!(lines(&list).contains(&"done-0".to_string()));

        // And it stays open while more finish.
        list.show(a_history(13));
        assert!(lines(&list).contains(&"done-0".to_string()));
    }

    #[test]
    fn view_folds_the_eleventh_row_of_a_group_and_leaves_ten_standing() {
        let ten = listed(a_history(10));
        assert_eq!(
            lines(&ten).len(),
            11,
            "a heading and ten rows, with nothing held back: {:?}",
            lines(&ten)
        );

        let eleven = listed(a_history(11));
        assert_eq!(
            lines(&eleven).last().map(String::as_str),
            Some("… 1 more"),
            "{:?}",
            lines(&eleven)
        );
    }

    #[test]
    fn view_folds_every_group_and_not_only_the_finished_ones() {
        // A dozen agents stopped on a question is as long a wall as a dozen
        // endings, and the fold is for the wall rather than for history.
        let mut views: Vec<View> = (0..12)
            .map(|n| view(&format!("ask-{n}"), Phase::Waiting, 10 * n))
            .collect();
        views.push(view("done-a1b", Phase::Done, 5));

        assert_eq!(
            lines(&listed(views)),
            [
                "Needs input (12)",
                "ask-0",
                "ask-1",
                "ask-2",
                "ask-3",
                "ask-4",
                "ask-5",
                "ask-6",
                "ask-7",
                "ask-8",
                "ask-9",
                "… 2 more",
                "",
                "Completed (1)",
                "done-a1b"
            ]
        );
    }

    #[test]
    fn view_keeps_failures_and_requests_ahead_of_the_plainly_done() {
        // Fourteen endings and rows for ten. The failure and the row carrying
        // a number are what somebody scans this group for, so the fold keeps
        // them over the oldest plainly done rows — in the order the group
        // already reads in.
        //
        // Nobody has read any of these, and the four that fold away are the
        // four oldest all the same: whether a row has been looked at plays no
        // part, because looking at one would otherwise move it.
        let mut list = List::default();
        list.asking(a_forge);
        let mut views: Vec<View> = (1..=12)
            .map(|n| view(&format!("done-{n:02}"), Phase::Done, 10 * n))
            .collect();
        views.push(view("broke-e5f", Phase::Failed, 2));
        views.push(on_a_branch(
            view("merged-f6g", Phase::Done, 1),
            "amx/merged-f6g",
        ));
        list.show(views);

        assert_eq!(
            lines(&list),
            [
                "Completed (14)",
                "done-12",
                "done-11",
                "done-10",
                "done-09",
                "done-08",
                "done-07",
                "done-06",
                "done-05",
                "broke-e5f",
                "merged-f6g",
                "… 4 more"
            ]
        );
    }

    #[test]
    fn view_never_folds_the_row_out_from_under_the_cursor() {
        let mut list = listed(a_history(11));
        for _ in 0..9 {
            list.down();
        }
        assert_eq!(list.selected().unwrap().id(), "done-1");

        // A twelfth ending arrives and pushes the cursor's row past the ten
        // the fold leaves standing. It is kept anyway, and an older row goes
        // in its place: a fold that took it would leave the cursor on
        // whoever came up there.
        let mut views = a_history(11);
        views.push(view("done-11", Phase::Done, 110));
        list.show(views);

        assert_eq!(
            lines(&list),
            [
                "Completed (12)",
                "done-11",
                "done-10",
                "done-9",
                "done-8",
                "done-7",
                "done-6",
                "done-5",
                "done-4",
                "done-3",
                "done-1",
                "… 2 more"
            ]
        );
        assert_eq!(list.selected().unwrap().id(), "done-1");
    }

    #[test]
    fn view_reaches_the_top_and_the_foot_of_the_list_in_one_move() {
        let mut list = listed(vec![
            view("ask-a1b", Phase::Waiting, 10),
            view("busy-b2c", Phase::Working, 20),
            view("done-c3d", Phase::Done, 30),
        ]);
        // Headings and the blanks between the groups, so neither end of the
        // walk is a line the cursor may rest on by accident.
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

        // From anywhere, and never onto a blank: the cursor does not rest on
        // spacing, so neither end of the list may be one.
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
    fn view_puts_the_cursor_on_an_agent_the_caller_names() {
        let mut list = listed(vec![
            view("ask-a1b", Phase::Waiting, 10),
            view("busy-b2c", Phase::Working, 20),
        ]);
        assert_eq!(list.selected().unwrap().id(), "ask-a1b");

        assert!(list.land_on("busy-b2c"));
        assert_eq!(list.selected().unwrap().id(), "busy-b2c");

        // An agent the list is not drawing has no line for the cursor to go
        // to, and the answer says so rather than the cursor moving.
        assert!(!list.land_on("port-c3d"));
        assert_eq!(list.selected().unwrap().id(), "busy-b2c");
    }

    #[test]
    fn view_names_the_first_agent_that_needs_you() {
        // A question nobody has answered is what is holding somebody up.
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

        // With nothing asking, work standing in front of a reviewer.
        let mut without_the_question = fleet();
        without_the_question.remove(2);
        assert_eq!(
            over_the_forge(without_the_question)
                .first_needing()
                .as_deref(),
            Some("review-b2c")
        );

        // And with neither, the turn that ended most recently, which the group
        // has at its head already.
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

        // Work in flight and rows somebody put away is a wall with nothing on
        // it for them.
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

        // Gathered by project, the agent that needs somebody is the same one:
        // what a row is waiting for is not a fact about the way the rows were
        // laid out.
        let mut list = over_the_disk(fleet());
        assert_eq!(list.first_needing().as_deref(), Some("ask-a1b"));
        assert!(list.land_on("ask-a1b"));

        // A narrowing that hid the question leaves what is under it, because
        // the rows on the screen are the rows a cursor can reach.
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

        // A shut heading holds its rows the same way: the count says the
        // question is there, and no line of it is somewhere to put a cursor.
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
        let mut list = listed(a_history(12));

        for _ in 0..10 {
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
    fn axis_says_which_project_the_cursor_is_standing_in() {
        // What a line opened here is about. A heading names a project and the
        // rows under it run in it, so where in the group somebody stopped
        // makes no difference to the answer.
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

        // Gathered by state there is no project over the cursor for it to be
        // standing in: the row above one belongs to whoever started it.
        list.turn();
        assert_eq!(list.project_under_cursor(), None);
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
            ["Needs input (1)", "ask-a1b", "", "Working (1)", "busy-b2c"]
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
    fn axis_folds_a_project_past_ten_rows_and_opens_that_heading_alone() {
        let views: Vec<View> = (0..12)
            .map(|n| at(view(&format!("done-{n}"), Phase::Done, 10 * n), "/src/api"))
            .collect();
        let mut list = over_the_disk(views);

        assert_eq!(
            lines(&list).len(),
            12,
            "a path holds as many rows as a group does: {:?}",
            lines(&list)
        );
        assert!(lines(&list).contains(&"… 2 more".to_string()));

        for _ in 0..10 {
            list.down();
        }
        list.unfold();
        assert_eq!(lines(&list).len(), 13, "{:?}", lines(&list));

        // The same agents gathered by state are folded still: what somebody
        // opened is one heading rather than the fleet.
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

        // Two words are two states to keep rather than the second word winning:
        // somebody watching a fleet wants what needs them and what is still
        // running on the one screen, and the third group goes.
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

        // Half of `s:waiting s:working` reads as a name on the way through, so
        // a line of state words has to take the name with it.
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

        // It stops on a question, and it has not moved: what somebody said is
        // that this agent is the one they want in front of them, not that the
        // group it happened to be in has a favourite.
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

        // And the same key lets it go, back under what it is doing.
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

        // Its turn goes on under there: what somebody said is that they are
        // not looking at this agent for now, not that it has finished.
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

        // And the same key wakes it, back under what it is doing.
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

        // Pinning it wakes it: an agent cannot be both the one somebody wants
        // in front of them and one they have put away.
        assert!(list.hold_or_let_go());
        assert_eq!(
            lines(&list),
            ["Pinned (1)", "busy-a1b", "", "Working (1)", "busy-b2c"]
        );
        assert!(!list.sleeping(list.agent_by_id("busy-a1b").unwrap()));

        // And sleeping it lets it go the same way.
        assert!(list.sleep_or_wake());
        assert_eq!(
            lines(&list),
            ["Working (1)", "busy-b2c", "", "Asleep (1)", "busy-a1b"]
        );
        assert!(!list.holding(list.agent_by_id("busy-a1b").unwrap()));

        // A heading is not an agent to put to sleep, any more than it is one
        // to pin.
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

        // A pinned agent has a group of its own, so the rows a move can reach
        // are the ones left in the group it came out of.
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

        // And a heading is not an agent either to move or to pin.
        list.up();
        assert!(list.on_heading());
        assert!(!list.move_by(1));
        assert!(!list.hold_or_let_go());
    }

    #[test]
    fn arranged_a_move_reaches_the_rows_on_the_screen_and_not_the_folded_ones() {
        let mut list = listed(a_history(12));
        // Down to the last row the fold leaves standing.
        for _ in 0..9 {
            list.down();
        }
        assert_eq!(list.selected().unwrap().id(), "done-2");

        assert!(
            !list.move_by(1),
            "the row under it is the fold, and behind that is history"
        );
        assert_eq!(lines(&list).last().map(String::as_str), Some("… 2 more"));

        // Opened, every row is a row a move can reach.
        list.down();
        list.unfold();
        assert!(list.move_by(1));
        let after = lines(&list);
        assert_eq!(&after[10..], ["done-2", "done-0", "done-1"], "{after:?}");
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

        // A file written before the wall had a second mark reads as a wall
        // with nobody asleep, rather than as no arrangement at all.
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
            walled(&wall_order(&fleet(), &list.arrangement())),
            [
                "Pinned busy-d4e",
                "Working busy-b2c",
                // On a branch and nothing written down about it, so it is a
                // turn that is over rather than work in front of a reviewer:
                // a reader that prints once asks no forge.
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
            walled(&wall_order(&fleet(), &Arrangement::default())),
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
            walled(&wall_order(&fleet(), &list.arrangement())),
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
            walled(&wall_order(&fleet(), &list.arrangement())),
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
        // The heading is drawn out of this and nothing else, so the words a
        // person reads over the rows are settled here rather than by whoever
        // paints them.
        assert_eq!(
            Group::ALL.map(Group::title),
            [
                "Pinned",
                "Ready for review",
                "Needs input",
                "Working",
                "Completed",
                "Asleep"
            ]
        );
    }

    #[test]
    fn header_counts_a_group_in_a_word_the_list_can_be_narrowed_by() {
        // The heading over the rows says what the group means; the counter at
        // the top says the word that stands for it, and every one of those is
        // a word `s:` takes — so the header teaches the filter language by
        // existing rather than by documenting itself.
        let fleet = || {
            vec![
                view("pinned-a1b", Phase::Working, 10),
                on_a_branch(view("review-b2c", Phase::Done, 20), "amx/fix-login-a1b"),
                view("ask-c3d", Phase::Waiting, 30),
                view("busy-d4e", Phase::Working, 40),
                view("done-e5f", Phase::Done, 50),
                view("nap-f6g", Phase::Working, 60),
            ]
        };
        let one_of_each = [
            (Group::Pinned, "pinned-a1b"),
            (Group::Review, "review-b2c"),
            (Group::NeedsInput, "ask-c3d"),
            (Group::Working, "busy-d4e"),
            (Group::Completed, "done-e5f"),
            (Group::Asleep, "nap-f6g"),
        ];
        assert_eq!(
            one_of_each.len(),
            Group::ALL.len(),
            "a group with nobody in it here is a group this proves nothing about"
        );

        for (group, id) in one_of_each {
            let mut list = sleeping(pinning(over_the_forge(fleet()), "pinned-a1b"), "nap-f6g");
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
        // The counters teach the five group words, but the wall knew eight
        // states before it knew five groups, and `s:failed` was how somebody
        // found the one that died among everything that finished. A word the
        // record says has to keep finding its rows, or the completed group
        // becomes the one place the list cannot be narrowed inside.
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

        // The view opens on the agent that is asking, so the key pins that
        // one, and the badge is the one number on the screen that does not
        // move for it.
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

        // The reading has already asked tmux, so an agent whose pane went is
        // stopped by the time the list sees it — and that is the agent the
        // gate skips for having no pane, counted the same way here.
        let mut gone = view("gone-e5f", Phase::Working, 50);
        gone.verdict.phase = Phase::Stopped;
        gone.verdict.evidence = Evidence::Gone;
        list.narrow(vec![Narrow::State(None)]);
        list.show(vec![view("busy-a1b", Phase::Working, 10), gone]);
        assert_eq!(list.live(), 1);

        // An agent amx parked keeps its phase — nothing about it ended — and
        // holds no pane, so the gate skips it too. Eight of them on a machine
        // read as `9 running` beside one agent at work, on 2026-09-11.
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
    fn acts_a_heading_answers_for_its_agents_whether_or_not_they_are_drawn() {
        let mut list = listed(a_history(12));
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
            12,
            "the fold decides how many rows are drawn, not how many there are"
        );

        list.shut_or_open();
        assert_eq!(
            members(&list, under).len(),
            12,
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
    fn acts_a_row_takes_the_rename_then_the_sessions_title_then_the_id() {
        // All three on one agent, so what outranks what is read off one row
        // rather than off three that could each be true on their own.
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

        // What somebody remembers about an agent is what they asked it for.
        // The id is a word amx made up and the summary is the agent's, so the
        // task is the one string on the record that the person typed.
        let mut list = listed(vec![porting, logging]);
        list.narrow(vec![Narrow::Name(Some("importer".to_string()))]);
        assert_eq!(lines(&list), ["Completed (1)", "a1b"]);

        // Ignoring case, because a task is a sentence somebody wrote and a
        // search that missed it over a capital is a search nobody trusts.
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

        // Somebody has come to the wall from the request itself, and its
        // number is the only word for the agent they have in front of them.
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
