//! `amx ls` — every agent, and what it is doing.
//!
//! Two audiences read this. A person wants a short table they can take in at a
//! glance; a program wants a shape it can branch on without parsing English,
//! which is what `--json` is for. Both answer from the same reading, so they
//! can never disagree.

use anyhow::{Context, Result};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::derive::{self, View};
use crate::store::{Meta, now};
use crate::verbs::send;
use crate::{exit, gc, paths, rules, worktree};

/// Run the verb against the machine.
pub fn from_env(json: bool, dir: Option<&Path>) -> Result<i32> {
    let root = paths::state_root()?;
    let scope = Scope::of(dir)?;
    let mut out = std::io::stdout().lock();
    run(&root, json, &scope, now(), &mut out)
}

/// The verb, with the state directory and the clock named.
pub fn run(root: &Path, json: bool, scope: &Scope, now: u64, out: &mut impl Write) -> Result<i32> {
    // Listing is the moment amx tidies up after itself: it is run often, and
    // nobody is waiting on its answer the way a caller waits on `result`.
    let _ = gc::sweep(root, now);

    // Narrowed once, before either reader is answered, so the table and the
    // JSON are the same reading of the same agents.
    let views = scope.narrow(derive::views(root, rules::bundled(), now)?);
    if json {
        let listed: Vec<_> = views.iter().map(View::json).collect();
        writeln!(out, "{}", serde_json::to_string_pretty(&listed)?)?;
    } else {
        table(&views, out)?;
    }
    Ok(exit::OK)
}

/// Which agents a reading is about.
///
/// Every agent on the machine is the answer amx has always given, and from a
/// terminal that is not anywhere in particular it is the right one. From
/// inside a project it is not: the agents of the repository in front of you
/// are a handful of the rows and the rest is somebody else's afternoon, read
/// past every time. `--dir` says which directory the question is about, and
/// the reading answers about that directory alone.
///
/// It narrows the reading rather than the record. Nothing is written down, no
/// agent is hidden from any other surface, and the same agent is in two
/// readings at once when the directories nest.
#[derive(Debug, Clone, Default)]
pub struct Scope {
    /// The directory the reading is about, `None` being the whole machine.
    under: Option<PathBuf>,
}

impl Scope {
    /// The scope a command line named, or every agent when it named none.
    pub fn of(dir: Option<&Path>) -> Result<Scope> {
        Ok(Scope {
            under: dir.map(named).transpose()?,
        })
    }

    /// Whether the reading is about this agent: it runs under the directory,
    /// or the repository its worktree was cut from is under it.
    pub fn covers(&self, meta: &Meta) -> bool {
        let Some(under) = self.under.as_deref() else {
            return true;
        };
        sits_under(&meta.dir, under) || cut_from(meta).is_some_and(|repo| sits_under(&repo, under))
    }

