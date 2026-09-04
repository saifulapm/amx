//! `amx doctor` against a real tmux server, and against real panes.
//!
//! The server check these prove exists because of a machine where every agent
//! died in under a second and doctor stayed green: a tmux server had outlived
//! the directory it was started in, and every pane it forked after that landed
//! somewhere that was not there. Nothing short of a real server proves it —
//! the deleted directory has to be one a real process is really holding.
//!
//! The setup check is here for the same kind of reason: what it names is read
//! off a pane, against the screens document of whichever vendor is drawing on
//! it. A vendor that reports nothing has no other witness, so the only honest
//! way to ask whether doctor sees an agent stopped at that vendor's own gate is
//! to stop one there — which is what `tests/mock_pi` is for.
//!
//! Linux only, which is where the server check is: elsewhere there is no way to
//! read another process's working directory and doctor says nothing about it.
//! The `cfg` below is that check's, and what it costs is the rest of this file:
//! the setup tests sit under it rather than each carrying one of their own.
#![cfg(target_os = "linux")]

mod common;

use common::Harness;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;

/// A shell that sits there, so the server has a pane and stays up.
const IDLE: &[&str] = &["sh", "-c", "while :; do sleep 0.05; done"];

/// The task every agent here is started on.
const TASK: &str = "fix the login bug";

/// Every screen a fresh pi stops on: what it is, the scenario that puts it on a
/// pane, and the row the vendor draws on that screen and no other.
///
/// Which of pi's screens are gates is `assets/screen-rules-pi.toml`'s to say,
/// so no name out of that document is written here either: what doctor printed
/// is weighed against what the same reading told `amx status`.
const GATES: [(&str, &str, &str); 3] = [
    (
        "the folder-trust question",
        "stops-on-trust",
        "Project trust",
    ),
    (
        "the gate pi puts in front of a first run",
        "stops-at-setup",
        "Welcome to pi,",
    ),
    (
        "a pi waiting for a provider's key",
        "stops-on-login",
        "Login to",
    ),
];

/// Start a server on this harness's socket from a client standing in `cwd`.
///
/// A server takes its working directory from whichever client started it and
/// not from the `-c` a session was asked for, so standing the client somewhere
/// is the only way to put a server there on purpose.
fn serve_from(amx: &Harness, cwd: &Path) {
    let out = Command::new("tmux")
        .args(["-L", amx.socket(), "-f", "/dev/null"])
        .args(["new-session", "-d"])
        .args(IDLE)
        .current_dir(cwd)
        .output()
        .expect("starting a server");
    assert!(out.status.success(), "{out:?}");
}

/// Doctor's line about one check: whether it passed, and what it said.
///
/// Read by the check's name rather than by position, and the whole output is
/// carried along so a failure says what doctor actually printed.
fn check_line(printed: &str, name: &str) -> (bool, String) {
    printed
        .lines()
        .find_map(|line| {
            let mut fields = line.split_whitespace();
            let verdict = fields.next()?;
            (fields.next()? == name).then(|| (verdict == "ok", line.to_string()))
        })
        .unwrap_or_else(|| panic!("doctor said nothing about the {name}:\n{printed}"))
}

fn server_line(printed: &str) -> (bool, String) {
    check_line(printed, "server")
}

fn doctor(amx: &Harness) -> String {
    let out = amx.amx(&["doctor"]);
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Where pi's stand-in and its scenarios live.
fn pi_fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/mock_pi")
}

/// A PATH with the stand-in's directory in front of it, which is what makes
/// `pi` a program this machine has at all.
fn path_to_pi() -> String {
    let ours = pi_fixtures().to_string_lossy().into_owned();
    match std::env::var("PATH") {
        Ok(rest) => format!("{ours}:{rest}"),
        Err(_) => ours,
    }
}

