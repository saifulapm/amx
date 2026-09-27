//! `amx wait` — one clock over several agents.
//!
//! `amx result <id>` blocks on one agent, so a caller holding five of them runs
//! five background subshells or polls `ls --json` on a loop of its own. This is
//! the one wait a coordinator wants: name the agents, and it says which of them
//! is ready as each becomes ready.
//!
//! Settled is a turn that is over or an agent stopped on a question — done,
//! failed, stopped, idle (a parked agent reads idle and counts here) or waiting.
//! A question counts because a wait that went through one would be the wait
//! [`crate::verbs::result`] refuses to be: the question arrives during the wait,
//! and a caller that cannot see it cannot answer it. `--for <state>` waits for
//! one named phase instead, so `--for working` is how a caller confirms a fleet
//! started.
//!
//! It says whose answer is ready and nothing about what the answer is: `amx
//! result <id>` is still what hands one back, and it returns at once for an
//! agent that has ended.

use anyhow::Result;
use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::derive::{self, Evidence};
use crate::store::{Agent, Phase};
use crate::verbs::result::{self, Ended, Settled};
use crate::{complain, exit, paths, store};

/// How often the records are read while waiting — `result`'s own poll, for the
/// same reason: short enough that a caller chaining turns is not waiting on
/// amx, long enough to cost nothing.
const POLL: Duration = Duration::from_millis(200);

/// How often a *pane* is read, once a record has gone quiet enough that a
/// reading needs one. Asking tmux for a screen five times a second is not free,
/// and here there is a screen per agent named.
const LOOK: Duration = Duration::from_secs(1);

/// Run the verb against the machine.
pub fn from_env(
    ids: &[String],
    children: Option<&str>,
    any: bool,
    state: Option<Phase>,
    timeout: Option<u64>,
) -> Result<i32> {
    let root = paths::state_root()?;
    let mut out = std::io::stdout().lock();
    run(
        &root,
        ids,
        children,
        any,
        state,
        timeout.map(Duration::from_secs),
        &mut out,
    )
}

/// The verb, with the state directory named.
///
/// Every id is opened before the first sweep, so an id nobody knows is a
/// refusal rather than a wait that could never end — and it is refused before
/// anything at all has been printed, while the caller can still fix what it
/// typed.
pub fn run(
    root: &Path,
    ids: &[String],
    children: Option<&str>,
    any: bool,
    state: Option<Phase>,
    timeout: Option<Duration>,
    out: &mut impl Write,
) -> Result<i32> {
    let named = match children {
        Some(parent) => children_of(root, parent)?,
        None => taken(ids),
    };
    // A family of none would settle at once and print nothing, which reads as
    // every child having settled.
    if let (Some(parent), true) = (children, named.is_empty()) {
        complain!("amx wait: {parent} has no children");
        return Ok(exit::FAILURE);
    }
    let mut pending = named;
    for id in &pending {
        Agent::open(root, id)?;
    }

    let deadline = timeout.map(|patience| Instant::now() + patience);
    loop {
        // The slowest pace among the agents still being waited on: one sleep
        // covers the whole sweep, and an agent whose reading costs a screen
        // must not have every other agent's record poll it.
        let mut slowest = POLL;
        let mut at = 0;
        while at < pending.len() {
            let view = derive::view(root, &pending[at], store::now())?;
            if !ready(root, &pending[at], view.phase(), state)? {
                slowest = slowest.max(pace(&view.verdict.evidence));
                at += 1;
                continue;
            }
            // Flushed a line at a time: a caller reading this as it comes is
            // the point, and a line held in a buffer until the last agent
            // settles says nothing sooner than `result` would have.
            writeln!(out, "{} {}", pending.remove(at), view.phase())?;
            out.flush()?;
            if any {
                return Ok(exit::OK);
            }
        }

        if pending.is_empty() {
            return Ok(exit::OK);
        }
        // After the sweep, so an agent that settled in the same beat the
        // patience ran out in is one that settled: what has been printed
        // stands whichever ending this is.
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            return Ok(exit::TIMEOUT);
        }
        std::thread::sleep(slowest);
    }
}

/// The agents to wait on, in the order they were named, each one once.
///
/// A caller assembling a command line from a list of its own has no reason to
/// have deduplicated it, and an id named twice is one agent, not two waits.
fn taken(ids: &[String]) -> Vec<String> {
    let mut taken: Vec<String> = Vec::with_capacity(ids.len());
    for id in ids {
        if !taken.iter().any(|already| already == id) {
            taken.push(id.clone());
        }
    }
    taken
}

/// The ids of every agent whose record names `parent`, in the order they
/// were created.
///
/// The parent itself is opened first, so an id nobody knows is refused the
/// way a named id is rather than reading as a family of none. A child whose
/// record has been removed is simply not here: the records are the list, and
/// nothing keeps a second one.
pub fn children_of(root: &Path, parent: &str) -> Result<Vec<String>> {
    Agent::open(root, parent)?;
    let mut children: Vec<(u64, String)> = Vec::new();
    for id in store::list(root)? {
        let Ok(agent) = Agent::open(root, &id) else {
            continue;
        };
        let Ok(meta) = agent.meta() else { continue };
        if meta.parent.as_deref() == Some(parent) {
            children.push((meta.created, id));
        }
    }
    children.sort();
    Ok(children.into_iter().map(|(_, id)| id).collect())
}

