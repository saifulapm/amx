//! `amx wait`: wait on several agents at once and print each as it settles.
//!
//! An agent settles when its turn is over (done, failed, stopped, idle, which
//! includes parked) or when it is waiting on a question, for the same reason
//! [`crate::verbs::result`] never waits through one. `--for <state>` waits for
//! that phase instead, e.g. `--for working` to confirm a fleet started. Only
//! ids and phases are printed; `amx result <id>` hands back the answer.

use anyhow::{Context, Result};
use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::derive::{self, Record, View};
use crate::store::{Agent, Phase};
use crate::verbs::result::{self, Ended, POLL, Settled, Turns, pace};
use crate::{complain, exit, paths, store};

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
/// Every id is opened before the first sweep, so an unknown id fails before
/// anything is printed.
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
    // An empty family would settle at once and print nothing, which reads as
    // every child having settled.
    if let (Some(parent), true) = (children, named.is_empty()) {
        complain!("amx wait: {parent} has no children");
        return Ok(exit::FAILURE);
    }
    let mut pending = named
        .into_iter()
        .map(|id| Ok((Turns::of(root, &id)?, id)))
        .collect::<Result<Vec<_>>>()?;

    let deadline = timeout.map(|patience| Instant::now() + patience);
    loop {
        // One sleep per sweep, at the slowest pace any pending agent needs, so
        // an agent whose reading takes a screen capture sets the rate.
        let mut slowest = POLL;
        let ids: Vec<&str> = pending.iter().map(|(_, id)| id.as_str()).collect();
        let views = readings(root, &ids)?;
        let mut still = Vec::with_capacity(pending.len());
        for ((mut turns, id), view) in pending.into_iter().zip(views) {
            if !ready(&mut turns, view.phase(), state) {
                slowest = slowest.max(pace(&view.verdict.evidence));
                still.push((turns, id));
                continue;
            }
            // Flushed per line: callers read these as they arrive.
            writeln!(out, "{id} {}", view.phase())?;
            out.flush()?;
            if any {
                return Ok(exit::OK);
            }
        }
        pending = still;

        if pending.is_empty() {
            return Ok(exit::OK);
        }
        // Checked after the sweep, so an agent that settled at the deadline is
        // still reported.
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            return Ok(exit::TIMEOUT);
        }
        std::thread::sleep(slowest);
    }
}

/// Read these agents in one pass, returned in the order given.
///
/// Uses [`derive::views_of`], so tmux is asked once per server rather than once
/// per agent.
fn readings(root: &Path, ids: &[&str]) -> Result<Vec<View>> {
    let mut records = Vec::with_capacity(ids.len());
    for id in ids {
        let agent = Agent::open(root, id)?;
        let meta = agent.meta()?;
        let state = agent.state()?;
        records.push(Record { agent, meta, state });
    }
    let mut views = derive::views_of(root, records, store::now());
    ids.iter()
        .map(|id| {
            let at = views
                .iter()
                .position(|view| view.id() == *id)
                .with_context(|| format!("the record of {id} names another agent"))?;
            Ok(views.swap_remove(at))
        })
        .collect()
}

/// The ids in the order named, without duplicates.
fn taken(ids: &[String]) -> Vec<String> {
    let mut taken: Vec<String> = Vec::with_capacity(ids.len());
    for id in ids {
        if !taken.iter().any(|already| already == id) {
            taken.push(id.clone());
        }
    }
    taken
}

/// The ids of every agent whose record names `parent`, oldest first.
///
/// Errors if `parent` names no agent, so a typo is not read as an empty
/// family. Records that cannot be read are skipped.
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

/// Whether this reading is what the wait is for. The log is read only when the
/// phase needs it.
fn ready(turns: &mut Turns, phase: Phase, wanted: Option<Phase>) -> bool {
    let ended = match wanted {
        Some(_) => Ended::NotYet,
        None => turns.ended(phase),
    };
    settled(phase, wanted, ended)
}

/// Whether this reading is what the wait is for.
///
/// With no `--for`, anything `result` would stop on: a finished turn or a
/// question. `Unknown` does not count, and neither does an idle agent with an
/// unanswered message still queued.
fn settled(phase: Phase, wanted: Option<Phase>, ended: Ended) -> bool {
    match wanted {
        Some(wanted) => phase == wanted,
        None => result::settled(phase, ended) != Settled::NotYet,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::derive::Evidence;
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
        // Still running, or unknown: no answer to take yet.
        for phase in [Phase::Starting, Phase::Working, Phase::Unknown] {
            assert!(!settled(phase, None, Ended::Turn), "{phase}");
        }
    }

    #[test]
    fn a_message_nothing_has_answered_keeps_an_idle_agent_from_settling() {
        // Idle after a turn cut short by hand, with the sent message still
        // queued: the recorded answer belongs to the previous turn.
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

        // A resume ends that turn, and the wait with it.
        said("resume");
        let (code, printed) = waited(root.path(), &["a"], false, None, patience);
        assert_eq!(code, exit::OK);
        assert_eq!(printed, "a idle\n");
    }

    #[test]
    fn for_a_named_phase_waits_for_that_one_and_no_other() {
        // With `--for`, only the named phase ends the wait.
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
            assert_eq!(pace(&evidence), result::LOOK, "{evidence:?}");
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

        // Printed in the order named, whatever order the records are read in.
        let (code, said) = waited(root.path(), &["b", "a"], false, None, None);
        assert_eq!(code, exit::OK);
        assert_eq!(said, "b idle\na done\n");
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
        // A timeout still prints the agents that settled before it.
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

        // Without `--for`, a working agent never settles.
        let patience = Some(Duration::ZERO);
        let (code, said) = waited(root.path(), &["a"], false, None, patience);
        assert_eq!(code, exit::TIMEOUT);
        assert_eq!(said, "");
    }

    #[test]
    fn an_id_that_names_no_agent_is_refused_before_anything_is_waited_on() {
        // Refused before anything is printed, naming the unknown id.
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
        // An empty family would otherwise end at once with nothing printed.
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

    /// Run one wait; return its exit code and what it printed.
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

    /// A parked agent in `phase`, so a reading with no pane returns that phase.
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
