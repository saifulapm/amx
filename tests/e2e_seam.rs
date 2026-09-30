//! amx driven by a program that runs a fleet of workers: it prepares a
//! directory per worker, then calls `new`, `ls --json`, `result` and `stop`
//! with the flags such a program uses. The caller reads only exit codes and
//! JSON, never tmux, a human-readable table or a pane's screen.

mod common;

use common::{Harness, code, stderr, stdout};
use serde_json::Value;
use std::path::Path;
use std::process::Output;

/// The session id the caller picks. It names the transcript the caller reads
/// later, so amx must pass it through.
const SESSION: &str = "018f2c7e-0000-4000-8000-000000000000";

/// The path of the brief the worker is told to read.
const BRIEF: &str = "/cache/briefs/t1.md";

/// A program driving amx, with one method per amx call it makes.
struct Caller<'a> {
    amx: &'a Harness,
}

impl<'a> Caller<'a> {
    fn new(amx: &'a Harness) -> Caller<'a> {
        Caller { amx }
    }

    /// Run `amx new` in `dir`, which the caller has already prepared, passing
    /// the caller's session id and model to the vendor.
    fn dispatch(&self, dir: &Path, scenario: &str) -> Output {
        self.amx
            .amx_command(&[
                "new",
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

    /// Every worker, from one `ls --json` call. Both liveness checks read
    /// this, so polling a fleet costs one call.
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

    /// Whether the worker is in any state other than done, failed or stopped.
    fn alive(&self, id: &str) -> bool {
        !matches!(
            self.row(id)["state"].as_str(),
            Some("done" | "failed" | "stopped")
        )
    }

    /// The later of `last_event` and `since`, in epoch seconds, which a caller
    /// measures its stall deadline from. A worker with no events yet still
    /// has its dispatch time.
    fn last_activity(&self, id: &str) -> u64 {
        let row = self.row(id);
        let field = |name: &str| row[name].as_u64().unwrap_or(0);
        field("last_event").max(field("since"))
    }

    /// Run `amx result`, which waits for the turn to end. The timeout makes a
    /// wait that never returns fail its own test instead of hanging the suite.
    fn result(&self, id: &str) -> Output {
        self.amx.amx(&["result", id, "--timeout", "20"])
    }

    /// Stop the worker with `--force`, taking every default, since nobody is
    /// at a terminal to answer prompts.
    fn stop(&self, id: &str) -> Output {
        self.amx.amx(&["stop", id, "--force"])
    }
}

#[test]
fn a_worker_is_dispatched_watched_answered_and_stopped() {
    let amx = Harness::new();
    let caller = Caller::new(&amx);
    let dir = amx.a_repo();

    let dispatched = caller.dispatch(&dir, "a-dispatched-worker");
    assert_eq!(code(&dispatched), 0, "{}", stderr(&dispatched));
    let id = handle(&dispatched);

    // The worker must read as alive as soon as `new` returns.
    assert!(caller.alive(&id), "a worker is alive as soon as it exists");
    assert!(
        caller.last_activity(&id) > 0,
        "a worker whose last sign of life is the epoch is one every deadline \
         has already passed"
    );

    // `result` prints the answer verbatim, blank lines included.
    let answered = caller.result(&id);
    assert_eq!(code(&answered), 0, "{}", stderr(&answered));
    assert_eq!(
        stdout(&answered),
        "the importer is ported\n\n- four endpoints moved\n- the tests pass\n"
    );

    // The worker outlives its turn until the caller stops it.
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
    // The caller merges from this directory, so a worktree cut inside it
    // would hold work the caller never looks at.
    let amx = Harness::new();
    let caller = Caller::new(&amx);
    let dir = amx.a_repo();

    let id = handle(&caller.dispatch(&dir, "a-dispatched-worker"));
    let row = caller.row(&id);

    assert_eq!(row["dir"], dir.to_string_lossy().as_ref());
    assert!(
        row["worktree"].is_null() && row["branch"].is_null(),
        "the caller owns the branch: {row}"
    );
    // `base` is still recorded, so `amx diff` has a commit to diff against.
    let head = std::process::Command::new("git")
        .current_dir(&dir)
        .args(["rev-parse", "HEAD"])
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .expect("running git");
    let head = String::from_utf8_lossy(&head.stdout).trim_end().to_string();
    assert_eq!(
        row["base"].as_str(),
        Some(head.as_str()),
        "the commit the caller's directory is on: {row}"
    );
    assert!(!dir.join(".amx").exists(), "and nothing was cut inside it");

    // The vendor process itself runs there too.
    let pane = amx.pane_of(&id);
    let ran_in = amx.tmux(&["display-message", "-p", "-t", &pane, "#{pane_current_path}"]);
    assert_eq!(
        std::fs::canonicalize(ran_in).unwrap(),
        std::fs::canonicalize(&dir).unwrap()
    );
}

#[test]
fn the_arguments_the_caller_passes_reach_the_vendor_untouched() {
    // amx appends the brief as the prompt and changes nothing else.
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

    let answered = caller.result(&first);
    assert_eq!(code(&answered), 0, "{}", stderr(&answered));
    assert!(stdout(&answered).starts_with("the importer is ported"));

    let answered = caller.result(&second);
    assert_eq!(code(&answered), 0, "{}", stderr(&answered));
    assert_eq!(stdout(&answered).trim(), "hello");

    // One `ls` lists both, each row with the fields a caller branches on.
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
    // Nobody can answer a permission prompt here, so `result` must return at
    // the prompt, with the prompt, instead of running out the timeout.
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
    // After a stop mid-turn, `ls` and `result` must both see the worker as
    // ended, or the caller waits on a worker that no longer exists.
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

#[test]
fn status_json_carries_the_conversations_context_and_last_words() {
    let amx = Harness::new();
    let caller = Caller::new(&amx);
    let dir = amx.a_repo();

    let id = handle(&caller.dispatch(&dir, "carries-usage"));

    // `new` returns before the vendor starts, so no transcript exists yet.
    let before = amx.amx(&["status", &id, "--json"]);
    assert_eq!(code(&before), 0, "{}", stderr(&before));
    let before: Value = serde_json::from_slice(&before.stdout).expect("status --json prints json");
    assert_eq!(before["context"], Value::Null, "{before}");
    assert_eq!(before["last_words"], Value::Null, "{before}");

    let answered = caller.result(&id);
    assert_eq!(code(&answered), 0, "{}", stderr(&answered));

    let after = amx.amx(&["status", &id, "--json"]);
    assert_eq!(code(&after), 0, "{}", stderr(&after));
    let after: Value = serde_json::from_slice(&after.stdout).expect("status --json prints json");
    assert_eq!(
        after["context"], 200,
        "input, cache creation and cache read tokens, summed: {after}"
    );
    assert_eq!(after["last_words"], "hello", "{after}");
}

/// The agent id `amx new` printed, which must be the only line on stdout.
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
