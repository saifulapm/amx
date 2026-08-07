//! V10 acceptance: `amx integration install|uninstall|status`.
//!
//! Every test here runs the real binary against real configuration files. That
//! is not ceremony: an installer is *only* its effect on somebody else's file,
//! and the failure this task exists to prevent — herdr's `integration status`
//! reporting `current` for hooks that silently do nothing — is invisible to any
//! test that asserts on the installer's own return value.
//!
//! Two things are read rather than restated, so a test cannot drift from the
//! thing it is guarding:
//!
//! - **The event list comes from the registry.** `Registry::builtin()` is the
//!   same parse the server does, over the same embedded `agents.toml`, whose
//!   `hook_events` V01 measured. A test that spelled the eleven Claude Code
//!   events out would pass forever after the next spike changed them.
//! - **The agents' own configuration paths are honoured**, `$CLAUDE_CONFIG_DIR`
//!   and `$CODEX_HOME` where set and `$HOME/.claude` / `$HOME/.codex`
//!   otherwise. Both branches are exercised, and both are pointed at a
//!   temporary directory, so no test can reach the developer's own agents.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use std::path::{Path, PathBuf};
use std::process::Stdio;

use amx_core::agent::AgentKind;
use amx_server::agent::registry::Registry;
use serde_json::{Value, json};

mod support;

use support::{Output, TempDir};

/// The marker this build writes. One place, so a bump is one edit.
const MARKER: &str = "--marker 1";

/// A machine with its own agent homes and its own amx state.
struct Rig {
    /// `$HOME` for every command: the agents' configuration lives under it.
    home: TempDir,
    /// The amx binary under test.
    exe: PathBuf,
}

impl Rig {
    fn new(tag: &str) -> Self {
        Self {
            home: TempDir::new(tag),
            exe: PathBuf::from(env!("CARGO_BIN_EXE_amx")),
        }
    }

    /// Run `amx <args>` with the agent variables unset, so both agents fall
    /// back to `$HOME`.
    fn amx(&self, args: &[&str]) -> Output {
        self.amx_with(args, &[])
    }

    /// Run `amx <args>` with `vars` added to the environment.
    fn amx_with(&self, args: &[&str], vars: &[(&str, &Path)]) -> Output {
        let mut command = std::process::Command::new(&self.exe);
        command
            .args(args)
            .env("HOME", self.home.path())
            .env("XDG_RUNTIME_DIR", self.home.path())
            .env("XDG_STATE_HOME", self.home.path())
            .env("XDG_CONFIG_HOME", self.home.path().join("config"))
            // A developer with either of these exported would otherwise have
            // this suite edit their own agents' configuration.
            .env_remove("CLAUDE_CONFIG_DIR")
            .env_remove("CODEX_HOME")
            .stdin(Stdio::null());
        for (name, value) in vars {
            command.env(name, value);
        }
        let out = command.output().expect("run amx");
        Output {
            code: out.status.code(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        }
    }

    /// Where Claude Code's user settings land.
    fn claude(&self) -> PathBuf {
        self.home.path().join(".claude").join("settings.json")
    }

    /// Where Codex's hooks land.
    fn codex(&self) -> PathBuf {
        self.home.path().join(".codex").join("hooks.json")
    }
}

/// A configuration file's parsed contents.
fn read(path: &Path) -> Value {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|err| panic!("read {}: {err}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|err| panic!("parse {}: {err}", path.display()))
}

/// Write `text` to `path`, creating its directory.
fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().expect("a parent")).expect("create the config dir");
    std::fs::write(path, text).expect("write the config");
}

/// The events an agent's stanza subscribes, straight off the embedded registry.
fn events(agent: &str) -> Vec<String> {
    Registry::builtin()
        .resolve(&AgentKind::new(agent).expect("a valid agent id"))
        .unwrap_or_else(|| panic!("no `{agent}` stanza"))
        .hook_events
        .clone()
}

/// The `hooks` block of a configuration document.
fn block(document: &Value) -> &serde_json::Map<String, Value> {
    document["hooks"].as_object().expect("a hooks object")
}

/// Every command string under `event`.
fn commands(document: &Value, event: &str) -> Vec<String> {
    block(document)[event]
        .as_array()
        .expect("an array of entries")
        .iter()
        .flat_map(|group| group["hooks"].as_array().expect("handlers"))
        .map(|handler| handler["command"].as_str().expect("a command").to_owned())
        .collect()
}

// --------------------------------------------------------------- what it writes

