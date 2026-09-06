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

/// Doctor run with `dirs`, and only those, on the PATH.
fn doctor_on(amx: &Harness, dirs: &[&Path]) -> String {
    let out = amx
        .amx_command(&["doctor"])
        .env("PATH", std::env::join_paths(dirs).unwrap())
        .output()
        .expect("running amx doctor");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn doctor_names_the_amx_the_path_finds_when_it_is_not_this_one() {
    // Two installed amx, and `amx doctor --fix` run under the stale one
    // judged the stale extension against its own body, said ok, and the
    // build carrying the fix never ran. What a pi started by hand reports to
    // is whichever amx the PATH finds, so doctor says which that is.
    let amx = Harness::new();
    let ours = tempfile::TempDir::new().unwrap();
    std::os::unix::fs::symlink(common::AMX, ours.path().join("amx")).unwrap();
    let theirs = tempfile::TempDir::new().unwrap();
    let other = theirs.path().join("amx");
    std::fs::write(&other, "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&other, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();

    // The PATH reaches this amx by another name, and nothing else by it.
    let printed = doctor_on(&amx, &[ours.path()]);
    let (ok, line) = check_line(&printed, "amx");
    assert!(ok, "one install, under two names: {line}");

    // The PATH reaches another program of the name first.
    let printed = doctor_on(&amx, &[theirs.path(), ours.path()]);
    let (ok, line) = check_line(&printed, "amx");
    assert!(!ok, "{printed}");
    assert!(
        line.contains(&other.canonicalize().unwrap().display().to_string()),
        "the one the PATH finds is named: {line}"
    );
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

/// Where pi loads a global extension from, under this harness's home: the
/// path pi's own entry names, joined the way `install` joins it.
fn pi_extension(amx: &Harness) -> PathBuf {
    amx.home().join(".pi/agent/extensions/amx.ts")
}

#[test]
fn doctor_writes_pis_extension_once_somebody_agrees_and_uninstall_takes_it_back() {
    // pi reports through a file amx writes where pi loads extensions from,
    // not through entries in a settings file. doctor judges that file, --fix
    // writes it after asking, and uninstall removes it.
    let amx = Harness::new();
    amx.config("agent = \"pi\"\n");
    let extension = pi_extension(&amx);

    let printed = doctor(&amx);
    let (ok, line) = check_line(&printed, "hooks");
    assert!(!ok, "nothing is wired yet: {printed}");
    assert!(line.contains("extension"), "{line}");
    assert!(printed.contains("amx doctor --fix"), "{printed}");
    assert!(!extension.exists());

    let out = amx.amx_with_input(&["doctor", "--fix"], "y\n");
    let printed = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        printed.contains("will write its extension"),
        "it asked: {printed}"
    );
    assert!(printed.contains("wrote the extension"), "{printed}");
    let written = std::fs::read_to_string(&extension).expect("the extension");
    assert!(written.starts_with("// installed by amx\n"), "{written}");
    assert!(
        written.contains("_hook"),
        "it reports through amx: {written}"
    );
    let (ok, line) = check_line(&printed, "hooks");
    assert!(!ok, "the line before the fix said what was wrong: {line}");

    let printed = doctor(&amx);
    let (ok, line) = check_line(&printed, "hooks");
    assert!(ok, "the extension is in place: {line}");

    let out = amx.amx(&["uninstall"]);
    let printed = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(out.status.success(), "{printed}");
    assert!(
        printed.contains(&extension.display().to_string()),
        "uninstall names what it removed: {printed}"
    );
    assert!(!extension.exists(), "and it is gone");
}

#[test]
fn doctor_fix_keeps_a_copy_of_a_file_that_is_not_amxs_and_uninstall_puts_it_back() {
    let amx = Harness::new();
    amx.config("agent = \"pi\"\n");
    let extension = pi_extension(&amx);
    std::fs::create_dir_all(extension.parent().unwrap()).unwrap();
    let theirs = "// somebody else's extension\nexport default function () {}\n";
    std::fs::write(&extension, theirs).unwrap();

    let printed = doctor(&amx);
    let (ok, line) = check_line(&printed, "hooks");
    assert!(!ok, "a file that is not amx's is not the wiring: {line}");

    let out = amx.amx_with_input(&["doctor", "--fix"], "y\n");
    let printed = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        printed.contains("keeping a copy"),
        "it said so first: {printed}"
    );
    assert!(printed.contains("the file as it was is at"), "{printed}");
    assert!(
        std::fs::read_to_string(&extension)
            .unwrap()
            .starts_with("// installed by amx\n")
    );

    let out = amx.amx(&["uninstall"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert_eq!(
        std::fs::read_to_string(&extension).unwrap(),
        theirs,
        "their file is back where it was"
    );
}

#[test]
fn doctor_says_when_the_extension_on_disk_is_not_the_one_this_amx_ships() {
    let amx = Harness::new();
    amx.config("agent = \"pi\"\n");
    let extension = pi_extension(&amx);
    std::fs::create_dir_all(extension.parent().unwrap()).unwrap();
    std::fs::write(&extension, "// installed by amx\n// an older one\n").unwrap();

    let printed = doctor(&amx);
    let (ok, line) = check_line(&printed, "hooks");
    assert!(!ok, "{line}");
    assert!(line.contains("not the extension this amx ships"), "{line}");

    let out = amx.amx_with_input(&["doctor", "--fix"], "y\n");
    let printed = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        !printed.contains("the file as it was is at"),
        "an older amx's file is amx's to replace, and no copy is kept: {printed}"
    );
    let (ok, _) = check_line(&doctor(&amx), "hooks");
    assert!(ok);
}
