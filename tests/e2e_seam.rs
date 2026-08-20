//! The seam a program drives amx through.
//!
//! amx has two callers. One is a person at a terminal, and the rest of this
//! suite is about them. The other is a program running a fleet of agents: it
//! cuts its own directory for each one, hands it a brief, and then has exactly
//! four questions — start this, is it still going, what did it say, stop it.
//! This file asks those four in the order and with the flags such a program
//! uses, and reads nothing but exit codes and JSON.
//!
//! What is under test is the contract rather than the wiring. A caller here
//! never learns that tmux is underneath any of this, never parses a table
//! written for a person, and never sees an agent's screen.

mod common;

use common::Harness;
use serde_json::Value;
use std::path::Path;
use std::process::Output;

/// The conversation id the caller mints for itself. It names the transcript
/// the caller reads afterwards, so it is the caller's to choose and amx's to
/// pass along.
const SESSION: &str = "018f2c7e-0000-4000-8000-000000000000";

/// The brief the worker is pointed at, the way a caller writes one: a path,
/// with the instruction to read it.
const BRIEF: &str = "/cache/briefs/t1.md";

/// The caller: one method per question it has, and the single amx call that
/// answers it.
struct Caller<'a> {
    amx: &'a Harness,
}

impl<'a> Caller<'a> {
    fn new(amx: &'a Harness) -> Caller<'a> {
        Caller { amx }
    }

    /// Start a worker in a directory the caller has already prepared, out of
    /// sight of anybody's terminal, on the conversation id the caller minted.
    /// The handle is the id this prints; nothing else about it is read.
    fn dispatch(&self, dir: &Path, scenario: &str) -> Output {
        self.amx
            .amx_command(&[
                "new",
                "--bg",
                "--dir",
                &dir.to_string_lossy(),
                "--no-worktree",
                "--agent",
                &self.amx.mock(),
                &format!("Read {BRIEF} and execute it exactly."),
                "--",
                "--session-id",
                SESSION,
                "--model",
                "sonnet",
            ])
            .env("MOCK_CLAUDE_SCENARIO", self.amx.scenario(scenario))
            .output()
            .expect("dispatching a worker")
    }

    /// One reading of every worker there is. Both of the caller's questions
    /// about liveness are answered from this, so a fleet costs one call and
    /// not one per worker.
    fn ls(&self) -> Vec<Value> {
        let out = self.amx.amx(&["ls", "--json"]);
        assert_eq!(code(&out), 0, "{}", stderr(&out));
        serde_json::from_slice(&out.stdout).expect("ls --json prints json")
    }

    fn row(&self, id: &str) -> Value {
        self.ls()
            .into_iter()
            .find(|row| row["id"] == id)
            .unwrap_or_else(|| panic!("no row for {id}"))
    }

    /// Is anything of this worker still running? Three states mean it is over;
    /// every other state is a worker the caller keeps waiting on.
    fn alive(&self, id: &str) -> bool {
        !matches!(
            self.row(id)["state"].as_str(),
            Some("done" | "failed" | "stopped")
        )
    }

    /// The most recent sign of life, in epoch seconds — what a caller counts
    /// its stall deadline from. A worker nothing has been heard from yet still
    /// has the moment it was dispatched.
    fn last_activity(&self, id: &str) -> u64 {
        let row = self.row(id);
        let field = |name: &str| row[name].as_u64().unwrap_or(0);
        field("last_event").max(field("since"))
    }

    /// Wait for the turn to end, and take whatever the worker said. Every wait
    /// here is bounded, so a verb that never returns fails its own test rather
    /// than the suite.
    fn result(&self, id: &str) -> Output {
        self.amx.amx(&["result", id, "--timeout", "20"])
    }

    /// Stop the worker and everything it started, taking the defaults: there
    /// is nobody at a terminal to be asked.
    fn stop(&self, id: &str) -> Output {
        self.amx.amx(&["stop", id, "--force"])
    }
}

#[test]
fn a_worker_is_dispatched_watched_answered_and_stopped() {
    let amx = Harness::new();
    let caller = Caller::new(&amx);
    // The caller cut this itself, and will read the work out of it itself.
    let dir = amx.a_repo();

    let dispatched = caller.dispatch(&dir, "a-dispatched-worker");
    assert_eq!(code(&dispatched), 0, "{}", stderr(&dispatched));
    let id = handle(&dispatched);

    // Liveness, from the moment the dispatch returns. There is no window in
    // which a worker that has just started reads as one that has gone.
    assert!(caller.alive(&id), "a worker is alive as soon as it exists");
    assert!(
        caller.last_activity(&id) > 0,
        "a worker whose last sign of life is the epoch is one every deadline \
         has already passed"
    );

    // The answer: waited for rather than polled, and handed over as the worker
    // said it, line breaks and all.
    let answered = caller.result(&id);
    assert_eq!(code(&answered), 0, "{}", stderr(&answered));
    assert_eq!(
        stdout(&answered),
        "the importer is ported\n\n- four endpoints moved\n- the tests pass\n"
    );

    // Answering is not ending. The worker is still there — to look in on, to
    // send more work to — until the caller says otherwise.
    assert!(caller.alive(&id), "the turn ended, not the worker");
    assert!(
        caller.last_activity(&id) >= caller.row(&id)["created"].as_u64().unwrap(),
        "and the turn it just finished is the last thing heard from it"
    );

    let stopped = caller.stop(&id);
    assert_eq!(code(&stopped), 0, "{}", stderr(&stopped));
    assert!(!caller.alive(&id));
    assert_eq!(caller.row(&id)["state"], "stopped");
}

#[test]
fn a_worker_runs_where_it_was_put_and_cuts_nothing_of_its_own() {
    // The caller made this directory, wrote the brief into it, and will merge
    // what comes out of it. A worker that cut a second one underneath would do
    // its work somewhere nobody was ever going to look.
    let amx = Harness::new();
    let caller = Caller::new(&amx);
    let dir = amx.a_repo();

    let id = handle(&caller.dispatch(&dir, "a-dispatched-worker"));
    let row = caller.row(&id);

    assert_eq!(row["dir"], dir.to_string_lossy().as_ref());
    assert!(
        row["worktree"].is_null() && row["branch"].is_null() && row["base"].is_null(),
        "the caller owns the branch and the commit it came from: {row}"
    );
    assert!(!dir.join(".amx").exists(), "and nothing was cut inside it");

    // Not only on the record: the vendor's own process is in that directory.
    let pane = amx.pane_of(&id);
    let ran_in = amx.tmux(&["display-message", "-p", "-t", &pane, "#{pane_current_path}"]);
    assert_eq!(
        std::fs::canonicalize(ran_in).unwrap(),
        std::fs::canonicalize(&dir).unwrap()
    );
}

#[test]
fn the_arguments_the_caller_passes_reach_the_vendor_untouched() {
    // How the worker is configured — which conversation it opens, which model
    // it runs — is the caller's business and travels in the caller's own
    // words. amx adds the brief at the end, where a prompt goes, and changes
    // nothing else.
    let amx = Harness::new();
    let caller = Caller::new(&amx);
    let dir = amx.a_repo();

    let id = handle(&caller.dispatch(&dir, "a-dispatched-worker"));
    let pane = amx.pane_of(&id);
    let argv = amx.until("the worker to say how it was called", || {
        amx.capture(&pane)
            .lines()
            .find(|line| line.starts_with("argv:"))
            .map(str::to_string)
    });

    assert!(argv.contains(&format!("--session-id {SESSION}")), "{argv}");
    assert!(argv.contains("--model sonnet"), "{argv}");
    assert!(
        argv.trim_end()
            .ends_with(&format!("Read {BRIEF} and execute it exactly.")),
        "the brief is the last word, the way a prompt is: {argv}"
    );
}

#[test]
fn liveness_and_every_answer_about_a_fleet_come_from_one_reading() {
    let amx = Harness::new();
    let caller = Caller::new(&amx);
    let dir = amx.a_repo();

    let first = handle(&caller.dispatch(&dir, "a-dispatched-worker"));
    let second = handle(&caller.dispatch(&dir, "finishes"));

    // Each answer belongs to the worker whose handle asked for it.
    let answered = caller.result(&first);
    assert_eq!(code(&answered), 0, "{}", stderr(&answered));
    assert!(stdout(&answered).starts_with("the importer is ported"));

    let answered = caller.result(&second);
    assert_eq!(code(&answered), 0, "{}", stderr(&answered));
    assert_eq!(stdout(&answered).trim(), "hello");

    // And one reading accounts for both of them, with the fields a caller
    // branches on carried on every row.
    let listed = caller.ls();
    assert_eq!(listed.len(), 2, "{listed:#?}");
    for row in &listed {
        for field in ["id", "state", "since", "last_event", "task", "dir"] {
            assert!(!row[field].is_null(), "row without {field}: {row}");
        }
    }
}

#[test]
fn a_worker_that_ends_badly_is_an_ending_the_caller_can_read() {
    let amx = Harness::new();
    let caller = Caller::new(&amx);
    let dir = amx.a_repo();

    let id = handle(&caller.dispatch(&dir, "fails"));

    let answered = caller.result(&id);
    assert_eq!(code(&answered), 1, "{}", stderr(&answered));
    assert_eq!(stdout(&answered), "", "nothing on stdout is an answer");

    assert!(!caller.alive(&id));
    let row = caller.row(&id);
    assert_eq!(row["state"], "failed");
    assert_eq!(row["exit"], 2, "and the code its command ended on: {row}");
}

#[test]
fn a_worker_that_stops_to_ask_is_handed_back_rather_than_waited_out() {
    // There is nobody here to answer a permission question. What the caller
    // must not do is spend its deadline finding that out — the wait ends at
    // the question, with the question, so the worker can be set aside and the
    // rest of the fleet got on with.
    let amx = Harness::new();
    let caller = Caller::new(&amx);
    let dir = amx.a_repo();

    let id = handle(&caller.dispatch(&dir, "asks-a-question"));

    let answered = caller.result(&id);
    assert_eq!(code(&answered), 2, "{}", stderr(&answered));
    assert!(
        stdout(&answered).contains("Claude needs your permission"),
        "{:?}",
        stdout(&answered)
    );

    assert!(caller.alive(&id), "and it is still there to be answered");
    let row = caller.row(&id);
    assert_eq!(row["state"], "waiting");
    assert!(
        row["question"]
            .as_str()
            .is_some_and(|asked| asked.contains("permission")),
        "the reading carries the question too, so a caller that lists before \
         it waits already knows: {row}"
    );
}

#[test]
fn stopping_a_worker_mid_turn_ends_it_and_the_next_question_says_so() {
    // The caller's deadline passed, or the wave was abandoned. Whatever the
    // worker was in the middle of, the ending has to be one the next reading
    // agrees with, or the caller waits on a worker that is not there.
    let amx = Harness::new();
    let caller = Caller::new(&amx);
    let dir = amx.a_repo();

    let id = handle(&caller.dispatch(&dir, "works-without-end"));
    amx.until_state(&id, "working");

    let stopped = caller.stop(&id);
    assert_eq!(code(&stopped), 0, "{}", stderr(&stopped));
    assert!(!caller.alive(&id));

    let answered = caller.result(&id);
    assert_eq!(
        code(&answered),
        1,
        "a stopped worker answers at once, rather than holding the caller to \
         its timeout: {}",
        stderr(&answered)
    );
    assert_eq!(stdout(&answered), "");
}

/// The handle a dispatch hands back: the id, and nothing else on stdout for a
/// caller to have to strip.
fn handle(out: &Output) -> String {
    assert!(out.status.success(), "amx new: {}", stderr(out));
    let printed = stdout(out);
    assert_eq!(
        printed.lines().count(),
        1,
        "a dispatch prints the handle alone: {printed:?}"
    );
    printed.trim().to_string()
}

/// The code the caller branches on.
fn code(out: &Output) -> i32 {
    out.status.code().expect("amx exited with a code")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}