#[test]
fn install_writes_hook_entries_for_exactly_the_registry_event_list() {
    let rig = Rig::new("v10-events");
    let out = rig.amx(&["integration", "install", "claude"]);
    assert!(out.stdout.contains("installed"), "{out:?}");

    let document = read(&rig.claude());
    let mut wrote: Vec<String> = block(&document).keys().cloned().collect();
    let mut wanted = events("claude");
    wrote.sort();
    wanted.sort();
    assert_eq!(
        wrote, wanted,
        "the event list is the stanza's, or it is wrong"
    );

    // Every entry runs this binary's `_hook` with the agent id and the marker.
    for event in &wanted {
        let commands = commands(&document, event);
        assert_eq!(commands.len(), 1, "{event} has one entry: {commands:?}");
        assert!(
            commands[0].starts_with(&rig.exe.display().to_string()),
            "{commands:?}"
        );
        assert!(commands[0].contains("_hook claude"), "{commands:?}");
        assert!(commands[0].ends_with(MARKER), "{commands:?}");
    }

    // Tool-scoped events carry the matcher V01's measured configuration used;
    // the rest carry none.
    let matcher = |event: &str| block(&document)[event][0].get("matcher").cloned();
    assert_eq!(matcher("PreToolUse"), Some(json!("*")));
    assert_eq!(matcher("Stop"), None);

    // The measured caveat is not optional decoration: a correct install in an
    // untrusted folder looks exactly like a broken one.
    assert!(
        out.stdout.contains("trusted"),
        "the folder-trust note: {out:?}"
    );
}

#[test]
fn install_honours_the_agents_own_config_dir_variables() {
    let rig = Rig::new("v10-vars");
    let elsewhere = rig.home.path().join("elsewhere");
    let out = rig.amx_with(
        &["integration", "install"],
        &[
            ("CLAUDE_CONFIG_DIR", elsewhere.as_path()),
            ("CODEX_HOME", elsewhere.as_path()),
        ],
    );
    out.ok();

    assert!(elsewhere.join("settings.json").is_file(), "{out:?}");
    assert!(elsewhere.join("hooks.json").is_file(), "{out:?}");
    assert!(!rig.claude().exists(), "the $HOME fallback was used anyway");
}

// ------------------------------------------------------ what it must not touch

#[test]
fn install_preserves_foreign_hooks_and_reinstall_is_idempotent() {
    let rig = Rig::new("v10-foreign");
    // A settings file with the shapes an installer can damage: keys in an order
    // that is not alphabetical, a hook for an event amx also wants, a hook for
    // an event amx does not, and a group amx's entry will end up sharing.
    write(
        &rig.claude(),
        r#"{
  "model": "opus",
  "hooks": {
    "Stop": [
      {"hooks": [{"type": "command", "command": "/usr/bin/notify-send done"}]}
    ],
    "SessionEnd": [
      {"hooks": [{"type": "command", "command": "/usr/bin/logger bye"}]}
    ]
  },
  "permissions": {"allow": ["Read"]}
}
"#,
    );

    rig.amx(&["integration", "install", "claude"]).ok();
    let document = read(&rig.claude());

    // Both foreign hooks survive, and amx's entry sits beside the one that
    // shares its event rather than replacing it.
    assert_eq!(document["model"], json!("opus"));
    assert_eq!(document["permissions"], json!({"allow": ["Read"]}));
    let stop = commands(&document, "Stop");
    assert!(
        stop.contains(&"/usr/bin/notify-send done".to_owned()),
        "{stop:?}"
    );
    assert!(stop.iter().any(|c| c.contains("_hook claude")), "{stop:?}");
    assert!(
        commands(&document, "SessionEnd").contains(&"/usr/bin/logger bye".to_owned()),
        "the foreign SessionEnd hook was lost"
    );

    // The user's own key order is not amx's to change.
    let text = std::fs::read_to_string(rig.claude()).expect("read");
    let order: Vec<usize> = ["\"model\"", "\"hooks\"", "\"permissions\""]
        .iter()
        .map(|key| text.find(key).unwrap_or_else(|| panic!("{key} is gone")))
        .collect();
    assert!(
        order[0] < order[1] && order[1] < order[2],
        "reordered:\n{text}"
    );

    // And reinstalling over a current install writes nothing at all.
    let before = std::fs::read(rig.claude()).expect("read");
    let again = rig.amx(&["integration", "install", "claude"]);
    assert!(again.stdout.contains("unchanged"), "{again:?}");
    assert_eq!(
        before,
        std::fs::read(rig.claude()).expect("read"),
        "rewritten"
    );
}