    /// A reading with only the agents the scope is about left in it.
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

/// A directory as it was typed, read as amx will compare it: absolute, and
/// through whatever links the shell reached it by.
///
/// A directory that is not there is not an error. Records outlive the trees
/// they name — a repository moved or deleted leaves its agents behind — and
/// `amx ls --dir` on one of those is a fair question with an answer. What it
/// cannot be is relative to nowhere, so the path is anchored on the working
/// directory whether or not the disk knows it.
fn named(dir: &Path) -> Result<PathBuf> {
    let anchored = std::path::absolute(dir)
        .with_context(|| format!("reading the directory `{}`", dir.display()))?;
    Ok(std::fs::canonicalize(&anchored).unwrap_or(anchored))
}

/// Whether a directory is the one named or inside it.
///
/// Compared as paths first, which asks no disk and is the answer for the
/// records amx writes: the directory in a record is the one the agent was
/// started in, spelled out from the root. A shell that reached the same
/// directory another way — a link into a checkout, `/tmp` where `/tmp` is a
/// link — is asked of the disk instead, once, and only after the plain
/// comparison has already said no.
fn sits_under(dir: &Path, under: &Path) -> bool {
    dir.starts_with(under)
        || std::fs::canonicalize(dir).is_ok_and(|reached| reached.starts_with(under))
}

/// The repository an agent's worktree was cut from, if it is in one amx cut.
///
/// A worktree of amx's own shape is `<repo>/.amx/worktrees/<id>`, so the
/// repository is three components back up the path: string work, no disk, and
/// the same law `stop` and the view read a tree by. It is what a person means
/// by the project an agent belongs to — a worktree agent of `~/code/amx` is an
/// agent of `~/code/amx`, whatever directory it happens to run in.
fn cut_from(meta: &Meta) -> Option<PathBuf> {
    let tree = meta.worktree.as_deref().unwrap_or(&meta.dir);
    worktree::is_amx_tree(tree)
        .then(|| tree.ancestors().nth(3))
        .flatten()
        .map(Path::to_path_buf)
}

/// The table a person reads.
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
            view.phase().as_str(),
            view.id(),
            age(view),
            doing(view),
        )?;
    }
    Ok(())
}

/// What this agent is up to, as a row can carry it: what it is waiting to be
/// told, with the choices it is waiting to be told from, else what it is doing.
///
/// The choices ride the row because they are short, they are numbered, and the
/// number is the whole of the answer — a person scanning a wall for the agent
/// that is blocked can answer it without opening anything. There are none to
/// carry unless a question is outstanding: they are cleared with it.
fn doing(view: &View) -> String {
    let mut said = inert(first_line(view.line().unwrap_or("")));
    for choice in send::numbered(&view.state.options) {
        said.push_str("  ");
        said.push_str(&inert(&choice));
    }
    said
}

/// A string amx did not author, on one line and unable to drive the terminal
/// it prints into. Both halves are the table's: a row is a row, and the bytes
/// in it came from a program amx does not control.
fn inert(text: &str) -> String {
    crate::tmux::sanitize(first_line(text)).trim().to_string()
}

/// The reading's own number, in the shortest form that says it: how long a
/// finished run worked, how long a waiting agent has waited, and how long since
/// anything was heard from one still going.
///
/// The number is the reading's and not this table's, and the view says the
/// same one in the same units off the same reading, so a person who has both
/// open is never told two things about one agent.
fn age(view: &View) -> String {
    let seconds = view.verdict.age;
    match seconds {
        0..=59 => format!("{seconds}s"),
        60..=3599 => format!("{}m", seconds / 60),
        3600..=86_399 => format!("{}h", seconds / 3600),
        _ => format!("{}d", seconds / 86_400),
    }
}

