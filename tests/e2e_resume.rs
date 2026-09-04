//! Bringing an agent back: the pane goes, the session does not.

mod common;

use common::Harness;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::process::Output;

/// The id the vendor's stand-in announces for a session it was asked to
/// continue. A resume changes exactly this about an agent, so it is what the
/// tests watch for.
const CONTINUED: &str = "b7d2a5c8-3e14-4f9a-8c26-0d5b1a7e3f42";

/// An agent started the way a person starts one, playing `scenario`.
fn start(amx: &Harness, id: &str, dir: &Path, scenario: &str) {
    start_with(amx, id, dir, scenario, &[]);
}

/// The same, with arguments of the vendor's own after the separator.
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

/// How the vendor's stand-in says it was called, once it has said it.
fn argv_of(amx: &Harness, id: &str) -> String {
    let pane = amx.pane_of(id);
    amx.until("the vendor to say how it was called", || {
        amx.capture(&pane)
            .lines()
            .find(|line| line.starts_with("argv:"))
            .map(str::to_string)
    })
}

/// `amx resume`, with the stand-in ready to play a continued session.
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

/// Wait for the vendor's stand-in to announce the session it was given.
fn until_continued(amx: &Harness, id: &str) {
    amx.until(&format!("{id} to be on its continued session"), || {
        (amx.meta(id)["session"] == CONTINUED).then_some(())
    });
}

/// A terminal of somebody's own, running `amx` with `args`, with the stand-in
/// ready to play a continued session.
///
/// Outside tmux as far as amx can tell, which is what tmux's own two variables
/// say and the only thing that says it: a terminal with nothing else on it is
/// the one that shows what attaching came to.
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

/// The same terminal, inside tmux, which is where most people type `amx`.
///
/// A pane is inside tmux by birth, so this is [`a_terminal`] with tmux's own
/// two variables left where tmux put them. The view answers to them: inside
/// one it has a client to move and outside one it has the terminal itself.
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

/// A person looking at a session: a tmux client of their own, on a terminal of
/// its own, the way somebody who typed `tmux attach` has one.
///
/// tmux's two variables are cleared for it, because the pane the client starts
/// in is itself inside tmux and a client that knows that declines to nest.
fn watching(amx: &Harness, session: &str) -> String {
    amx.tmux(&[
        "new-session",
        "-d",
        "-P",
        "-F",
        "#{pane_id}",
        "--",
        "env",
        "-u",
        "TMUX",
        "-u",
        "TMUX_PANE",
        "tmux",
        "-L",
        amx.socket(),
        "-f",
        "/dev/null",
        "attach-session",
        "-t",
        session,
    ])
}

/// The terminals of whoever is looking at a session, if anybody is.
fn clients_on(amx: &Harness, session: &str) -> String {
    amx.tmux(&["list-clients", "-t", session, "-F", "#{client_tty}"])
}

/// Wait for the continued session to be drawing on this terminal.
fn until_looking_at_it(amx: &Harness, terminal: &str) {
    amx.until("the agent on the screen", || {
        amx.capture(terminal)
            .contains("continuing where we left off")
            .then_some(())
    });
}

/// An agent that ran, and whose pane is gone: what somebody comes back to in
/// the morning. Answers with the pane it used to be in.
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

/// A claude somebody started themselves, taken onto the wall by `amx adopt`.
///
/// The one shape of agent amx has a session for and no command: that claude
/// was run by hand, in a pane amx never opened. Answers with its pane.
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
        // The two variables the verb reads: tmux says which pane the command
        // was typed in, and the vendor says which conversation typed it.
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

/// The conversation the adopted agent says it is.
const ADOPTED: &str = "9f3c1d20-5a44-4e7b-8c19-6d0a2b5f7e31";

/// A pi somebody started themselves, stopped on the dialog it raises, in a
/// pane amx never opened. Answers with that pane, once the dialog is on it.
///
/// The stand-in next door to mock-claude, started here by hand: what makes a
/// pane pi's is the program running in it, and adoption is about a pane that
/// was running before amx was asked about it. Nothing here goes through the
/// PATH the way `amx new --agent pi` has to.
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

    // The hint row pi draws under every dialog, which is the anchor its own
    // document reads the screen by. A capture taken before it is painted is a
    // different screen, and adoption reads the pane once.
    amx.until("pi's dialog on the pane", || {
        amx.capture(&pane).contains("↑↓ navigate").then_some(())
    });
    pane
}

/// `amx adopt`, typed in a pane by the vendor that names `session`.
///
/// The suite is run from inside somebody's own agent often enough that a
/// vendor's session variable is already in this process's environment. claude
/// is the first entry in the table, so a stray copy of its variable would
/// answer for every adoption here before the vendor under test was reached.
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

/// Something on the server that is not the agent under test, the way a machine
/// somebody works on has something else on it.
///
/// Losing the last pane takes the server with it, and a server that starts
/// again hands the next pane the id the dead one had — which is a test
/// measuring tmux rather than amx.
fn something_else_on_the_server(amx: &Harness) {
    amx.tmux(&[
        "new-session",
        "-d",
        "--",
        "sh",
        "-c",
        "while :; do sleep 0.05; done",
    ]);
}

/// Wait until the pane is gone, however it went.
fn until_pane_gone(amx: &Harness, pane: &str) {
    amx.until("the pane to go", || (!amx.pane_alive(pane)).then_some(()));
}