#[test]
fn uninstall_removes_only_amx_owned_entries() {
    let rig = Rig::new("v10-uninstall");
    write(
        &rig.claude(),
        r#"{"model": "opus",
            "hooks": {"Stop": [
              {"hooks": [{"type": "command", "command": "/usr/bin/notify-send done"}]}]}}"#,
    );
    rig.amx(&["integration", "install", "claude"]).ok();

    // A user who merged amx's entry into their own group: the handler list is
    // shared, so removal has to be per-handler, not per-group.
    let mut document = read(&rig.claude());
    let ours = commands(&document, "SessionEnd")[0].clone();
    document["hooks"]["SessionEnd"] = json!([{"hooks": [
        {"type": "command", "command": ours},
        {"type": "command", "command": "/usr/bin/logger bye"},
    ]}]);
    write(
        &rig.claude(),
        &serde_json::to_string_pretty(&document).unwrap(),
    );

    let out = rig.amx(&["integration", "uninstall", "claude"]);
    assert!(out.stdout.contains("removed"), "{out:?}");

    let document = read(&rig.claude());
    assert_eq!(document["model"], json!("opus"));
    assert_eq!(commands(&document, "Stop"), ["/usr/bin/notify-send done"]);
    assert_eq!(commands(&document, "SessionEnd"), ["/usr/bin/logger bye"]);
    // Every event amx added and nobody else wanted is gone with it. (The
    // comparison is order-free: `serde_json::Value` sorts its keys, which is
    // exactly why the installer does not use it.)
    let mut left: Vec<&String> = block(&document).keys().collect();
    left.sort();
    assert_eq!(left, ["SessionEnd", "Stop"], "amx left entries behind");

    // Uninstalling twice is not an error and changes nothing.
    let before = std::fs::read(rig.claude()).expect("read");
    let again = rig.amx(&["integration", "uninstall", "claude"]);
    assert!(again.stdout.contains("unchanged"), "{again:?}");
    assert_eq!(before, std::fs::read(rig.claude()).expect("read"));
}

// -------------------------------------------------------------------- status

#[test]
fn status_reports_current_outdated_and_not_installed_from_the_marker() {
    let rig = Rig::new("v10-status");

    let before = rig.amx(&["integration", "status", "claude"]);
    assert!(before.stdout.contains("not installed"), "{before:?}");
    assert_eq!(before.code, Some(0), "nothing installed is not a failure");

    rig.amx(&["integration", "install", "claude"]).ok();
    let current = rig.amx(&["integration", "status", "claude"]);
    assert!(current.stdout.contains("current"), "{current:?}");
    assert!(current.stdout.contains("marker 1"), "{current:?}");
    assert_eq!(current.code, Some(0));

    // An older amx's marker, with everything else identical.
    let text = std::fs::read_to_string(rig.claude()).expect("read");
    write(&rig.claude(), &text.replace(MARKER, "--marker 0"));
    let stale = rig.amx(&["integration", "status", "claude"]);
    assert!(stale.stdout.contains("outdated"), "{stale:?}");
    assert!(stale.stdout.contains("marker 0"), "{stale:?}");

    // And an install that subscribes fewer events than the registry now lists.
    let mut document = read(&rig.claude());
    let hooks = document["hooks"].as_object_mut().expect("a block");
    hooks.remove("Stop");
    write(
        &rig.claude(),
        &serde_json::to_string_pretty(&document).unwrap(),
    );
    let short = rig.amx(&["integration", "status", "claude"]);
    assert!(short.stdout.contains("outdated"), "{short:?}");

    // Reinstalling puts it back.
    rig.amx(&["integration", "install", "claude"]).ok();
    assert!(
        rig.amx(&["integration", "status", "claude"])
            .stdout
            .contains("current")
    );
}

#[test]
fn status_fails_when_the_referenced_amx_binary_is_missing() {
    let rig = Rig::new("v10-broken");
    rig.amx(&["integration", "install", "claude"]).ok();
    let installed = std::fs::read_to_string(rig.claude()).expect("read");

    // The state herdr cannot see: a marker that says `current` on top of a
    // command that cannot run. The hooks are configured and do nothing.
    let gone = rig.home.path().join("moved-away/amx");
    write(
        &rig.claude(),
        &installed.replace(&rig.exe.display().to_string(), &gone.display().to_string()),
    );
    let missing = rig.amx(&["integration", "status", "claude"]);
    assert!(missing.stdout.contains("broken"), "{missing:?}");
    assert_ne!(missing.code, Some(0), "a broken install must fail the verb");

    // A path that does exist and is not amx is just as broken: `status` runs it
    // rather than testing the path, because a hook that execs the wrong program
    // reports nothing and looks installed.
    write(
        &rig.claude(),
        &installed.replace(&rig.exe.display().to_string(), "/bin/sh"),
    );
    let foreign = rig.amx(&["integration", "status", "claude"]);
    assert!(foreign.stdout.contains("broken"), "{foreign:?}");
    assert_ne!(foreign.code, Some(0));
}

