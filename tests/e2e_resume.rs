//! Tests for bringing an agent back after its pane is gone: `amx resume`,
//! `amx attach` and Enter in the view start a new pane that continues the
//! agent's recorded vendor session. Also covers the refusals for agents with
//! nothing to continue, and how `amx adopt` reads a pane it takes over.

mod common;

use common::{Harness, a_project, argv_of, clients_on, something_else_on_the_server, watching};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::process::Output;

/// The session id mock-claude announces when it continues a session. Tests
/// wait for it in the agent's meta to know a resume took effect.
const CONTINUED: &str = "b7d2a5c8-3e14-4f9a-8c26-0d5b1a7e3f42";

/// Start mock-claude as agent `id` in `dir` with `amx new`, playing
/// `scenario`.
fn start(amx: &Harness, id: &str, dir: &Path, scenario: &str) {
    start_with(amx, id, dir, scenario, &[]);
}

/// [`start`], with `vendor` appended to the `amx new` arguments.
fn start_with(amx: &Harness, id: &str, dir: &Path, scenario: &str, vendor: &[&str]) {
    let out = amx
        .amx_command(
            &[
                &[
                    "new",
                    "--name",
                    id,
                    "--dir",
                    &dir.to_string_lossy(),
                    "--agent",
                    &amx.mock(),
                    "fix the login bug",
                ][..],
                vendor,
            ]
            .concat(),
        )
        .env("MOCK_CLAUDE_SCENARIO", amx.scenario(scenario))
        .output()
        .expect("running amx new");
    assert!(
        out.status.success(),
        "amx new: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Run `amx resume` with mock-claude set up to continue a session.
fn resume(amx: &Harness, args: &[&str]) -> Output {
    amx.amx_command(&[&["resume"], args].concat())
        .env("MOCK_CLAUDE_SCENARIO", amx.scenario("continues-a-session"))
        .env("MOCK_CLAUDE_SESSION_2", CONTINUED)
        .output()
        .expect("running amx resume")
}

fn said(out: &Output) -> String {
    assert!(
        out.status.success(),
        "amx resume: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Wait until the agent's meta records [`CONTINUED`] as its session.
fn until_continued(amx: &Harness, id: &str) {
    amx.until(&format!("{id} to be on its continued session"), || {
        (amx.meta(id)["session"] == CONTINUED).then_some(())
    });
}

/// Run amx with `args` in a new terminal outside tmux, with mock-claude set up
/// to continue a session, and answer with the terminal's pane.
///
/// Clearing `TMUX` and `TMUX_PANE` puts it outside tmux, where attaching takes
/// over the terminal itself, so its screen shows the result.
fn a_terminal(amx: &Harness, args: &[&str]) -> String {
    let scenario = amx.scenario("continues-a-session");
    amx.in_a_terminal(
        &[
            ("TMUX", ""),
            ("TMUX_PANE", ""),
            ("MOCK_CLAUDE_SCENARIO", &scenario.to_string_lossy()),
            ("MOCK_CLAUDE_SESSION_2", CONTINUED),
        ],
        args,
    )
}

/// [`a_terminal`] with `TMUX` and `TMUX_PANE` left set, so amx sees a terminal
/// inside tmux.
///
/// Inside tmux the view switches a client instead of taking over the terminal.
fn a_terminal_inside_tmux(amx: &Harness, args: &[&str]) -> String {
    let scenario = amx.scenario("continues-a-session");
    amx.in_a_terminal(
        &[
            ("MOCK_CLAUDE_SCENARIO", &scenario.to_string_lossy()),
            ("MOCK_CLAUDE_SESSION_2", CONTINUED),
        ],
        args,
    )
}

/// Wait until a client is attached to the agent's session.
///
/// Attach writes the `--last` trail just before it hands over to tmux, so a
/// client on the session means the trail is written.
fn until_attached(amx: &Harness, id: &str) {
    amx.until(&format!("a terminal on {id}"), || {
        (!clients_on(amx, &format!("amx-{id}")).is_empty()).then_some(())
    });
}

/// Wait until `terminal`'s own client is attached to `id`'s session.
///
/// Matches on the terminal's tty, since the session may already have other
/// clients.
fn until_the_terminal_is_on(amx: &Harness, terminal: &str, id: &str) {
    let tty = amx.tmux(&["display-message", "-p", "-t", terminal, "#{pane_tty}"]);
    amx.until(&format!("this terminal on {id}"), || {
        clients_on(amx, &format!("amx-{id}"))
            .lines()
            .any(|on| on == tty)
            .then_some(())
    });
}

/// Wait until the continued session is drawn on `terminal`.
fn until_looking_at_it(amx: &Harness, terminal: &str) {
    amx.until("the agent on the screen", || {
        amx.capture(terminal)
            .contains("continuing where we left off")
            .then_some(())
    });
}

/// Start an agent, let it go idle and stop it, and answer with the pane it
/// had.
fn ran_and_stopped(amx: &Harness, id: &str) -> String {
    something_else_on_the_server(amx);
    start(amx, id, amx.home(), "happy-turn");
    amx.until_state(id, "idle");
    amx.amx(&["stop", id, "--force"]);
    assert_eq!(amx.state(id)["state"], "stopped");
    let gone = amx.pane_of(id);
    assert!(!amx.pane_alive(&gone), "stopping took the pane with it");
    gone
}

/// Leave agent `id` idle with its record naming another agent's live pane, and
/// answer with that pane.
///
/// This is the state after a reboot: the pane died with the server and no exit
/// was recorded, so the record still says idle, and tmux, which numbers panes
/// from `%0` per server, has given the same pane id to another agent.
fn taken_over(amx: &Harness, id: &str) -> String {
    something_else_on_the_server(amx);
    start(amx, id, amx.home(), "happy-turn");
    amx.until_state(id, "idle");
    kill_pane(amx, &amx.pane_of(id));
    assert_eq!(
        amx.state(id)["state"],
        "idle",
        "a pane that goes without amx being told leaves the record where it was"
    );

    // amx tells whose pane it is by the `amx-<id>` session it sits in.
    let theirs = amx.tmux(&[
        "new-session",
        "-d",
        "-s",
        "amx-port-importer-c3d",
        "-P",
        "-F",
        "#{pane_id}",
        "--",
        "sh",
        "-c",
        "echo the other agent; while :; do sleep 0.05; done",
    ]);
    amx.set_meta(id, json!({ "pane": theirs }));
    theirs
}

/// Adopt a claude started by hand as agent `id`, and answer with its pane.
///
/// An adopted agent has a session but no recorded command, so amx cannot
/// start it again.
fn adopted(amx: &Harness, id: &str) -> String {
    something_else_on_the_server(amx);
    let pane = amx.tmux(&[
        "new-session",
        "-d",
        "-P",
        "-F",
        "#{pane_id}",
        "--",
        "sh",
        "-c",
        "while :; do sleep 0.05; done",
    ]);
    let out = amx
        .amx_command(&["adopt", "--name", id, "--task", "fix the login bug"])
        // adopt reads the pane from tmux and the session from the vendor.
        .env("TMUX_PANE", &pane)
        .env("CLAUDE_CODE_SESSION_ID", ADOPTED)
        .output()
        .expect("running amx adopt");
    assert!(
        out.status.success(),
        "amx adopt: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    pane
}

/// The session id the adopted agent reports.
const ADOPTED: &str = "9f3c1d20-5a44-4e7b-8c19-6d0a2b5f7e31";

/// Start mock pi on its question dialog in a pane amx did not open, and answer
/// with the pane once the dialog is drawn.
///
/// The fixture runs by path, as a pi started by hand would, so it needs no
/// PATH setup the way `amx new --agent pi` does.
fn a_pi_on_its_dialog(amx: &Harness) -> String {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/mock_pi");
    let scenario = format!(
        "MOCK_PI_SCENARIO={}",
        fixtures
            .join("scenarios/asks-a-question.scenario")
            .display()
    );
    let pi = fixtures.join("pi").to_string_lossy().into_owned();
    let pane = amx.tmux(&[
        "new-session",
        "-d",
        "-P",
        "-F",
        "#{pane_id}",
        "--",
        "env",
        &scenario,
        &pi,
    ]);

    // Wait for the hint row pi draws under every dialog, which its screen
    // document anchors on. Adoption reads the pane only once.
    amx.until("pi's dialog on the pane", || {
        amx.capture(&pane).contains("↑↓ navigate").then_some(())
    });
    pane
}

/// Run `amx adopt` for `pane` with the vendor session variable `session` set.
///
/// `CLAUDE_CODE_SESSION_ID` is removed first: the suite often runs inside a
/// claude, and claude is first in the vendor table, so a leaked copy would win
/// over the vendor under test.
fn adopt_as(amx: &Harness, id: &str, pane: &str, session: (&str, &str)) {
    let out = amx
        .amx_command(&["adopt", "--name", id, "--task", "fix the login bug"])
        .env_remove("CLAUDE_CODE_SESSION_ID")
        .env("TMUX_PANE", pane)
        .env(session.0, session.1)
        .output()
        .expect("running amx adopt");
    assert!(
        out.status.success(),
        "amx adopt: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn until_pane_gone(amx: &Harness, pane: &str) {
    amx.until("the pane to go", || (!amx.pane_alive(pane)).then_some(()));
}

/// Kill `pane` and wait until it is gone.
fn kill_pane(amx: &Harness, pane: &str) {
    amx.tmux(&["kill-pane", "-t", pane]);
    until_pane_gone(amx, pane);
}

#[test]
fn resume_brings_a_stopped_agent_back_on_the_session_it_had() {
    let amx = Harness::new();
    let id = "fix-login-a1b";
    start(&amx, id, amx.home(), "happy-turn");
    amx.until_state(id, "idle");
    let session = amx.meta(id)["session"]
        .as_str()
        .expect("a session was recorded")
        .to_string();

    amx.amx(&["stop", id, "--force"]);
    assert_eq!(amx.state(id)["state"], "stopped");

    let out = resume(&amx, &[id]);
    assert!(said(&out).contains(id), "it says what came back");

    // The original task is already in the session, so it is not passed again.
    let pane = amx.pane_of(id);
    let called = amx.until("the vendor to say how it was called", || {
        let screen = amx.capture(&pane);
        screen.contains("argv:").then_some(screen)
    });
    assert!(called.contains(&format!("--resume={session}")), "{called}");
    assert!(!called.contains("fix the login bug"), "{called}");

    until_continued(&amx, id);
    assert!(amx.pane_alive(&pane));
    assert_ne!(amx.state(id)["state"], "stopped");
    assert_eq!(amx.state(id)["exit"], json!(null), "it is running again");
}

#[test]
fn resume_with_no_message_is_idle_the_moment_the_session_opens() {
    // With no message no turn follows, so only the session opening can move
    // the record off `starting`. The pane is no fallback: a custom footer can
    // hide the prompt screen, which left pi agents stuck in `starting`.
    let amx = Harness::new();
    let id = "fix-login-a1b";
    start(&amx, id, amx.home(), "happy-turn");
    amx.until_state(id, "idle");
    amx.amx(&["stop", id, "--force"]);

    resume(&amx, &[id]);
    until_continued(&amx, id);
    assert_eq!(
        amx.state(id)["state"],
        "idle",
        "at its prompt, with no reading of the pane in it"
    );

    // With a message, the message is the first turn and the record stays
    // `starting` until the vendor reports on it. The mock announces the session
    // and never starts the turn.
    amx.amx(&["stop", id, "--force"]);
    resume(&amx, &[id, "and now the linter"]);
    until_continued(&amx, id);
    assert_eq!(
        amx.state(id)["state"],
        "starting",
        "a turn was asked for, and the session opening is not the end of it"
    );
}

#[test]
fn resume_keeps_what_the_agent_worked_and_what_it_was_called() {
    // A resume continues the same agent, so the state's `worked` time and a
    // name set on the wall carry over instead of being reset with the answer
    // and exit code.
    let amx = Harness::new();
    let id = "fix-login-a1b";
    start(&amx, id, amx.home(), "happy-turn");
    amx.until_state(id, "idle");
    amx.amx(&["stop", id, "--force"]);

    let path = amx.agent_dir(id).join("state.json");
    let mut state = amx.state(id);
    state["worked"] = json!(22_178);
    state["name"] = json!("billing");
    std::fs::write(&path, state.to_string()).unwrap();

    resume(&amx, &[id]);
    until_continued(&amx, id);
    assert_eq!(amx.state(id)["worked"], 22_178);
    assert_eq!(amx.state(id)["name"], "billing");
}

#[test]
fn resume_brings_back_an_agent_whose_pane_answers_for_somebody_else() {
    // A pane now held by another agent counts as gone. Treating the agent as
    // still running there would refuse the resume.
    let amx = Harness::new();
    let id = "fix-login-a1b";
    let theirs = taken_over(&amx, id);

    said(&resume(&amx, &[id]));
    until_continued(&amx, id);
    assert_ne!(amx.pane_of(id), theirs, "a pane of its own again");
    assert!(
        amx.pane_alive(&theirs),
        "and the other agent is left where it was"
    );
}

#[test]
fn logs_of_an_agent_whose_pane_answers_for_somebody_else_read_the_record() {
    // The pane the record names shows another agent, so logs must not read it
    // and falls back to the answer on the record.
    let amx = Harness::new();
    let id = "fix-login-a1b";
    taken_over(&amx, id);
    // With a transcript, logs reads that file whatever the pane, so drop it to
    // exercise the pane path.
    amx.set_meta(id, json!({ "transcript": null }));

    let out = amx.amx(&["logs", id]);
    assert!(
        out.status.success(),
        "amx logs: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "the tests pass now\n",
        "the answer this agent left, not what the pane is showing now"
    );
}

#[test]
fn resume_refuses_an_agent_that_has_not_ended() {
    let amx = Harness::new();
    let id = "watch-log-c3d";
    amx.play(id, "works-without-end");
    amx.until_state(id, "working");
    let pane = amx.pane_of(id);

    let out = resume(&amx, &[id]);
    assert_eq!(
        out.status.code(),
        Some(2),
        "an agent that is still going is not something to start again"
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains(id));
    assert_eq!(amx.pane_of(id), pane, "and it was left where it was");
    assert_eq!(amx.state(id)["state"], "working");
}

#[test]
fn resume_two_racers_bring_back_one_agent_and_not_two() {
    // Two panes resuming the same session would share one record, so of two
    // concurrent resumes exactly one may win.
    let amx = Harness::new();
    let id = "fix-login-a1b";
    start(&amx, id, amx.home(), "happy-turn");
    amx.until_state(id, "idle");
    amx.amx(&["stop", id, "--force"]);
    assert_eq!(amx.state(id)["state"], "stopped");

    let state_dir = amx
        .state_root()
        .parent()
        .expect("the state root has a parent")
        .to_path_buf();
    // Both racers spin on the `go` file, so they reach the has-it-ended check
    // together instead of one process start apart.
    let go = state_dir.join("go");
    let racers: Vec<_> = (0..2)
        .map(|_| {
            std::process::Command::new("sh")
                .arg("-c")
                .arg(format!(
                    "until [ -e '{go}' ]; do :; done; exec '{amx}' resume '{id}'",
                    go = go.display(),
                    amx = common::AMX,
                ))
                .env("AMX_STATE_DIR", &state_dir)
                .env("HOME", amx.home())
                .env("XDG_CONFIG_HOME", amx.home().join(".config"))
                .env("AMX_TMUX_SOCKET", amx.socket())
                .env("MOCK_CLAUDE_SCENARIO", amx.scenario("continues-a-session"))
                .env("MOCK_CLAUDE_SESSION_2", CONTINUED)
                .env_remove("TMUX")
                .env_remove("TMUX_PANE")
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .expect("starting a racer")
        })
        .collect();
    std::fs::write(&go, b"").expect("the starting gun");
    let done: Vec<Output> = racers
        .into_iter()
        .map(|racer| racer.wait_with_output().expect("waiting for a racer"))
        .collect();

    let winners = done.iter().filter(|out| out.status.success()).count();
    assert_eq!(
        winners,
        1,
        "one resume owns the comeback: {}",
        done.iter()
            .map(|out| format!(
                "[exit {:?} out {:?} err {:?}]",
                out.status.code(),
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            ))
            .collect::<Vec<_>>()
            .join(" ")
    );
    until_continued(&amx, id);
    assert!(
        amx.pane_alive(&amx.pane_of(id)),
        "and the record names the pane that is actually running"
    );
}

#[test]
fn resume_puts_a_message_to_the_agent_it_brings_back() {
    // The message goes on the vendor's argv, where `new` puts a task, so the
    // agent has it as soon as the pane starts.
    let amx = Harness::new();
    let id = "fix-login-a1b";
    start(&amx, id, amx.home(), "happy-turn");
    amx.until_state(id, "idle");
    let session = amx.meta(id)["session"]
        .as_str()
        .expect("a session was recorded")
        .to_string();
    amx.amx(&["stop", id, "--force"]);

    said(&resume(&amx, &[id, "and now the linter"]));

    let called = argv_of(&amx, id);
    assert!(called.contains(&format!("--resume={session}")), "{called}");
    assert!(called.contains("and now the linter"), "{called}");
    assert!(
        !called.contains("fix the login bug"),
        "the work asked for once: {called}"
    );

    // The message becomes the handoff's task, which a later resume does not
    // pass again.
    assert_eq!(amx.handoff(id)["task"], "and now the linter");
}

#[test]
fn resume_records_the_message_as_a_send_before_the_vendor_speaks() {
    // `result` returns the turn after the last send, so a send recorded after
    // the vendor starts would let `result` return the previous turn. Resume
    // writes it under the writer lock, which the new pane's hooks wait on.
    let amx = Harness::new();
    let id = "fix-login-a1b";
    start(&amx, id, amx.home(), "happy-turn");
    amx.until_state(id, "idle");
    amx.amx(&["stop", id, "--force"]);
    let before = amx.event_kinds(id).len();

    said(&resume(&amx, &[id, "and now the linter"]));

    let written = amx.event_kinds(id);
    let sent = written
        .iter()
        .position(|kind| kind == "send")
        .unwrap_or_else(|| panic!("no send was recorded: {written:?}"));
    assert_eq!(
        &written[before..=sent],
        ["resume", "send"],
        "the send follows the resume and nothing came between: {written:?}"
    );
    assert_eq!(
        amx.events(id)[sent]["payload"]["text"],
        "and now the linter"
    );

    until_continued(&amx, id);
    assert!(
        amx.event_kinds(id)[sent + 1..].contains(&"SessionStart".to_string()),
        "{written:?}"
    );

    // A resume without a message records no send.
    amx.amx(&["stop", id, "--force"]);
    let before = amx.event_kinds(id).len();
    said(&resume(&amx, &[id]));
    assert!(
        !amx.event_kinds(id)[before..].contains(&"send".to_string()),
        "{:?}",
        amx.event_kinds(id)
    );
}

#[test]
fn resume_that_cannot_place_a_pane_leaves_the_record_as_it_was() {
    // tmux refuses a duplicate session name, so a session squatting on
    // `amx-<id>` makes placing the pane fail after every earlier check passed.
    let amx = Harness::new();
    let id = "fix-login-a1b";
    ran_and_stopped(&amx, id);
    amx.tmux(&[
        "new-session",
        "-d",
        "-s",
        &format!("amx-{id}"),
        "--",
        "sh",
        "-c",
        "while :; do sleep 0.05; done",
    ]);
    let (state, meta, handoff, events) =
        (amx.state(id), amx.meta(id), amx.handoff(id), amx.events(id));

    let out = resume(&amx, &[id, "and now the linter"]);

    assert!(
        !out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert_eq!(
        amx.state(id),
        state,
        "the answer and exit are the ones it ended with"
    );
    assert_eq!(amx.meta(id), meta, "the record names the pane it had");
    assert_eq!(
        amx.handoff(id),
        handoff,
        "a later resume starts from what it had"
    );
    assert_eq!(
        amx.events(id),
        events,
        "nothing was said to a pane that never came"
    );
    assert!(
        !amx.agent_dir(id).join("boot-env.json").exists(),
        "the environment is not left beside a record no boot will read"
    );
}

#[test]
fn resume_never_shows_a_reader_a_fresh_record_naming_the_old_pane() {
    // A record at `starting` over a gone pane reads as an agent that died
    // starting. Resume places the pane first and records it before resetting
    // the state, so a reader never sees the reset state with the old pane.
    let amx = Harness::new();
    let id = "fix-login-a1b";
    let gone = ran_and_stopped(&amx, id);

    let mut resuming = amx
        .amx_command(&["resume", id])
        .env("MOCK_CLAUDE_SCENARIO", amx.scenario("continues-a-session"))
        .env("MOCK_CLAUDE_SESSION_2", CONTINUED)
        .spawn()
        .expect("running amx resume");
    let mut seen = Vec::new();
    while resuming
        .try_wait()
        .expect("waiting for amx resume")
        .is_none()
    {
        let state = amx.state(id)["state"].clone();
        let pane = amx.meta(id)["pane"].clone();
        if state != "stopped" && pane == gone.as_str() {
            seen.push(state);
        }
    }
    assert!(resuming.wait().expect("amx resume").success());
    assert!(seen.is_empty(), "read {seen:?} over the pane that went");
    assert_ne!(amx.pane_of(id), gone);
}

#[test]
fn boot_keeps_what_its_own_pane_prints_and_not_the_recorded_one() {
    // Resume records the new pane only after tmux makes it, so `_boot` can
    // start while the record still names the old pane. The output kept must
    // come from the boot's own pane, not the one the record names.
    let amx = Harness::new();
    let id = "fix-login-a1b";
    let elsewhere = amx.tmux(&[
        "new-session",
        "-d",
        "-P",
        "-F",
        "#{pane_id}",
        "--",
        "sh",
        "-c",
        "while :; do echo somebody else; sleep 0.05; done",
    ]);
    amx.record(id, &elsewhere);
    let dir = amx.agent_dir(id);
    std::fs::write(
        dir.join("handoff.json"),
        json!({ "task": "fix the login bug", "command": ["echo", "booted here"] }).to_string(),
    )
    .unwrap();
    std::fs::write(dir.join("boot-env.json"), "{}").unwrap();

    let pane = amx.in_a_terminal(&[], &["_boot", id]);
    until_pane_gone(&amx, &pane);

    let output = dir.join("output");
    let kept = amx.until("the boot's words in the record", || {
        std::fs::read_to_string(&output)
            .ok()
            .filter(|kept| !kept.is_empty())
    });
    assert!(kept.contains("booted here"), "{kept:?}");
    assert!(!kept.contains("somebody else"), "{kept:?}");
}

#[test]
fn resume_says_a_command_row_has_no_vendor_to_take_a_message() {
    // An `--exec` row has no vendor to read a message, so resume refuses
    // before writing anything and points at `amx new --exec` instead.
    let amx = Harness::new();
    let id = "run-tests-a1b";
    something_else_on_the_server(&amx);
    let out = amx
        .amx_command(&["new", "--name", id, "--exec", "true"])
        .output()
        .expect("running amx new --exec");
    assert!(
        out.status.success(),
        "amx new: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    amx.until_state(id, "done");
    let gone = amx.pane_of(id);
    until_pane_gone(&amx, &gone);
    let before = amx.event_kinds(id);

    let out = resume(&amx, &[id, "and now the linter"]);
    assert_eq!(out.status.code(), Some(1));
    let why = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(why.contains("amx new --exec"), "{why}");
    assert_eq!(amx.pane_of(id), gone, "and nothing was started");
    assert_eq!(amx.event_kinds(id), before, "nor written");
}

#[test]
fn resume_takes_no_message_for_every_agent_at_once() {
    // A message with `--all` is a usage error (64), and nothing is resumed.
    let amx = Harness::new();
    let id = "fix-login-a1b";
    let gone = ran_and_stopped(&amx, id);

    let out = resume(&amx, &["--all", "and now the linter"]);
    assert_eq!(out.status.code(), Some(64));
    assert_eq!(amx.pane_of(id), gone, "and nothing was brought back");
}

#[test]
fn resume_picks_up_an_agent_whose_command_ran_to_the_end() {
    // An agent that finished still has a session to continue.
    let amx = Harness::new();
    let id = "say-hello-b2c";
    start(&amx, id, amx.home(), "finishes");
    amx.until_state(id, "done");

    said(&resume(&amx, &[id]));
    until_continued(&amx, id);
    // Resumed without a message, so it waits at its prompt.
    assert_eq!(amx.state(id)["state"], "idle");
}

#[test]
fn resume_puts_the_agent_back_in_a_session_of_its_own() {
    // The old session went with the pane, and resume makes a new one under
    // the same `amx-<id>` name.
    let amx = Harness::new();
    let id = "quiet-fix-a1b";
    let out = amx
        .amx_command(&[
            "new",
            "--name",
            id,
            "--dir",
            &amx.home().to_string_lossy(),
            "--agent",
            &amx.mock(),
            "fix the login bug",
        ])
        .env("MOCK_CLAUDE_SCENARIO", amx.scenario("happy-turn"))
        .output()
        .expect("running amx new");
    assert!(
        out.status.success(),
        "amx new: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    amx.until_state(id, "idle");
    amx.amx(&["stop", id, "--force"]);
    assert_eq!(amx.state(id)["state"], "stopped");

    let out = resume(&amx, &[id]);
    assert!(said(&out).contains(id));
    let session = amx.tmux(&[
        "display-message",
        "-p",
        "-t",
        &amx.pane_of(id),
        "#{session_name}",
    ]);
    assert_eq!(
        session,
        format!("amx-{id}"),
        "back in the session the id names"
    );
}

#[test]
fn resume_from_inside_tmux_leaves_the_window_where_it_was() {
    // Like `new`, resume run inside tmux must not switch the caller's window
    // or add one to their session.
    let amx = Harness::new();
    let id = "quiet-fix-a1b";
    start(&amx, id, amx.home(), "happy-turn");
    amx.until_state(id, "idle");
    amx.amx(&["stop", id, "--force"]);

    let env = amx.inside_tmux();
    let pane = env
        .iter()
        .find(|(name, _)| name == "TMUX_PANE")
        .map(|(_, pane)| pane.clone())
        .expect("the pane the terminal is in");
    let watching = amx.tmux(&["display-message", "-p", "-t", &pane, "#{session_id}"]);

    // With a second window, a switch of the current window would show.
    amx.tmux(&[
        "new-window",
        "-d",
        "-t",
        &watching,
        "--",
        "sh",
        "-c",
        "while :; do sleep 0.05; done",
    ]);
    let windows = amx.tmux(&["list-windows", "-t", &watching, "-F", "#{window_id}"]);
    let window = amx.tmux(&["display-message", "-p", "-t", &watching, "#{window_id}"]);

    let out = amx
        .amx_command(&["resume", id])
        .env("MOCK_CLAUDE_SCENARIO", amx.scenario("continues-a-session"))
        .env("MOCK_CLAUDE_SESSION_2", CONTINUED)
        .envs(env)
        .output()
        .expect("running amx resume");
    assert!(said(&out).contains(id), "it says what came back");

    assert_eq!(
        amx.tmux(&["display-message", "-p", "-t", &watching, "#{window_id}"]),
        window,
        "the window a person was looking at is the window they are still looking at"
    );
    assert_eq!(
        amx.tmux(&["list-windows", "-t", &watching, "-F", "#{window_id}"]),
        windows,
        "and nothing was added to the session they were in"
    );
    assert_eq!(
        amx.tmux(&[
            "display-message",
            "-p",
            "-t",
            &amx.pane_of(id),
            "#{session_name}",
        ]),
        format!("amx-{id}"),
        "the agent came back beside them, in a session of its own"
    );
}

#[test]
fn resume_all_brings_back_everything_a_dead_server_took() {
    let amx = Harness::new();
    let ids = ["fix-login-a1b", "port-importer-c3d"];
    for id in ids {
        start(&amx, id, amx.home(), "happy-turn");
    }
    for id in ids {
        amx.until_state(id, "idle");
    }

    amx.tmux(&["kill-server"]);

    let out = resume(&amx, &["--all"]);
    let printed = said(&out);
    for id in ids {
        assert!(printed.contains(id), "{printed}");
        until_continued(&amx, id);
        assert!(amx.pane_alive(&amx.pane_of(id)), "{id} has a pane again");
    }
}

#[test]
fn resume_all_says_so_when_there_is_nothing_to_bring_back() {
    let amx = Harness::new();
    let id = "watch-log-c3d";
    amx.play(id, "works-without-end");
    amx.until_state(id, "working");

    let printed = said(&resume(&amx, &["--all"]));
    assert!(
        !printed.contains(id),
        "a running agent is not swept up: {printed}"
    );
    assert!(printed.contains("nothing"), "{printed}");
}

#[test]
fn resume_puts_back_the_tree_that_stopping_took_away() {
    let amx = Harness::new();
    let repo = amx.a_repo();
    let id = "fix-login-a1b";
    start(&amx, id, &repo, "happy-turn");
    amx.until_state(id, "idle");
    let worktree = PathBuf::from(
        amx.meta(id)["worktree"]
            .as_str()
            .expect("a worktree of its own"),
    );

    amx.amx(&["stop", id, "--force"]);
    assert!(!worktree.exists(), "stopping cleared the tree away");

    said(&resume(&amx, &[id]));
    assert!(worktree.exists(), "and resuming needs it back");
    assert!(
        worktree.join("README.md").exists(),
        "on the branch it was working on"
    );
    until_continued(&amx, id);
}

#[test]
fn resume_hands_the_vendor_what_the_agent_was_started_with() {
    let amx = Harness::new();
    let id = "fix-login-a1b";
    start_with(
        &amx,
        id,
        amx.home(),
        "happy-turn",
        &["--", "--add-dir", "/srv/data"],
    );
    amx.until_state(id, "idle");
    let session = amx.meta(id)["session"]
        .as_str()
        .expect("a session was recorded")
        .to_string();

    amx.amx(&["stop", id, "--force"]);
    said(&resume(&amx, &[id]));

    let called = argv_of(&amx, id);
    assert!(
        called.contains("--add-dir /srv/data"),
        "a directory the agent was given access to is one it still needs: {called}"
    );
    assert!(called.contains(&format!("--resume={session}")), "{called}");
}

#[test]
fn resuming_twice_over_asks_for_one_session_and_not_two() {
    // Each resume records the command it launched, so the second one reads a
    // command that already has `--resume`. With two, the vendor would pick
    // which session to open.
    let amx = Harness::new();
    let id = "fix-login-a1b";
    start_with(
        &amx,
        id,
        amx.home(),
        "happy-turn",
        &["--", "--add-dir", "/srv/data"],
    );
    amx.until_state(id, "idle");

    for _ in 0..2 {
        amx.amx(&["stop", id, "--force"]);
        said(&resume(&amx, &[id]));
        until_continued(&amx, id);
    }

    let called = argv_of(&amx, id);
    assert_eq!(
        called.matches("--resume").count(),
        1,
        "one flag, whatever the vendor was launched with before: {called}"
    );
    assert!(
        called.contains(&format!("--resume={CONTINUED}")),
        "{called}"
    );
    assert!(called.contains("--add-dir /srv/data"), "{called}");
}

#[test]
fn resume_says_so_when_there_is_no_session_to_continue() {
    let amx = Harness::new();
    let id = "never-hooked-a1b";
    // The agent never announced a session.
    amx.record(id, "%99");
    amx.set_state(id, json!({ "state": "stopped" }));

    let out = resume(&amx, &[id]);
    assert_eq!(out.status.code(), Some(1));
    let why = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(why.contains("session"), "{why}");
    assert!(why.contains("amx new"), "and what to do instead: {why}");
}

#[test]
fn resume_will_not_take_the_machine_past_max_agents() {
    let amx = Harness::new();
    amx.config("max_agents = 1\n");
    amx.play("watch-log-c3d", "works-without-end");
    amx.until_state("watch-log-c3d", "working");

    let id = "fix-login-a1b";
    amx.record(id, "%99");
    amx.set_state(id, json!({ "state": "stopped" }));

    // The cap is checked before anything about this agent, so its missing
    // session never comes up.
    let out = resume(&amx, &[id]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("max_agents"));
}

/// Start an agent in `project` and stop it, so it holds no place under the
/// caps.
fn stopped_in(amx: &Harness, id: &str, project: &Path) {
    start(amx, id, project, "happy-turn");
    amx.until_state(id, "idle");
    amx.amx(&["stop", id, "--force"]);
    assert_eq!(amx.state(id)["state"], "stopped");
}

#[test]
fn resume_counts_the_cap_against_the_project_the_agent_ran_in() {
    // A resumed agent comes back in its own project, so that project's
    // `max_agents` applies. Each project here allows one agent.
    let amx = Harness::new();
    let alpha = a_project(&amx, "alpha", "max_agents = 1\n");
    let beta = a_project(&amx, "beta", "max_agents = 1\n");

    let id = "fix-login-a1b";
    stopped_in(&amx, id, &alpha);
    start(&amx, "watch-log-c3d", &alpha, "works-without-end");
    amx.until_state("watch-log-c3d", "working");

    let refused = resume(&amx, &[id]);
    assert_eq!(refused.status.code(), Some(2), "blocked, not failed");
    let why = String::from_utf8_lossy(&refused.stderr);
    assert!(why.contains("max_agents is 1"), "{why}");
    assert!(
        why.contains(&alpha.display().to_string()),
        "the project it counted: {why}"
    );

    // Recorded under beta instead, it resumes: alpha's agent does not count
    // there.
    amx.set_meta(id, json!({ "dir": beta.to_string_lossy() }));
    let out = resume(&amx, &[id]);
    assert!(
        out.status.success(),
        "amx resume: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    until_continued(&amx, id);
}

#[test]
fn resume_will_not_take_the_machine_past_max_total() {
    // `max_total` is machine-wide: with alpha's agent in the last place,
    // beta's agent cannot resume, whatever room beta's own cap leaves.
    let amx = Harness::new();
    amx.config("max_total = 1\n");
    let alpha = a_project(&amx, "alpha", "max_agents = 5\n");
    let beta = a_project(&amx, "beta", "max_agents = 5\n");

    let id = "fix-login-a1b";
    stopped_in(&amx, id, &beta);
    start(&amx, "watch-log-c3d", &alpha, "works-without-end");
    amx.until_state("watch-log-c3d", "working");

    let refused = resume(&amx, &[id]);
    assert_eq!(refused.status.code(), Some(2), "blocked, not failed");
    let why = String::from_utf8_lossy(&refused.stderr);
    assert!(why.contains("max_total is 1"), "{why}");
}

#[test]
fn resume_says_so_when_there_is_no_such_agent() {
    let amx = Harness::new();
    let out = resume(&amx, &["never-made-abc"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("never-made-abc"));
}

#[test]
fn attach_brings_back_an_agent_whose_pane_is_gone() {
    // Attach resumes the agent's session in a new pane, then hands the
    // terminal over as usual.
    let amx = Harness::new();
    let id = "fix-login-a1b";
    let gone = ran_and_stopped(&amx, id);
    let session = amx.meta(id)["session"]
        .as_str()
        .expect("a session was recorded")
        .to_string();

    let terminal = a_terminal(&amx, &["attach", id]);

    until_continued(&amx, id);
    let pane = amx.pane_of(id);
    assert_ne!(pane, gone, "a pane of its own again");
    assert!(amx.pane_alive(&pane));

    let called = argv_of(&amx, id);
    assert!(
        called.contains(&format!("--resume={session}")),
        "on the session it had rather than the task it was started on: {called}"
    );
    assert!(!called.contains("fix the login bug"), "{called}");

    until_looking_at_it(&amx, &terminal);
}

#[test]
fn attach_brings_back_an_agent_whose_pane_answers_for_somebody_else() {
    // The recorded pane belongs to another agent now. Attaching to it would
    // show that agent's work under this one's name.
    let amx = Harness::new();
    let id = "fix-login-a1b";
    let theirs = taken_over(&amx, id);

    let terminal = a_terminal(&amx, &["attach", id]);

    until_continued(&amx, id);
    assert_ne!(amx.pane_of(id), theirs, "a pane of its own again");
    assert!(
        amx.pane_alive(&theirs),
        "and the other agent is left where it was"
    );
    until_looking_at_it(&amx, &terminal);
}

#[test]
fn attach_says_so_when_there_is_nothing_to_bring_back() {
    // The agent never announced a session. The error names the missing
    // session, not the missing pane.
    let amx = Harness::new();
    let id = "never-hooked-a1b";
    amx.record(id, "%99");
    amx.set_state(id, json!({ "state": "stopped" }));

    let out = amx.amx(&["attach", id]);
    assert_eq!(out.status.code(), Some(1));
    let why = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(why.contains("session"), "{why}");
    assert!(why.contains("amx new"), "and what to do instead: {why}");
    assert!(
        !why.contains("no pane"),
        "which is a fact about the pane, not a reason: {why}"
    );
}

#[test]
fn attach_says_so_when_amx_never_started_the_agent() {
    // An adopted claude has a session but no recorded command, since it was
    // started by hand. The error says that, not that the handoff file is
    // missing.
    let amx = Harness::new();
    let id = "their-own-a1b";
    let pane = adopted(&amx, id);
    assert_eq!(
        amx.meta(id)["session"],
        ADOPTED,
        "the session is the half amx did record"
    );
    kill_pane(&amx, &pane);

    let out = amx.amx(&["attach", id]);
    assert_eq!(out.status.code(), Some(1));
    let why = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(why.contains(id), "{why}");
    assert!(why.contains("by hand"), "and which half is missing: {why}");
    assert!(
        !why.contains("handoff"),
        "the name of a file amx keeps is not a reason: {why}"
    );
    assert!(
        !why.contains("no pane"),
        "which is a fact about the pane, not a reason: {why}"
    );
}

#[test]
fn resume_says_so_when_amx_never_started_the_agent() {
    // Attach on a gone pane goes through resume, so both refuse in the same
    // words.
    let amx = Harness::new();
    let id = "their-own-a1b";
    let pane = adopted(&amx, id);
    kill_pane(&amx, &pane);

    let out = resume(&amx, &[id]);
    assert_eq!(out.status.code(), Some(1));
    let why = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(why.contains(id), "{why}");
    assert!(why.contains("by hand"), "{why}");
}

#[test]
fn adopt_reads_the_pane_by_the_document_of_the_vendor_it_took_over() {
    // Adoption reads the pane with the adopted vendor's screen document. On
    // pi's dialog, pi's document gives `waiting`; claude's, first in the
    // vendor table, cannot read that screen.
    let amx = Harness::new();
    let theirs = "their-own-pi-a1b";
    adopt_as(
        &amx,
        theirs,
        &a_pi_on_its_dialog(&amx),
        ("PI_SESSION_ID", ADOPTED),
    );
    assert_eq!(
        amx.meta(theirs)["agent"],
        "pi",
        "the vendor whose variable is in the pane"
    );
    assert_eq!(
        amx.state(theirs)["state"],
        "waiting",
        "read by pi's own document, which is the only one that claims this \
         screen"
    );

    // The same screen adopted as claude shows the result depends on the
    // document. It needs its own pane and session: each belongs to one record.
    let mistaken = "read-as-claude-c3d";
    adopt_as(
        &amx,
        mistaken,
        &a_pi_on_its_dialog(&amx),
        (
            "CLAUDE_CODE_SESSION_ID",
            "4c1e8b73-2f60-4a15-9d38-7e2b6c0f9a54",
        ),
    );
    assert_eq!(amx.meta(mistaken)["agent"], "claude");
    assert_eq!(
        amx.state(mistaken)["state"],
        "unknown",
        "claude's document has no anchor on a pi screen, and an adoption is \
         not the moment to guess"
    );
}

#[test]
fn attach_says_so_when_the_row_is_a_command_and_not_an_agent() {
    // A command has no session to continue. The error says so, not that the
    // pane is gone.
    let amx = Harness::new();
    let id = "run-tests-a1b";
    something_else_on_the_server(&amx);
    let out = amx
        .amx_command(&["new", "--name", id, "--exec", "true"])
        .output()
        .expect("running amx new --exec");
    assert!(
        out.status.success(),
        "amx new: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    amx.until_state(id, "done");
    until_pane_gone(&amx, &amx.pane_of(id));

    let out = amx.amx(&["attach", id]);
    assert_eq!(out.status.code(), Some(1));
    let why = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(why.contains("session"), "{why}");
    assert!(
        !why.contains("no pane"),
        "which is a fact about the pane, not a reason: {why}"
    );
}

#[test]
fn attach_with_no_id_takes_the_agent_the_wall_names() {
    // For tmux key bindings, which cannot pass an id, the wall picks the agent
    // in the view's order: a waiting agent sorts above one that has ended.
    let amx = Harness::new();
    let stopped = "fix-login-a1b";
    ran_and_stopped(&amx, stopped);
    amx.play("ask-b2c", "asks-a-question");
    amx.until_state("ask-b2c", "waiting");

    let terminal = a_terminal(&amx, &["attach", "--waiting"]);
    amx.until("the question this was asked for", || {
        amx.capture(&terminal)
            .contains("Do you want to proceed?")
            .then_some(())
    });

    // `--prev` from outside any agent starts at the bottom of the wall, the
    // stopped agent. Its pane is gone, so it is resumed as `attach <id>` would.
    let terminal = a_terminal(&amx, &["attach", "--prev"]);
    until_continued(&amx, stopped);
    until_looking_at_it(&amx, &terminal);
}

#[test]
fn attach_by_the_wall_says_so_when_there_is_no_wall_and_when_there_is_an_id() {
    let amx = Harness::new();

    // An id with `--next` is a usage error, refused before anything is read.
    let out = amx.amx(&["attach", "fix-login-a1b", "--next"]);
    assert_eq!(out.status.code(), Some(64));

    // On an empty wall the error says so instead of naming a missing record.
    let out = amx.amx(&["attach", "--next"]);
    assert_eq!(out.status.code(), Some(1));
    let why = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(why.contains("no agents to attach to"), "{why}");
}

#[test]
fn attach_last_goes_back_to_the_agent_this_terminal_came_from() {
    // Attach records each agent it hands a terminal to on a trail, newest
    // first, and `--last` reads it.
    let amx = Harness::new();
    let first = "fix-login-a1b";
    let second = "port-import-b2c";
    something_else_on_the_server(&amx);
    start(&amx, first, amx.home(), "happy-turn");
    amx.until_state(first, "idle");
    start(&amx, second, amx.home(), "happy-turn");
    amx.until_state(second, "idle");

    // With an empty trail `--last` fails instead of picking some agent.
    let out = amx.amx(&["attach", "--last"]);
    assert_eq!(out.status.code(), Some(1));
    let why = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(why.contains("no agent to go back to"), "{why}");

    a_terminal(&amx, &["attach", first]);
    until_attached(&amx, first);
    a_terminal(&amx, &["attach", second]);
    until_attached(&amx, second);

    // From a shell outside any agent, `--last` goes to the newest entry.
    let terminal = a_terminal(&amx, &["attach", "--last"]);
    until_the_terminal_is_on(&amx, &terminal, second);

    // Run from inside `second`'s pane, as a tmux key binding is, `--last`
    // skips the current agent and goes to `first`. `first`'s pane is gone by
    // then, so it is resumed as `attach <id>` would.
    kill_pane(&amx, &amx.pane_of(first));
    let pane = amx.pane_of(second);
    let scenario = amx.scenario("continues-a-session");
    amx.in_a_terminal(
        &[
            ("TMUX_PANE", &pane),
            ("MOCK_CLAUDE_SCENARIO", &scenario.to_string_lossy()),
            ("MOCK_CLAUDE_SESSION_2", CONTINUED),
        ],
        &["attach", "--last"],
    );

    until_continued(&amx, first);
    assert!(
        amx.pane_alive(&amx.pane_of(first)),
        "a pane of its own again"
    );
    assert!(
        amx.pane_alive(&pane),
        "and the agent the key was pressed in is left where it was"
    );
}

#[test]
fn enter_on_a_dead_agent_brings_it_back() {
    // Outside tmux the view owns the terminal, so Enter hands the terminal
    // itself to the resumed agent.
    let amx = Harness::new();
    let id = "fix-login-a1b";
    let gone = ran_and_stopped(&amx, id);

    let view = a_terminal(&amx, &[]);
    amx.until("the row", || amx.capture(&view).contains(id).then_some(()));
    amx.tmux(&["send-keys", "-t", &view, "Enter"]);

    until_continued(&amx, id);
    let pane = amx.pane_of(id);
    assert_ne!(pane, gone, "a pane of its own again");
    assert!(amx.pane_alive(&pane));
    until_looking_at_it(&amx, &view);

    // Enter in the view also goes on the attach trail.
    let terminal = a_terminal(&amx, &["attach", "--last"]);
    until_the_terminal_is_on(&amx, &terminal, id);
}

#[test]
fn enter_on_an_agent_whose_pane_answers_for_somebody_else_brings_it_back() {
    // Enter resolves the pane like `amx attach`: the agent's own pane or none,
    // never another agent's.
    let amx = Harness::new();
    let id = "fix-login-a1b";
    let theirs = taken_over(&amx, id);

    let view = a_terminal(&amx, &[]);
    amx.until("the row", || amx.capture(&view).contains(id).then_some(()));
    amx.tmux(&["send-keys", "-t", &view, "Enter"]);

    until_continued(&amx, id);
    assert_ne!(amx.pane_of(id), theirs, "a pane of its own again");
    assert!(
        amx.pane_alive(&theirs),
        "and the other agent is left where it was"
    );
    until_looking_at_it(&amx, &view);
}

#[test]
fn enter_on_an_agent_with_nothing_to_resume_says_why() {
    let amx = Harness::new();
    let id = "never-hooked-a1b";
    amx.record(id, "%99");
    amx.set_state(id, json!({ "state": "stopped" }));

    let view = a_terminal(&amx, &[]);
    amx.until("the row", || amx.capture(&view).contains(id).then_some(()));
    amx.tmux(&["send-keys", "-t", &view, "Enter"]);

    let said = amx.until("the view to say why", || {
        let screen = amx.capture(&view);
        screen.contains("session").then_some(screen)
    });
    assert!(
        !said.contains("no longer has a pane"),
        "which is a fact about the pane, not a reason: {said}"
    );
}

#[test]
fn enter_from_inside_tmux_moves_the_client_to_the_session_it_brought_back() {
    // Inside tmux, Enter switches the existing client. The target is the
    // session the resume just made, not the one the row named when Enter was
    // pressed.
    let amx = Harness::new();
    let id = "fix-login-a1b";
    let gone = ran_and_stopped(&amx, id);

    let view = a_terminal_inside_tmux(&amx, &[]);
    let holding = amx.tmux(&["display-message", "-p", "-t", &view, "#{session_name}"]);

    // Attach a client to the view's session for Enter to switch.
    let terminal = watching(&amx, &holding);
    let tty = amx.until("a client on the view", || {
        let clients = clients_on(&amx, &holding);
        (!clients.is_empty()).then_some(clients)
    });

    amx.until("the row", || amx.capture(&view).contains(id).then_some(()));
    amx.tmux(&["send-keys", "-t", &view, "Enter"]);

    until_continued(&amx, id);
    let pane = amx.pane_of(id);
    assert_ne!(pane, gone, "a pane of its own again");
    assert!(amx.pane_alive(&pane));

    until_looking_at_it(&amx, &terminal);
    assert_eq!(
        clients_on(&amx, &format!("amx-{id}")),
        tty,
        "the client that was on the view is the one that moved"
    );
}

#[test]
fn enter_on_a_claude_started_by_hand_says_which_half_is_missing() {
    // The adopted-claude refusal, from the view: the row says the command is
    // missing, not that the handoff file is.
    let amx = Harness::new();
    let id = "their-own-a1b";
    let pane = adopted(&amx, id);
    kill_pane(&amx, &pane);

    let view = a_terminal(&amx, &[]);
    amx.until("the row", || amx.capture(&view).contains(id).then_some(()));
    amx.tmux(&["send-keys", "-t", &view, "Enter"]);

    let said = amx.until("the view to say why", || {
        let screen = amx.capture(&view);
        screen.contains("by hand").then_some(screen)
    });
    assert!(
        !said.contains("no longer has a pane"),
        "which is a fact about the pane, not a reason: {said}"
    );
    assert!(
        !said.contains("handoff"),
        "the name of a file amx keeps is not a reason: {said}"
    );
}
