//! `amx doctor` against a real tmux server and real panes.
//!
//! The server check catches a tmux server whose working directory was
//! deleted: every pane it forks then fails at once. Proving it needs a real
//! server holding a real deleted directory. The gate check reads a pane
//! against its vendor's screen rules, so it is tested with mock pi stopped at
//! each of pi's gates.
//!
//! Linux only, because the server check reads another process's working
//! directory through /proc. The whole file sits under that `cfg`.
#![cfg(target_os = "linux")]

mod common;

use common::{Harness, check_line, status};
use serde_json::{Value, json};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

/// A pane command that keeps the server up.
const IDLE: &[&str] = &["sh", "-c", "while :; do sleep 0.05; done"];

const TASK: &str = "fix the login bug";

/// Each screen a fresh pi stops on: a description, the scenario that draws
/// it, and a row unique to that screen.
///
/// Rule names are not repeated here: doctor's output is checked against the
/// rule `amx status` reports, which comes from `assets/screen-rules-pi.toml`.
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

/// Start a server on this harness's socket from a client whose cwd is `cwd`.
///
/// A server takes its working directory from the client that started it, not
/// from a session's `-c`.
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

fn server_line(printed: &str) -> (bool, String) {
    check_line(printed, "server")
}

/// doctor's hooks line for `vendor`: whether it passed, and the line.
///
/// doctor prints one hooks line per installed vendor, so the line is found by
/// vendor, not position.
fn hooks_line(printed: &str, vendor: &str) -> (bool, String) {
    printed
        .lines()
        .find_map(|line| {
            let mut fields = line.split_whitespace();
            let verdict = fields.next()?;
            let named = fields.next()? == "hooks" && fields.next()? == format!("{vendor}:");
            named.then(|| (verdict == "ok", line.to_string()))
        })
        .unwrap_or_else(|| panic!("doctor said nothing about {vendor}'s hooks:\n{printed}"))
}

fn doctor(amx: &Harness) -> String {
    let out = amx.amx(&["doctor"]);
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// The mock pi directory: the stand-in and its scenarios.
fn pi_fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/mock_pi")
}

/// PATH with the mock pi directory first.
fn path_to_pi() -> String {
    let ours = pi_fixtures().to_string_lossy().into_owned();
    match std::env::var("PATH") {
        Ok(rest) => format!("{ours}:{rest}"),
        Err(_) => ours,
    }
}

/// Start an agent on pi, with mock pi playing `scenario`.
///
/// PATH and the scenario go in the environment, which the spawn passes on to
/// the pane.
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