/// One line of it, so a paragraph of an answer cannot take over the table.
fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or("").trim()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::derive::{Evidence, Verdict};
    use crate::store::{Meta, Phase, State};
    use crate::tmux::{PaneId, Socket};
    use std::path::PathBuf;

    fn meta(id: &str, created: u64) -> Meta {
        Meta {
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
            },
        }
    }

    /// A row worked out from a record the way `ls` works one out, rather than
    /// written by hand: the last column is the reader's number, and this is
    /// the surface a person reads it off.
    fn reading(id: &str, state: State, created: u64, now: u64) -> View {
        let verdict =
            derive::read(&state, created, true, || None, rules::bundled(), now, 1).verdict;
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
        // An answer is a paragraph and a row is a row.
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
    fn reader_the_last_column_ticks_while_an_agent_runs_and_freezes_when_it_ends() {
        let mut record = State {
            state: Phase::Working,
            since: 1_000,
            last_event: 1_000,
            summary: Some("Running Bash".to_string()),
            ..State::default()
        };

        // Still going: how long since anything was heard, moving with the
        // clock, which is what says whether the rest of the row is worth
        // believing.
        for (now, said) in [(1_004, "4s"), (1_008, "8s")] {
            let text = printed(&[reading("fix-login-a1b", record.clone(), 1_000, now)]);
            assert!(text.contains(said), "{text}");
        }

        // It worked ten seconds and stood at a question for the hour before
        // that. Read an hour later and a day later, it is the run it was both
        // times, and the hour it spent waiting is nobody's ten seconds.
        record.state = Phase::Done;
        record.since = 4_610;
        record.last_event = 4_610;
        record.ended = 4_610;
        record.worked = 10;
        record.result = Some("the tests pass now".to_string());

        let hour = printed(&[reading("fix-login-a1b", record.clone(), 1_000, 8_210)]);
        assert!(hour.contains("10s"), "{hour}");
        assert_eq!(
            printed(&[reading("fix-login-a1b", record.clone(), 1_000, 90_000)]),
            hour,
            "a row of a run that worked ten seconds says ten seconds"
        );

        // The row is the reading's own number put into words, and not a second
        // number this table worked out for itself. It is what the view reads
        // and how the view says it, so the two surfaces cannot disagree.
        let read = reading("fix-login-a1b", record, 1_000, 90_000);
        assert_eq!(read.verdict.age, 10);
        assert!(hour.contains(&age(&read)), "{hour}");
    }

    /// An agent of a directory, with the worktree amx cut for it if it has
    /// one.
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
            // The one a comparison of strings would have taken with it.
            ran_in("alike-c3d", "/srv/app2", None),
            ran_in("elsewhere-d4e", "/srv/other", None),
        ];

        let scope = Scope::of(Some(Path::new("/srv/app"))).unwrap();
        assert_eq!(ids(scope.narrow(views.clone())), ["here-a1b", "deeper-b2c"]);

        // The directory itself is under itself, and nothing else is.
        let one = Scope::of(Some(Path::new("/srv/app/importer"))).unwrap();
        assert_eq!(ids(one.narrow(views.clone())), ["deeper-b2c"]);

        // A reading that names no directory is the machine, which is what
        // `amx ls` has always answered.
        assert_eq!(ids(Scope::of(None).unwrap().narrow(views.clone())).len(), 4);
        assert_eq!(ids(Scope::default().narrow(views)).len(), 4);
    }

    #[test]
    fn ls_an_agent_in_a_worktree_belongs_to_the_repository_it_was_cut_from() {
        // What it runs in is `<repo>/.amx/worktrees/<id>`, and what a person
        // means by it is the repository.
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

        // And the tree it runs in is under the directory it runs in, so
        // naming that reaches it too.
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

        // The link and the tree are the same directory, and an agent started
        // through either is an agent of it.
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

        // `--dir .` is the directory the shell is in, which is the whole
        // reason a relative path is worth taking at all.
        let here = std::env::current_dir().unwrap();
        assert_eq!(
            named(Path::new("src")).unwrap(),
            std::fs::canonicalize(here.join("src")).unwrap()
        );
    }

    #[test]
    fn ls_a_directory_that_is_not_there_answers_rather_than_fails() {
        // A record outlives the tree it names: a repository somebody deleted
        // leaves its agents behind, and asking after them is a fair question.
        let scope = Scope::of(Some(Path::new("/srv/gone"))).unwrap();
        let views = vec![
            ran_in("left-a1b", "/srv/gone/api", None),
            ran_in("elsewhere-b2c", "/srv/other", None),
        ];
        assert_eq!(ids(scope.narrow(views)), ["left-a1b"]);
    }

    #[test]
    fn reader_ages_read_as_a_person_would_say_them() {
        let aged = |seconds| age(&view("x-a1b", Phase::Idle, seconds, None));
        assert_eq!(aged(0), "0s");
        assert_eq!(aged(59), "59s");
        assert_eq!(aged(60), "1m");
        assert_eq!(aged(3_599), "59m");
        assert_eq!(aged(3_600), "1h");
        assert_eq!(aged(86_400), "1d");
    }
}