/// Take the pane away, and wait until it has gone.
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

    // The vendor was handed the session the agent already had, and not the
    // task it was started on: that work was asked for once.
    let pane = amx.pane_of(id);
    let called = amx.until("the vendor to say how it was called", || {
        let screen = amx.capture(&pane);
        screen.contains("argv:").then_some(screen)
    });
    assert!(called.contains(&format!("--resume={session}")), "{called}");
    assert!(!called.contains("fix the login bug"), "{called}");

    // And the record is the same agent, back at the beginning of a turn.
    until_continued(&amx, id);
    assert!(amx.pane_alive(&pane));
    assert_ne!(amx.state(id)["state"], "stopped");
    assert_eq!(amx.state(id)["exit"], json!(null), "it is running again");
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
    // Two `amx resume <id>` in flight at once. One session may only be
    // continued once: two panes both running `--resume=<same session>` would
    // fight over one record, so the loser has to hear that the agent is
    // already going again.
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
    // Both racers are up and spinning before the starting gun fires, so they
    // reach the has-it-ended gate together instead of one whole run apart.
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
fn resume_picks_up_an_agent_whose_command_ran_to_the_end() {
    // How an agent ended is not whether there is a session behind it. One that
    // finished has an answer and a session, and picking that session up is how
    // somebody carries on from it.
    let amx = Harness::new();
    let id = "say-hello-b2c";
    start(&amx, id, amx.home(), "finishes");
    amx.until_state(id, "done");

    said(&resume(&amx, &[id]));
    until_continued(&amx, id);
    assert_eq!(amx.state(id)["state"], "starting");
}

#[test]
fn resume_puts_the_agent_back_in_a_session_of_its_own() {
    // The pane the agent had went with the session that held it, and a resume
    // makes both again under the same name: an id is what addresses an agent,
    // whichever pane it is in this time.
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
    // The second door that starts a pane, held to what the first one promises:
    // whoever typed the command is looking at a window they chose, and nothing
    // amx does may take it from them.
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

    // A second window, so the one being looked at is a choice and not the only
    // thing there is to look at.
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

    // The server dies, and every pane on it goes with it.
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
fn clibatch_resume_hands_the_vendor_what_the_agent_was_started_with() {
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
fn clibatch_resuming_twice_over_asks_for_one_session_and_not_two() {
    // Each resume records what it launched, so the second one reads a command
    // that already names a session. Two of them would leave which session the
    // vendor opens up to the vendor.
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
    // A record whose agent never announced a session: nothing was ever
    // started that could be picked up again.
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

    // The cap is about what the machine is already running, so it is answered
    // before anything about this agent is.
    let out = resume(&amx, &[id]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("max_agents"));
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
    // What somebody asked for is to look at this agent, and a pane that is
    // gone is not an answer to that. The session behind it is, so attaching
    // picks it up and hands the terminal over exactly as it always did.
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
fn attach_says_so_when_there_is_nothing_to_bring_back() {
    // A record whose agent never announced a session: there is nothing to pick
    // up, and saying which is missing beats saying that the pane is.
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
    // An adopted claude is the one agent with a session behind it that amx
    // still cannot bring back: it was started by hand, and amx wrote down no
    // command to start a second time. That is the reason, and a complaint
    // about a file amx keeps for itself is not.
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
    // The same refusal at a shell prompt, in the same words: the verb and the
    // door that becomes it read the record the same way.
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
    // What is in a pane amx did not open is whatever somebody started, so the
    // screens the record's first reading is made against are that vendor's.
    // This pane is pi blocked on a dialog: pi's document claims that screen,
    // and the difference between reading it with that document and with the
    // one at the head of the table is a record that says somebody is wanted
    // here against a record that says amx cannot tell.
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

    // The same screen taken over as claude, which is what makes the reading
    // above evidence about the document and not about the screen. A second
    // pane and a second conversation, because one of either is one record's.
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
    // The other shape with nothing to continue. A command has no conversation
    // to pick up wherever it got to, and the answer says which is missing
    // rather than that the pane has gone.
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
fn enter_on_a_dead_agent_brings_it_back() {
    // The wall's own door to the same thing. Outside tmux the view is the
    // terminal, so what it has to give the agent is the terminal itself.
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
        !said.contains("no pane any more"),
        "which is a fact about the pane, not a reason: {said}"
    );
}

#[test]
fn enter_from_inside_tmux_moves_the_client_to_the_session_it_brought_back() {
    // The view's other way through, and the one most people are on. Inside
    // tmux the terminal is not the view's to lend: there is a client on it
    // already, and that client is what moves. What it moves to is the session
    // the resume has just made, which is not the session the row named when
    // the key went down.
    let amx = Harness::new();
    let id = "fix-login-a1b";
    let gone = ran_and_stopped(&amx, id);

    let view = a_terminal_inside_tmux(&amx, &[]);
    let holding = amx.tmux(&["display-message", "-p", "-t", &view, "#{session_name}"]);

    // Without a client there is nothing for enter to move, and a view nobody
    // has attached to is not a view anybody is reading.
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
    // The other thing that can be missing, at the view's door rather than the
    // shell's. There is a session here and no command to carry it, and what
    // the row says is that, not the name of a file amx keeps for itself.
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
        !said.contains("no pane any more"),
        "which is a fact about the pane, not a reason: {said}"
    );
    assert!(
        !said.contains("handoff"),
        "the name of a file amx keeps is not a reason: {said}"
    );
}