// --------------------------------------------------------------------- codex

/// V01 §4 renamed this test.
///
/// The plan's acceptance line reads
/// `codex_install_sets_features_codex_hooks_and_prints_the_trust_caveat`, from
/// third-party documentation of Codex 0.114. On 0.147.0, the version measured,
/// `codex features list` prints `hooks  stable  true`: hooks are on by default
/// and **there is no `codex_hooks` feature to set**. The name survives in the
/// shipped binary only as the Rust crate path
/// `codex_hooks::engine::command_runner`. So the assertion the plan named
/// cannot hold, and writing the key anyway would leave a line Codex never reads
/// in a user's `config.toml`. What the plan actually wanted from this test —
/// that Codex is configured where Codex looks, and that the trust caveat is
/// printed — is asserted below, together with the negative the measurement
/// implies.
#[test]
fn codex_install_writes_only_hooks_json_and_prints_the_trust_caveat() {
    let rig = Rig::new("v10-codex");
    let config = rig.home.path().join(".codex").join("config.toml");
    write(&config, "model = \"gpt-5\"\n");

    let out = rig.amx(&["integration", "install", "codex"]);
    out.ok();

    let document = read(&rig.codex());
    let mut wrote: Vec<String> = block(&document).keys().cloned().collect();
    let mut wanted = events("codex");
    wrote.sort();
    wanted.sort();
    assert_eq!(wrote, wanted);
    // No matchers: the configuration V01 measured firing wrote none.
    for event in &wanted {
        assert_eq!(block(&document)[event][0].get("matcher"), None, "{event}");
        assert!(commands(&document, event)[0].contains("_hook codex"));
    }

    // The features flag the plan asked for does not exist, so nothing is
    // written to `config.toml` — not the key, not a byte.
    assert_eq!(
        std::fs::read_to_string(&config).expect("read"),
        "model = \"gpt-5\"\n",
        "amx edited config.toml"
    );

    // The caveat, in the output, at install time — because an installer that
    // reported success while the hooks sat inert would be herdr's silent no-op
    // in a new costume.
    assert!(out.stdout.contains("approve"), "the trust caveat: {out:?}");
    assert!(out.stdout.contains("codex` run"), "{out:?}");

    // And `status` says what it cannot see, rather than implying they are live.
    let status = rig.amx(&["integration", "status", "codex"]);
    assert!(status.stdout.contains("current"), "{status:?}");
    assert!(status.stdout.contains("cannot see"), "{status:?}");
}

#[test]
fn a_bare_install_covers_every_agent_the_registry_has_hooks_for() {
    let rig = Rig::new("v10-all");
    let out = rig.amx(&["integration", "install"]);
    out.ok();
    assert!(rig.claude().is_file(), "{out:?}");
    assert!(rig.codex().is_file(), "{out:?}");

    let unknown = rig.amx(&["integration", "install", "not-an-agent"]);
    assert_ne!(unknown.code, Some(0), "{unknown:?}");
    assert!(unknown.stderr.contains("not-an-agent"), "{unknown:?}");
}

#[test]
fn a_settings_file_amx_cannot_parse_is_refused_not_overwritten() {
    let rig = Rig::new("v10-unparsable");
    let text = "{ this is not JSON }";
    write(&rig.claude(), text);

    let out = rig.amx(&["integration", "install", "claude"]);
    assert_ne!(out.code, Some(0), "{out:?}");
    // The refusal names the file it refused, which is what distinguishes
    // "amx read your settings and would not touch them" from any other failure.
    assert!(out.stderr.contains("settings.json"), "{out:?}");
    assert_eq!(std::fs::read_to_string(rig.claude()).expect("read"), text);

    // And uninstall is just as careful: it does not get to delete entries out
    // of a file it cannot read either.
    let out = rig.amx(&["integration", "uninstall", "claude"]);
    assert_ne!(out.code, Some(0), "{out:?}");
    assert_eq!(std::fs::read_to_string(rig.claude()).expect("read"), text);
}