/// The screen rule `amx status` reports for the agent.
fn rule_read(amx: &Harness, id: &str) -> String {
    let agent = status(amx, id);
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

    // Delete the server's working directory while it keeps running.
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
    // Each of these screens blocks the agent until a person answers it. The
    // check must use each vendor's own rules, not claude's folder-trust rule.
    for (what, scenario, drawn) in GATES {
        let amx = Harness::new();
        let id = "fix-login-a1b";
        start_pi(&amx, id, scenario);
        let pane = amx.pane_of(id);

        amx.until(&format!("{what} to be drawn"), || {
            amx.capture(&pane).contains(drawn).then_some(())
        });
        // Age the record so the reader goes by the screen.
        amx.set_state(
            id,
            json!({ "state": "starting", "since": 1, "last_event": 1 }),
        );

        let printed = doctor(&amx);
        let (ok, line) = check_line(&printed, "gate");
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

/// Run doctor with PATH set to exactly `dirs`.
fn doctor_on(amx: &Harness, dirs: &[&Path]) -> String {
    let out = amx
        .amx_command(&["doctor"])
        .env("PATH", std::env::join_paths(dirs).unwrap())
        .output()
        .expect("running amx doctor");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn doctor_asks_a_hooks_line_of_every_agent_this_machine_has() {
    // doctor checks every installed vendor, not only the configured one. A
    // vendor that is not installed is not mentioned.
    let amx = Harness::new();
    amx.config("agent = \"claude\"\n");
    amx.amx(&["setup", "claude"]);

    let claude_only = tempfile::TempDir::new().unwrap();
    let claude = claude_only.path().join("claude");
    std::fs::write(&claude, "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&claude, PermissionsExt::from_mode(0o755)).unwrap();
    let printed = doctor_on(&amx, &[claude_only.path()]);
    let (ok, line) = hooks_line(&printed, "claude");
    assert!(ok, "claude is wired: {line}");
    assert!(
        !printed.contains("pi:"),
        "nothing is missing from a machine that never installed pi:\n{printed}"
    );

    // With pi on PATH too, pi gets a failing line and claude's is unchanged.
    let printed = doctor_on(&amx, &[Path::new(&pi_fixtures()), claude_only.path()]);
    let (ok, _) = hooks_line(&printed, "claude");
    assert!(ok, "claude is still wired:\n{printed}");
    let (ok, line) = hooks_line(&printed, "pi");
    assert!(!ok, "and pi is not:\n{printed}");
    assert!(line.contains("extension"), "{line}");
    assert!(
        printed.contains("amx setup pi"),
        "the line that wires it names pi:\n{printed}"
    );
}

#[test]
fn an_agent_spelled_as_a_path_is_asked_about_as_the_vendor_it_names() {
    // An agent configured as a path to pi is judged as pi. A command the
    // vendor table does not know is judged as claude.
    let amx = Harness::new();
    let pi = pi_fixtures().join("pi");
    amx.config(&format!("agent = \"{}\"\n", pi.display()));
    amx.amx(&["setup", "pi"]);
    let empty = tempfile::TempDir::new().unwrap();

    let printed = doctor_on(&amx, &[empty.path()]);
    let (ok, line) = hooks_line(&printed, "pi");
    assert!(ok, "pi is wired: {line}");
    assert!(
        !printed.contains("claude:"),
        "nothing about claude:\n{printed}"
    );
    let (ok, line) = check_line(&printed, "agent");
    assert!(ok && !line.contains("no entry"), "{line}");

    let wrapper = empty.path().join("my-agent");
    std::fs::write(&wrapper, "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&wrapper, PermissionsExt::from_mode(0o755)).unwrap();
    amx.config(&format!("agent = \"{} --fast\"\n", wrapper.display()));
    amx.amx(&["setup", "claude"]);
    let printed = doctor_on(&amx, &[empty.path()]);
    let (ok, line) = check_line(&printed, "agent");
    assert!(ok, "an unknown agent is not a fault: {line}");
    assert!(
        line.contains("no entry for my-agent: read as claude"),
        "{line}"
    );
    let (ok, line) = hooks_line(&printed, "claude");
    assert!(ok, "and it is judged by claude's wiring: {line}");
}

#[test]
fn doctor_names_the_amx_the_path_finds_when_it_is_not_this_one() {
    // A pi started by hand reports to whichever amx PATH finds, which may be
    // an older install than the one running doctor, so doctor names it.
    let amx = Harness::new();
    let ours = tempfile::TempDir::new().unwrap();
    std::os::unix::fs::symlink(common::AMX, ours.path().join("amx")).unwrap();
    let theirs = tempfile::TempDir::new().unwrap();
    let other = theirs.path().join("amx");
    std::fs::write(&other, "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&other, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();

    // PATH finds this amx through a symlink.
    let printed = doctor_on(&amx, &[ours.path()]);
    let (ok, line) = check_line(&printed, "amx");
    assert!(ok, "one install, under two names: {line}");

    // PATH finds a different `amx` first.
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
    // No server yet is not a fault.
    let amx = Harness::new();

    let printed = doctor(&amx);
    // doctor did print, so the missing server line is meaningful.
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

/// Where pi loads amx's global extension from, under this harness's home.
fn pi_extension(amx: &Harness) -> PathBuf {
    amx.home().join(".pi/agent/extensions/amx.ts")
}

#[test]
fn doctor_writes_pis_extension_once_somebody_agrees_and_uninstall_takes_it_back() {
    // pi reports through an extension file. doctor only judges it; `amx setup
    // pi` writes it and uninstall removes it.
    let amx = Harness::new();
    amx.config("agent = \"pi\"\n");
    let extension = pi_extension(&amx);

    let printed = doctor(&amx);
    let (ok, line) = hooks_line(&printed, "pi");
    assert!(!ok, "nothing is wired yet: {printed}");
    assert!(line.contains("extension"), "{line}");
    assert!(
        printed.contains("amx setup pi"),
        "the remedy is the verb: {printed}"
    );
    assert!(!printed.contains("--fix"), "and not doctor: {printed}");
    assert!(!extension.exists());

    let out = amx.amx(&["setup", "pi"]);
    let printed = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        printed.contains("will write its extension"),
        "it says what it is doing: {printed}"
    );
    assert!(printed.contains("wrote the extension"), "{printed}");
    let written = std::fs::read_to_string(&extension).expect("the extension");
    assert!(written.starts_with("// installed by amx\n"), "{written}");
    assert!(
        written.contains("_hook"),
        "it reports through amx: {written}"
    );
    let printed = doctor(&amx);
    let (ok, line) = hooks_line(&printed, "pi");
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
    let (ok, line) = hooks_line(&printed, "pi");
    assert!(!ok, "a file that is not amx's is not the wiring: {line}");

    let out = amx.amx(&["setup", "pi"]);
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
    let (ok, line) = hooks_line(&printed, "pi");
    assert!(!ok, "{line}");
    assert!(line.contains("not the extension this amx ships"), "{line}");

    let out = amx.amx(&["setup", "pi"]);
    let printed = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        !printed.contains("keeping a copy"),
        "the consent line must not promise a copy it will not keep: {printed}"
    );
    assert!(
        !printed.contains("the file as it was is at"),
        "an older amx's file is amx's to replace, and no copy is kept: {printed}"
    );
    let (ok, _) = hooks_line(&doctor(&amx), "pi");
    assert!(ok);
}

#[test]
fn doctor_forgets_the_trees_claudes_store_still_names_after_they_went() {
    // claude adds a project key for every directory it starts in, and amx
    // makes a tree per agent, so keys for deleted trees pile up.
    let amx = Harness::new();
    amx.config("agent = \"claude\"\n");
    let store = amx.home().join(".claude.json");
    let gone = "/src/app/.amx/worktrees/fix-login-a1b";
    let theirs = "/src/app";
    std::fs::write(
        &store,
        serde_json::to_string_pretty(&json!({
            "numStartups": 412,
            "projects": {
                theirs: { "hasTrustDialogAccepted": true },
                gone: { "hasTrustDialogAccepted": true },
            },
        }))
        .unwrap(),
    )
    .unwrap();

    let printed = doctor(&amx);
    let (ok, line) = check_line(&printed, "store");
    assert!(!ok, "a tree that is gone is still named: {printed}");
    assert!(line.contains("one tree"), "how many: {line}");
    assert!(
        line.contains(&store.display().to_string()),
        "and which file: {line}"
    );

    // --fix removes the key without asking.
    let out = amx.amx(&["doctor", "--fix"]);
    let printed = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        printed.contains(&format!("forgot 1 tree from {}", store.display())),
        "it says what it did: {printed}"
    );

    let after: Value = serde_json::from_str(&std::fs::read_to_string(&store).unwrap()).unwrap();
    assert_eq!(after["projects"].get(gone), None, "{after}");
    assert!(
        after["projects"].get(theirs).is_some(),
        "the repository's entry is the person's own consent: {after}"
    );
    assert_eq!(after["numStartups"], 412, "{after}");

    let copies: Vec<String> = std::fs::read_dir(amx.home())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with(".claude.json.amx-backup-"))
        .collect();
    assert_eq!(copies.len(), 1, "the file as it was, once: {copies:?}");

    let printed = doctor(&amx);
    let (ok, line) = check_line(&printed, "store");
    assert!(ok, "nothing of amx's is left in it: {line}");
}

#[test]
fn doctor_writes_claudes_plugin_once_somebody_agrees_and_uninstall_takes_it_back() {
    // claude reports through a plugin under ~/.claude/skills, which claude
    // loads without a marketplace or a settings entry.
    let amx = Harness::new();
    amx.config("agent = \"claude\"\n");
    let plugin = amx.home().join(".claude/skills/amx");

    let printed = doctor(&amx);
    let (ok, line) = hooks_line(&printed, "claude");
    assert!(!ok, "nothing is wired yet: {printed}");
    assert!(line.contains("plugin"), "and it says which door: {line}");
    assert!(!plugin.exists());

    let out = amx.amx(&["setup", "claude"]);
    let printed = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        printed.contains("will write its plugin"),
        "it says what it is doing: {printed}"
    );

    let manifest = std::fs::read_to_string(plugin.join(".claude-plugin/plugin.json"))
        .expect("the manifest claude loads it by");
    assert!(manifest.contains("\"amx\""), "{manifest}");
    let wiring = std::fs::read_to_string(plugin.join("hooks/hooks.json")).expect("the wiring");
    assert!(
        wiring.contains("amx _hook"),
        "it reports through amx: {wiring}"
    );
    assert!(
        !amx.home().join(".claude/settings.json").exists(),
        "and no settings file of anybody's was opened to get there"
    );

    let printed = doctor(&amx);
    let (ok, line) = hooks_line(&printed, "claude");
    assert!(ok, "the plugin is in place: {line}");

    let out = amx.amx(&["uninstall"]);
    let printed = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(out.status.success(), "{printed}");
    assert!(!plugin.exists(), "and it is gone");
}

#[test]
fn doctor_names_an_id_directory_a_spawn_died_in_and_leaves_a_young_one_to_fix() {
    // A spawn claims its id by creating the directory, then writes the
    // record. A spawn that died in between leaves an id `--name` cannot reuse.
    let amx = Harness::new();
    std::fs::create_dir_all(amx.state_root().join("lost-a1b")).expect("an orphan id");

    let out = amx.amx(&["doctor"]);
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(said.contains("one id directory has no record"), "{said}");
    assert!(said.contains("amx doctor --fix"), "{said}");

    // A directory younger than ten minutes may be a spawn still starting, so
    // --fix leaves it.
    let fixed = amx.amx(&["doctor", "--fix"]);
    let said = String::from_utf8_lossy(&fixed.stdout);
    assert!(said.contains("removed 0 id directories"), "{said}");
    assert!(amx.state_root().join("lost-a1b").exists());
}
