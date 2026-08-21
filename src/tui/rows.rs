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

use crate::derive::View;
use crate::store::{Meta, Phase};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// How many finished agents are shown before the rest fold into a count.
pub const FOLD: usize = 3;

/// What an agent is, to somebody deciding what to do next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
}

/// Which way the agents are gathered.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Axis {
    /// Under what they need, which is what somebody opens the view for.
    #[default]
    State,
    /// Under the project they are running in.
    Project,
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

/// One line of the list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Item {
    /// A heading, and how many agents are under it.
    Heading(Under, usize),
    /// The agent at this position of the reading behind the list.
    Agent(usize),
    /// How many finished agents the fold is holding back.
    Fold(usize),
}

impl Item {
    /// Whether the cursor can rest on this line. A heading is a label, not a
    /// thing to do something to.
    pub fn selectable(self) -> bool {
        !matches!(self, Item::Heading(..))
    }
}

/// The agents, as lines with a cursor on one of them.
#[derive(Debug)]
pub struct List {
    views: Vec<View>,
    items: Vec<Item>,
    cursor: usize,
    unfolded: bool,
    axis: Axis,
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
            unfolded: false,
            axis: Axis::default(),
            projects: Vec::new(),
            roots: HashMap::new(),
            probe: holds_a_repository,
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

    /// Take a fresh reading.
    ///
    /// The cursor holds onto the agent it was on rather than the line number it
    /// was at: agents change groups while somebody is looking at them, and a
    /// cursor that stayed on line four would end up on whoever moved into it.
    pub fn show(&mut self, views: Vec<View>) {
        let held = self.selected().map(|view| view.id().to_string());
        self.views = views;
        self.rebuild();
        if let Some(id) = held {
            self.follow(&id);
        }
    }

    pub fn axis(&self) -> Axis {
        self.axis
    }

    /// Gather them the other way. The cursor holds its agent across the turn,
    /// because turning the axis is a question about the fleet and not about
    /// the one agent somebody was looking at.
    pub fn turn(&mut self) {
        self.axis = match self.axis {
            Axis::State => Axis::Project,
            Axis::Project => Axis::State,
        };
        let held = self.selected().map(|view| view.id().to_string());
        self.rebuild();
        if let Some(id) = held {
            self.follow(&id);
        }
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
                    .filter(|view| Group::of(view.phase()) == group)
                    .count();
                (count > 0).then_some((group, count))
            })
            .collect()
    }

    /// Whether there is nothing on the screen.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn down(&mut self) {
        self.step(1);
    }

    pub fn up(&mut self) {
        self.step(-1);
    }

    /// Move to the next line the cursor can rest on, staying put at the ends.
    fn step(&mut self, by: isize) {
        let mut at = self.cursor as isize;
        loop {
            at += by;
            let Ok(next) = usize::try_from(at) else {
                return;
            };
            let Some(item) = self.items.get(next) else {
                return;
            };
            if item.selectable() {
                self.cursor = next;
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

    /// Every agent, in the one order both axes draw them in:
    /// by what they need, and inside that the order they were started in —
    /// except the finished ones, where the newest ending comes first, because
    /// what just finished is what somebody scanning them came for.
    ///
    /// One order for both axes is what keeps a row's neighbours its own: an
    /// agent does not change who it sits beside merely because the fleet was
    /// gathered a different way.
    fn ordered(&self) -> Vec<usize> {
        let mut order: Vec<usize> = (0..self.views.len()).collect();
        order.sort_by(|&a, &b| {
            rank(&self.views[a])
                .cmp(&rank(&self.views[b]))
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

            let shown = if group == Group::Completed && !self.unfolded {
                FOLD.min(members.len())
            } else {
                members.len()
            };
            items.push(Item::Heading(Under::Group(group), members.len()));
            items.extend(members[..shown].iter().map(|&n| Item::Agent(n)));
            if shown < members.len() {
                items.push(Item::Fold(members.len() - shown));
            }
        }
        items
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
            let root = self
                .roots
                .get(self.views[n].id())
                .cloned()
                .unwrap_or_else(|| self.views[n].meta.dir.clone());
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
            items.push(Item::Heading(Under::Project(projects.len()), members.len()));
            items.extend(members.into_iter().map(Item::Agent));
            projects.push(root);
        }
        (projects, items)
    }

    /// Put the cursor back on the agent it was on.
    fn follow(&mut self, id: &str) {
        let found = self
            .items
            .iter()
            .position(|item| self.agent(*item).is_some_and(|view| view.id() == id));
        if let Some(at) = found {
            self.cursor = at;
        }
    }

    /// Put the cursor somewhere it can rest: where it is, else the next line
    /// down, else the last one above it.
    fn settle(&mut self) {
        if self.items.is_empty() {
            self.cursor = 0;
            return;
        }
        let at = self.cursor.min(self.items.len() - 1);
        let below = (at..self.items.len()).find(|&n| self.items[n].selectable());
        let above = (0..=at).rev().find(|&n| self.items[n].selectable());
        self.cursor = below.or(above).unwrap_or(0);
    }
}

/// When an agent's turn ended, as well as the record can say.
fn ended(view: &View) -> u64 {
    view.state.last_event.max(view.state.since)
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
fn shorten(path: &Path, home: Option<&Path>) -> String {
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
                Item::Heading(under, count) => format!("{} ({count})", list.title(*under)),
                Item::Agent(_) => list.agent(*item).unwrap().id().to_string(),
                Item::Fold(hidden) => format!("… {hidden} more"),
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
                "working (2)",
                "busy-a1b",
                "starting-e5f",
                "idle (1)",
                "idle-d4e",
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
    fn view_moves_the_cursor_over_the_agents_and_past_the_headings() {
        let mut list = listed(vec![
            view("ask-a1b", Phase::Waiting, 10),
            view("busy-b2c", Phase::Working, 20),
        ]);

        assert_eq!(list.selected().unwrap().id(), "ask-a1b");
        assert_eq!(list.cursor(), 1, "not the heading above it");

        list.down();
        assert_eq!(list.selected().unwrap().id(), "busy-b2c");
        list.down();
        assert_eq!(
            list.selected().unwrap().id(),
            "busy-b2c",
            "the end of the list is the end"
        );

        list.up();
        assert_eq!(list.selected().unwrap().id(), "ask-a1b");
        list.up();
        assert_eq!(list.selected().unwrap().id(), "ask-a1b");
    }

    #[test]
    fn view_keeps_the_cursor_on_the_agent_when_the_list_moves_under_it() {
        let mut list = listed(vec![
            view("ask-a1b", Phase::Waiting, 10),
            view("busy-b2c", Phase::Working, 20),
        ]);
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
    fn view_can_reach_the_fold_and_nothing_else_it_cannot_act_on() {
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
                "/src/web (1)",
                "busy-a1b",
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
                "/src/api (1)",
                "busy-a1b",
                "/aaa (1)",
                "idle-c3d",
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
            ["needs input (1)", "ask-a1b", "working (1)", "busy-b2c"]
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
    fn view_with_nothing_in_it_has_nothing_to_select() {
        let list = listed(Vec::new());
        assert!(list.is_empty());
        assert!(list.items().is_empty());
        assert!(list.selected().is_none());
        assert!(!list.on_fold());
    }
}
