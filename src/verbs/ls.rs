//! `amx ls`: every agent and what it is doing.
//!
//! A table for a person, or with `--json` a stable shape for a program. Both
//! come from the same reading. `ls` is also where finished records are swept.

use anyhow::Result;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::derive::{self, View};
use crate::store::{Meta, now};
use crate::verbs::send;
use crate::{exit, gc, paths, spawn};

/// Run the verb against the machine.
pub fn from_env(json: bool, dir: Option<&Path>) -> Result<i32> {
    let root = paths::state_root()?;
    let scope = Scope::of(dir)?;
    let mut out = std::io::stdout().lock();
    run(&root, json, &scope, now(), &mut out)
}

/// The verb, with the state directory and the clock named.
pub fn run(root: &Path, json: bool, scope: &Scope, now: u64, out: &mut impl Write) -> Result<i32> {
    // Read once: the sweep drops what it forgets, and the reading takes the
    // rest, so no state document is parsed twice.
    let records = gc::sweep(derive::records(root)?, now);

    // Narrowed before the reading, so an agent outside the scope costs no
    // screen and no summary.
    let records = records
        .into_iter()
        .filter(|record| scope.covers(&record.meta))
        .collect();
    let views = derive::views_of(root, records, now);
    if json {
        let listed: Vec<_> = views.iter().map(View::json).collect();
        writeln!(out, "{}", serde_json::to_string_pretty(&listed)?)?;
    } else {
        table(&views, out)?;
    }
    Ok(exit::OK)
}

/// Which agents a reading is about: every agent, or with `--dir` those under
/// one directory.
///
/// Only the reading is narrowed. Nothing is written to any record.
#[derive(Debug, Clone, Default)]
pub struct Scope {
    /// `None` for the whole machine.
    under: Option<PathBuf>,
}

impl Scope {
    /// The scope of `--dir`, or every agent without it.
    pub fn of(dir: Option<&Path>) -> Result<Scope> {
        Ok(Scope {
            under: dir.map(named).transpose()?,
        })
    }

    pub fn under(&self) -> Option<&Path> {
        self.under.as_deref()
    }

    /// Whether this agent runs under the directory, or its worktree was cut
    /// from a repository under it.
    pub fn covers(&self, meta: &Meta) -> bool {
        let Some(under) = self.under.as_deref() else {
            return true;
        };
        sits_under(&meta.dir, under) || sits_under(&spawn::project_dir(meta), under)
    }

    /// Keep only the views this scope covers.
    pub fn narrow(&self, views: Vec<View>) -> Vec<View> {
        match self.under {
            None => views,
            Some(_) => views
                .into_iter()
                .filter(|view| self.covers(&view.meta))
                .collect(),
        }
    }
}

/// The typed directory, made absolute.
///
/// A directory that does not exist is not an error: records outlive the trees
/// they name.
fn named(dir: &Path) -> Result<PathBuf> {
    paths::anchored(dir)
}

/// Whether `dir` is `under` or inside it.
///
/// Compared as paths first, then canonicalized, for a directory reached
/// through a symlink.
fn sits_under(dir: &Path, under: &Path) -> bool {
    dir.starts_with(under)
        || std::fs::canonicalize(dir).is_ok_and(|reached| reached.starts_with(under))
}

fn table(views: &[View], out: &mut impl Write) -> Result<()> {
    if views.is_empty() {
        writeln!(out, "no agents")?;
        return Ok(());
    }

    let widest = views.iter().map(|view| view.id().len()).max().unwrap_or(0);
    for view in views {
        writeln!(
            out,
            "{:<8} {:<widest$}  {:>5}  {}",
            view.phase().word(),
            view.id(),
            worked(view),
            doing(view),
        )?;
    }
    Ok(())
}

/// The row's last column: the agent's line on one line, followed by the
/// numbered choices of a pending question.
fn doing(view: &View) -> String {
    let mut said = super::inert_line(view.line().unwrap_or(""));
    for choice in send::numbered(&view.state.options) {
        said.push_str("  ");
        said.push_str(&super::inert_line(&choice));
    }
    said
}