/// Start an agent on the vendor amx knows as pi, with the stand-in ready to
/// play `scenario`.
///
/// Both ride the environment rather than the command line because that is how
/// they reach the pane: a spawn snapshots the environment it was run with, and
/// the pane is started from that snapshot.
fn start_pi(amx: &Harness, id: &str, scenario: &str) {
    let scenario = pi_fixtures()
        .join("scenarios")
        .join(format!("{scenario}.scenario"));
    let out = amx
        .amx_command(&[
            "new",
            "--name",
            id,
            "--dir",
            &amx.home().to_string_lossy(),
            "--agent",
            "pi",
            TASK,
        ])
        .env("PATH", path_to_pi())
        .env("MOCK_PI_SCENARIO", scenario)
        .output()
        .expect("running amx new");
    assert!(
        out.status.success(),
        "amx new: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// The rule the same reading names on this agent's row, out of its own
/// vendor's document.
fn rule_read(amx: &Harness, id: &str) -> String {
    let out = amx.amx(&["status", id, "--json"]);
    assert!(
        out.status.success(),
        "amx status: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let agent: Value = serde_json::from_slice(&out.stdout).expect("the status is json");
    agent["rule"]
        .as_str()
        .unwrap_or_else(|| panic!("no rule claimed this screen: {agent}"))
        .to_string()
}

#[test]
fn a_server_standing_somewhere_that_is_still_there_passes() {
    let amx = Harness::new();
    let dir = tempfile::TempDir::new().unwrap();
    serve_from(&amx, dir.path());

    let printed = doctor(&amx);
    let (ok, line) = server_line(&printed);
    assert!(ok, "nothing is wrong with this server: {line}");
}

#[test]
fn a_server_whose_directory_was_deleted_is_named_with_the_way_out() {
    let amx = Harness::new();
    let dir = tempfile::TempDir::new().unwrap();
    let gone = dir.path().canonicalize().unwrap();
    serve_from(&amx, &gone);

    // The whole failure, reproduced: the directory the server is standing in
    // goes, and the server carries on holding it.
    std::fs::remove_dir_all(&gone).unwrap();

    let printed = doctor(&amx);
    let (ok, line) = server_line(&printed);
    assert!(
        !ok,
        "the server is poisoned and doctor passed it: {printed}"
    );
    assert!(
        line.contains(&gone.display().to_string()),
        "the directory it is stuck holding is named: {line}"
    );
    assert!(
        printed.contains(&format!("tmux -L {} kill-server", amx.socket())),
        "and the way out is a command aimed at this server: {printed}"
    );
}

#[test]
fn an_agent_stopped_at_its_own_vendors_setup_gate_is_named() {
    // Three screens a person has to answer before the agent behind them does
    // any work at all, and doctor said nothing about any of them: the check
    // knew one vendor's folder-trust rule by name, so a pi stopped at its own
    // trust question, at the gate in front of a first run, or waiting for a
    // provider's key was an agent nobody was told about.
    for (what, scenario, drawn) in GATES {
        let amx = Harness::new();
        let id = "fix-login-a1b";
        start_pi(&amx, id, scenario);
        let pane = amx.pane_of(id);

        // The row the vendor draws on this screen and on no other, waited for
        // on its own.
        amx.until(&format!("{what} to be drawn"), || {
            amx.capture(&pane).contains(drawn).then_some(())
        });
        // Nothing heard for an hour, with nothing outstanding, which is where
        // the screen is the only witness there is on a vendor that reports
        // nothing.
        amx.set_state(
            id,
            json!({ "state": "starting", "since": 1, "last_event": 1 }),
        );

        let printed = doctor(&amx);
        let (ok, line) = check_line(&printed, "setup");
        assert!(
            !ok,
            "{what} is nobody's but a person's to answer: {printed}"
        );
        assert!(line.contains(id), "the agent stopped there: {line}");
        assert!(
            line.contains(&rule_read(&amx, id)),
            "the screen, as this vendor's own document names it: {line}"
        );
        assert!(
            printed.contains(&format!("amx attach {id}")),
            "and the way to it: {printed}"
        );
    }
}

#[test]
fn a_machine_with_no_server_yet_has_nothing_to_report() {
    // Never having started a server is not a fault, and the next one amx
    // starts will stand somewhere real.
    let amx = Harness::new();

    let printed = doctor(&amx);
    // Doctor ran and said its piece, so the absence below is a check that was
    // not asked rather than output that never arrived.
    assert!(printed.contains("tmux"), "doctor said nothing at all");
    assert!(
        !printed.lines().any(|line| {
            let mut fields = line.split_whitespace();
            fields.next();
            fields.next() == Some("server")
        }),
        "no line at all rather than a green one nobody measured:\n{printed}"
    );
}
