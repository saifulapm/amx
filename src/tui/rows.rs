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

use crate::derive::View;
use crate::store::Phase;

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

/// One line of the list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Item {
    /// A group, and how many agents are under it.
    Heading(Group, usize),
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
#[derive(Debug, Default)]
pub struct List {
    views: Vec<View>,
    items: Vec<Item>,
    cursor: usize,
    unfolded: bool,
}

impl List {
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

    /// How many agents are in each group that has any.
    pub fn counts(&self) -> Vec<(Group, usize)> {
        self.items
            .iter()
            .filter_map(|item| match item {
                Item::Heading(group, count) => Some((*group, *count)),
                _ => None,
            })
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.views.is_empty()
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
        let mut items = Vec::new();
        for group in Group::ALL {
            let mut members: Vec<usize> = (0..self.views.len())
                .filter(|&n| Group::of(self.views[n].phase()) == group)
                .collect();
            if members.is_empty() {
                continue;
            }
            if group == Group::Completed {
                // Newest ending first: what just finished is what somebody
                // scanning a list of finished agents came for.
                members.sort_by(|&a, &b| {
                    ended(&self.views[b])
                        .cmp(&ended(&self.views[a]))
                        .then_with(|| self.views[a].id().cmp(self.views[b].id()))
                });
            }

            let shown = if group == Group::Completed && !self.unfolded {
                FOLD.min(members.len())
            } else {
                members.len()
            };
            items.push(Item::Heading(group, members.len()));
            items.extend(members[..shown].iter().map(|&n| Item::Agent(n)));
            if shown < members.len() {
                items.push(Item::Fold(members.len() - shown));
            }
        }

        self.items = items;
        self.settle();
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

    /// The list as a person reads it down the screen.
    fn lines(list: &List) -> Vec<String> {
        list.items()
            .iter()
            .map(|item| match item {
                Item::Heading(group, count) => format!("{} ({count})", group.title()),
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
    fn view_with_nothing_in_it_has_nothing_to_select() {
        let list = listed(Vec::new());
        assert!(list.is_empty());
        assert!(list.items().is_empty());
        assert!(list.selected().is_none());
        assert!(!list.on_fold());
    }
}