/// The seconds this agent has worked, in the view's own units.
fn worked(view: &View) -> String {
    derive::in_words(view.verdict.worked)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::derive::{Evidence, Verdict};
    use crate::rules;
    use crate::store::{Meta, Phase, State};
    use crate::tmux::{PaneId, Socket};
    use std::path::PathBuf;

    fn meta(id: &str, created: u64) -> Meta {
        Meta {
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
            created,
        }
    }

    fn view(id: &str, phase: Phase, age: u64, line: Option<&str>) -> View {
        View {
            meta: meta(id, 1),
            state: State {
                state: phase,
                summary: line.map(str::to_string),
                ..State::default()
            },
            verdict: Verdict {
                phase,
                evidence: Evidence::Hooks,
                rule: None,
                age,
                worked: age,
            },
            doing: None,
        }
    }

    /// A view read from `state` the way `ls` reads one.
    fn reading(id: &str, state: State, created: u64, now: u64) -> View {
        let verdict = derive::read(
            &state,
            created,
            true,
            || None,
            rules::of("claude"),
            true,
            now,
            1,
            None,
        )
        .verdict;
        View::new(meta(id, created), state, verdict)
    }

    fn printed(views: &[View]) -> String {
        let mut out = Vec::new();
        table(views, &mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn reader_the_table_says_the_state_the_name_and_what_it_is_doing() {
        let text = printed(&[view(
            "fix-login-a1b",
            Phase::Working,
            12,
            Some("Running Bash"),
        )]);
        assert!(text.contains("working"), "{text}");
        assert!(text.contains("fix-login-a1b"), "{text}");
        assert!(text.contains("12s"), "{text}");
        assert!(text.contains("Running Bash"), "{text}");
    }

    #[test]
    fn reader_the_table_keeps_one_row_to_one_line() {
        let text = printed(&[view(
            "fix-login-a1b",
            Phase::Idle,
            1,
            Some("I fixed it.\n\nHere is what I changed:\n- the parser"),
        )]);
        assert_eq!(text.lines().count(), 1, "{text}");
        assert!(text.contains("I fixed it."), "{text}");
        assert!(!text.contains("the parser"), "{text}");
    }

    #[test]
    fn reader_the_table_says_so_when_there_is_nothing_to_say() {
        assert_eq!(printed(&[]).trim(), "no agents");
    }

    #[test]
    fn reader_the_last_column_ticks_while_an_agent_works_and_freezes_when_it_stops() {
        let mut record = State {
            state: Phase::Working,
            since: 1_000,
            last_event: 1_000,
            summary: Some("Running Bash".to_string()),
            ..State::default()
        };

        // Working: the open span counts and grows with the clock.
        for (now, said) in [(1_004, "4s"), (1_008, "8s")] {
            let text = printed(&[reading("fix-login-a1b", record.clone(), 1_000, now)]);
            assert!(text.contains(said), "{text}");
        }

        // Waiting on a question: the clock stops.
        record.state = Phase::Waiting;
        record.since = 1_010;
        record.last_event = 1_010;
        record.worked = 10;
        record.question = Some("Which fixture should the port keep?".to_string());
        let text = printed(&[reading(
            "fix-login-a1b",
            record.clone(),
            1_000,
            1_010 + derive::FRESH,
        )]);
        assert!(text.contains("10s"), "{text}");

        // Ended: the total stays fixed however late it is read.
        record.state = Phase::Done;
        record.since = 4_610;
        record.last_event = 4_610;
        record.ended = 4_610;
        record.question = None;
        record.result = Some("the tests pass now".to_string());

        let hour = printed(&[reading("fix-login-a1b", record.clone(), 1_000, 8_210)]);
        assert!(hour.contains("10s"), "{hour}");
        assert_eq!(
            printed(&[reading("fix-login-a1b", record.clone(), 1_000, 90_000)]),
            hour,
            "a row of a run that worked ten seconds says ten seconds"
        );

        // The column is the reading's number in the reading's units, so `ls`
        // and the view agree.
        let read = reading("fix-login-a1b", record, 1_000, 90_000);
        assert_eq!(read.verdict.worked, 10);
        assert_eq!(worked(&read), derive::in_words(read.verdict.worked));
        assert!(hour.contains(&worked(&read)), "{hour}");
    }

    /// A view of an agent in `dir`, with its worktree if it has one.
    fn ran_in(id: &str, dir: &str, worktree: Option<&str>) -> View {
        let mut view = view(id, Phase::Working, 1, None);
        view.meta.dir = PathBuf::from(dir);
        view.meta.worktree = worktree.map(PathBuf::from);
        view
    }

    fn ids(views: Vec<View>) -> Vec<String> {
        views.iter().map(|view| view.id().to_string()).collect()
    }

    #[test]
    fn ls_a_reading_of_a_directory_is_the_agents_under_it() {
        let views = vec![
            ran_in("here-a1b", "/srv/app", None),
            ran_in("deeper-b2c", "/srv/app/importer", None),
            // A string prefix match would wrongly include this one.
            ran_in("alike-c3d", "/srv/app2", None),
            ran_in("elsewhere-d4e", "/srv/other", None),
        ];

        let scope = Scope::of(Some(Path::new("/srv/app"))).unwrap();
        assert_eq!(ids(scope.narrow(views.clone())), ["here-a1b", "deeper-b2c"]);

        let one = Scope::of(Some(Path::new("/srv/app/importer"))).unwrap();
        assert_eq!(ids(one.narrow(views.clone())), ["deeper-b2c"]);

        // No directory is the whole machine.
        assert_eq!(ids(Scope::of(None).unwrap().narrow(views.clone())).len(), 4);
        assert_eq!(ids(Scope::default().narrow(views)).len(), 4);
    }

    #[test]
    fn ls_an_agent_in_a_worktree_belongs_to_the_repository_it_was_cut_from() {
        // It runs in `<repo>/.amx/worktrees/<id>` and belongs to `<repo>`.
        let tree = "/srv/app/.amx/worktrees/fix-login-a1b";
        let cut = ran_in("fix-login-a1b", tree, Some(tree));
        let elsewhere = ran_in(
            "port-it-b2c",
            "/srv/other/.amx/worktrees/port-it-b2c",
            Some("/srv/other/.amx/worktrees/port-it-b2c"),
        );
        let views = vec![cut, elsewhere];

        let scope = Scope::of(Some(Path::new("/srv/app"))).unwrap();
        assert_eq!(ids(scope.narrow(views.clone())), ["fix-login-a1b"]);

        // Naming the tree itself covers it too.
        let inside = Scope::of(Some(Path::new(tree))).unwrap();
        assert_eq!(ids(inside.narrow(views)), ["fix-login-a1b"]);
    }

    #[test]
    fn ls_a_directory_is_the_one_the_shell_reached_however_it_reached_it() {
        let repo = tempfile::TempDir::new().unwrap();
        let real = std::fs::canonicalize(repo.path()).unwrap();
        std::fs::create_dir(real.join("importer")).unwrap();

        let link = tempfile::TempDir::new().unwrap();
        let link = std::fs::canonicalize(link.path()).unwrap().join("app");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let views = vec![
            ran_in("here-a1b", real.join("importer").to_str().unwrap(), None),
            ran_in("elsewhere-b2c", "/srv/other", None),
        ];

        // A symlink and its target name the same directory.
        for named in [&real, &link] {
            let scope = Scope::of(Some(named)).unwrap();
            assert_eq!(ids(scope.narrow(views.clone())), ["here-a1b"], "{named:?}");
        }

        let through = vec![ran_in(
            "here-a1b",
            link.join("importer").to_str().unwrap(),
            None,
        )];
        let scope = Scope::of(Some(&real)).unwrap();
        assert_eq!(ids(scope.narrow(through)), ["here-a1b"]);

        // A relative path is taken from the working directory.
        let here = std::env::current_dir().unwrap();
        assert_eq!(
            named(Path::new("src")).unwrap(),
            std::fs::canonicalize(here.join("src")).unwrap()
        );
    }

    #[test]
    fn ls_a_directory_that_is_not_there_answers_rather_than_fails() {
        // Records outlive the trees they name, so a missing directory still
        // has agents under it.
        let scope = Scope::of(Some(Path::new("/srv/gone"))).unwrap();
        let views = vec![
            ran_in("left-a1b", "/srv/gone/api", None),
            ran_in("elsewhere-b2c", "/srv/other", None),
        ];
        assert_eq!(ids(scope.narrow(views)), ["left-a1b"]);
    }

    /// A record on disk with the given state, written directly so nothing
    /// stamps the current time.
    fn on_disk(root: &Path, id: &str, phase: Phase, last_event: u64) {
        let agent = crate::store::Agent::create(root, &meta(id, 1)).expect("a record");
        let state = State {
            state: phase,
            last_event,
            since: last_event,
            ..State::default()
        };
        std::fs::write(
            agent.dir().join("state.json"),
            serde_json::to_string(&state).unwrap(),
        )
        .expect("a record");
    }

    #[test]
    fn ls_sweeps_and_answers_from_the_one_reading_it_took() {
        const NOW: u64 = 1_800_000_000;
        let root = tempfile::TempDir::new().unwrap();
        on_disk(root.path(), "old-done-a1b", Phase::Done, NOW - gc::KEEP - 1);
        on_disk(root.path(), "just-done-c3d", Phase::Done, NOW - 60);

        let mut out = Vec::new();
        assert_eq!(
            run(root.path(), false, &Scope::default(), NOW, &mut out).unwrap(),
            exit::OK
        );

        // The table shows what the sweep kept, and the forgotten record is
        // gone from disk.
        let text = String::from_utf8(out).unwrap();
        assert_eq!(text.lines().count(), 1, "{text}");
        assert!(text.contains("just-done-c3d"), "{text}");
        assert_eq!(crate::store::list(root.path()).unwrap(), ["just-done-c3d"]);
    }

    #[test]
    fn ls_dir_reads_the_agents_under_it_and_sweeps_every_record() {
        const NOW: u64 = 1_800_000_000;
        let root = tempfile::TempDir::new().unwrap();
        on_disk(root.path(), "here-a1b", Phase::Done, NOW - 60);
        on_disk(root.path(), "there-c3d", Phase::Done, NOW - 60);
        on_disk(
            root.path(),
            "old-there-e5f",
            Phase::Done,
            NOW - gc::KEEP - 1,
        );
        for id in ["there-c3d", "old-there-e5f"] {
            crate::store::Agent::open(root.path(), id)
                .unwrap()
                .writer()
                .unwrap()
                .update_meta(|meta| meta.dir = PathBuf::from("/srv/other"))
                .unwrap();
        }

        let scope = Scope::of(Some(Path::new("/srv/app"))).unwrap();
        let mut out = Vec::new();
        assert_eq!(
            run(root.path(), false, &scope, NOW, &mut out).unwrap(),
            exit::OK
        );

        let text = String::from_utf8(out).unwrap();
        assert_eq!(text.lines().count(), 1, "{text}");
        assert!(text.contains("here-a1b"), "{text}");
        let mut left = crate::store::list(root.path()).unwrap();
        left.sort();
        assert_eq!(left, ["here-a1b", "there-c3d"]);
    }

    #[test]
    fn reader_durations_read_as_a_person_would_say_them() {
        let said = |seconds| worked(&view("x-a1b", Phase::Idle, seconds, None));
        assert_eq!(said(0), "0s");
        assert_eq!(said(59), "59s");
        assert_eq!(said(60), "1m");
        assert_eq!(said(3_599), "59m");
        assert_eq!(said(3_600), "1h");
        assert_eq!(said(86_400), "1d");
    }
}