/// Whether this agent's reading is what the wait was for, reading its log
/// only where the answer turns on it.
pub(crate) fn ready(root: &Path, id: &str, phase: Phase, wanted: Option<Phase>) -> Result<bool> {
    let ended = match wanted {
        Some(_) => Ended::NotYet,
        None => result::ended(root, id, phase)?,
    };
    Ok(settled(phase, wanted, ended))
}

/// Whether this reading is what the wait was for.
///
/// With no `--for`, whatever `result` would stop waiting on: a turn that is
/// over, or an agent stopped on a question. `Starting` and `Working` are
/// agents still going, `Unknown` is amx not knowing — a reading nobody can act
/// on is not an agent that is ready — and an idle agent with a message still
/// in front of it has a turn to come.
fn settled(phase: Phase, wanted: Option<Phase>, ended: Ended) -> bool {
    match wanted {
        Some(wanted) => phase == wanted,
        None => result::settled(phase, ended) != Settled::NotYet,
    }
}

/// How long to wait before reading again, given what the last reading cost.
fn pace(evidence: &Evidence) -> Duration {
    match evidence {
        Evidence::Screen | Evidence::Unknown => LOOK,
        Evidence::Record | Evidence::Gone | Evidence::LetGo | Evidence::Hooks => POLL,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{Event, Meta};
    use crate::tmux::{PaneId, Socket};

    #[test]
    fn a_turn_that_is_over_or_a_question_is_what_a_wait_is_for() {
        for phase in [
            Phase::Waiting,
            Phase::Idle,
            Phase::Done,
            Phase::Failed,
            Phase::Stopped,
        ] {
            assert!(settled(phase, None, Ended::Turn), "{phase}");
        }
        // Still going, or amx not knowing: neither is an agent whose answer a
        // caller can go and take.
        for phase in [Phase::Starting, Phase::Working, Phase::Unknown] {
            assert!(!settled(phase, None, Ended::Turn), "{phase}");
        }
    }

    #[test]
    fn a_message_nothing_has_answered_keeps_an_idle_agent_from_settling() {
        // Idle after a turn cut short by hand, with the message amx sent still
        // in front of it: the answer on the record is the turn before's.
        let root = tempfile::TempDir::new().unwrap();
        record(root.path(), "a", Phase::Idle);
        let said = |kind: &str| {
            Agent::open(root.path(), "a")
                .unwrap()
                .writer()
                .unwrap()
                .append(&Event::new(
                    kind,
                    serde_json::json!({ "text": "and the linter" }),
                ))
                .unwrap();
        };
        said(crate::verbs::send::SEND);

        let patience = Some(Duration::ZERO);
        let (code, printed) = waited(root.path(), &["a"], false, None, patience);
        assert_eq!(code, exit::TIMEOUT);
        assert_eq!(printed, "");

        // A resume ends the turn the message asked for, and the wait with it.
        said("resume");
        let (code, printed) = waited(root.path(), &["a"], false, None, patience);
        assert_eq!(code, exit::OK);
        assert_eq!(printed, "a idle\n");
    }

    #[test]
    fn for_a_named_phase_waits_for_that_one_and_no_other() {
        // `--for working` is how a caller confirms a fleet started, so the
        // phases the plain wait ends on are no longer endings.
        assert!(settled(Phase::Working, Some(Phase::Working), Ended::NotYet));
        assert!(!settled(Phase::Done, Some(Phase::Working), Ended::NotYet));
        assert!(!settled(
            Phase::Waiting,
            Some(Phase::Working),
            Ended::NotYet
        ));
        assert!(settled(Phase::Unknown, Some(Phase::Unknown), Ended::NotYet));
    }

    #[test]
    fn an_agent_named_twice_is_one_agent() {
        assert_eq!(taken(&ids(&["b", "a", "b"])), ["b", "a"]);
        assert_eq!(taken(&ids(&["a"])), ["a"]);
    }

    #[test]
    fn a_wait_reads_the_records_often_and_the_panes_rarely() {
        for evidence in [
            Evidence::Record,
            Evidence::Gone,
            Evidence::Hooks,
            Evidence::LetGo,
        ] {
            assert_eq!(pace(&evidence), POLL, "{evidence:?}");
        }
        for evidence in [Evidence::Screen, Evidence::Unknown] {
            assert_eq!(pace(&evidence), LOOK, "{evidence:?}");
        }
    }

    #[test]
    fn every_agent_is_printed_as_it_settles_and_the_wait_ends_with_the_last() {
        let root = tempfile::TempDir::new().unwrap();
        record(root.path(), "a", Phase::Done);
        record(root.path(), "b", Phase::Idle);

        let (code, said) = waited(root.path(), &["a", "b"], false, None, None);
        assert_eq!(code, exit::OK);
        assert_eq!(said, "a done\nb idle\n");
    }

    #[test]
    fn any_ends_at_the_first_agent_to_settle_and_leaves_the_rest_running() {
        let root = tempfile::TempDir::new().unwrap();
        record(root.path(), "a", Phase::Working);
        record(root.path(), "b", Phase::Failed);

        let (code, said) = waited(root.path(), &["a", "b"], true, None, None);
        assert_eq!(code, exit::OK);
        assert_eq!(
            said, "b failed\n",
            "the one still working is not waited out"
        );
    }

    #[test]
    fn a_wait_that_runs_out_of_patience_keeps_what_settled() {
        // Exit 3 is the caller's own deadline, and the agents that did settle
        // before it are ready whatever this exits with: their lines stand.
        let root = tempfile::TempDir::new().unwrap();
        record(root.path(), "a", Phase::Working);
        record(root.path(), "b", Phase::Done);

        let patience = Some(Duration::ZERO);
        let (code, said) = waited(root.path(), &["a", "b"], false, None, patience);
        assert_eq!(code, exit::TIMEOUT);
        assert_eq!(said, "b done\n");
    }

    #[test]
    fn a_wait_for_a_phase_ends_on_that_phase() {
        let root = tempfile::TempDir::new().unwrap();
        record(root.path(), "a", Phase::Working);

        let working = Some(Phase::Working);
        let (code, said) = waited(root.path(), &["a"], false, working, None);
        assert_eq!(code, exit::OK);
        assert_eq!(said, "a working\n");

        // And an agent at work is not one whose turn is over, however long
        // anybody waits for it.
        let patience = Some(Duration::ZERO);
        let (code, said) = waited(root.path(), &["a"], false, None, patience);
        assert_eq!(code, exit::TIMEOUT);
        assert_eq!(said, "");
    }

    #[test]
    fn an_id_that_names_no_agent_is_refused_before_anything_is_waited_on() {
        // A wait on an agent that does not exist could never end, and the
        // caller can still fix what it typed: nothing is printed, and the
        // refusal names the id it could not find.
        let root = tempfile::TempDir::new().unwrap();
        record(root.path(), "a", Phase::Done);

        let mut out = Vec::new();
        let refused = run(
            root.path(),
            &ids(&["a", "nope"]),
            None,
            false,
            None,
            None,
            &mut out,
        )
        .expect_err("an id nobody knows");
        assert!(refused.to_string().contains("nope"), "{refused:#}");
        assert!(out.is_empty(), "it printed {out:?} before refusing");
    }

    #[test]
    fn children_of_a_childless_parent_is_a_failure() {
        // A wait on a family of none would end at once and say nothing, which
        // reads as every child having settled.
        let root = tempfile::TempDir::new().unwrap();
        record(root.path(), "lonely-a1b", Phase::Idle);

        let mut out = Vec::new();
        let code = run(
            root.path(),
            &[],
            Some("lonely-a1b"),
            false,
            None,
            None,
            &mut out,
        )
        .unwrap();
        assert_eq!(code, exit::FAILURE);
        assert!(out.is_empty(), "it printed {out:?}");
    }

    fn ids(ids: &[&str]) -> Vec<String> {
        ids.iter().map(ToString::to_string).collect()
    }

    /// One wait, and what it printed.
    fn waited(
        root: &Path,
        named: &[&str],
        any: bool,
        state: Option<Phase>,
        timeout: Option<Duration>,
    ) -> (i32, String) {
        let mut out = Vec::new();
        let code = run(root, &ids(named), None, any, state, timeout, &mut out).unwrap();
        (code, String::from_utf8(out).unwrap())
    }

    /// An agent whose record says this phase and whose pane is gone.
    ///
    /// Parked, so a reading with no pane to look at hands back the phase on the
    /// record — which is what lets one test name a phase and get it.
    fn record(root: &Path, id: &str, phase: Phase) {
        let meta = Meta {
            role: None,
            parent: None,
            depth: 0,
            id: id.to_string(),
            task: "fix the login bug".to_string(),
            agent: Some("claude".to_string()),
            model: None,
            effort: None,
            dir: std::path::PathBuf::from("/srv/app"),
            worktree: None,
            branch: None,
            base: None,
            socket: Socket::Name(format!("amx-no-such-server-{}", std::process::id())),
            pane: PaneId::new("%404").unwrap(),
            bg: false,
            session: None,
            transcript: None,
            created: 1,
        };
        let agent = Agent::create(root, &meta).unwrap();
        let writer = agent.writer().unwrap();
        writer
            .append(&Event::new("SessionStart", serde_json::json!({})))
            .unwrap();
        writer
            .observe(|state| {
                state.state = phase;
                state.parked_at = 4_600;
            })
            .unwrap();
    }
}
