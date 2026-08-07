//! V16 acceptance: the agent skill, and the reference notifier beside it.
//!
//! Both artifacts exist to be *used by something outside amx*, which is why
//! neither is tested by reading the source it was written from:
//!
//! - The skill is checked as the file `amx skill install` actually wrote: every
//!   command it names is resolved against [`SPECS`](amx_proto::control::SPECS),
//!   and every flag it writes after one against the tree the binary parses. A
//!   skill is a set of promises about a CLI; a renamed method or a flag nobody
//!   added has to break here, in a test, rather than in an agent's hands three
//!   weeks later.
//! - The notifier is run — the real `examples/notify.sh`, over a real
//!   `amx events --json`, against a real session, with a stub `notify-send`
//!   first on `$PATH`. 04 §8's claim is that "any program can `amx events
//!   --json`"; the only way to check a claim about *any program* is to be one.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use std::collections::BTreeSet;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use amx_proto::control::SPECS;
use amx_server::session::probe::probe;
use serde_json::{Value, json};

mod support;

use support::{Env, TempDir, wait_until};

/// The verbs the skill may name that are not method rows.
///
/// 04 §1's process lifecycle and 04 §8's streaming consumer: `amx events`
/// (bare) is a long-lived subscriber rather than the `events.subscribe` call,
/// and `skill` writes a file without touching a session. Neither could be a
/// table row, because a row is something a server handles. Everything else the
/// skill names has to be one.
const NON_WIRE: &[&str] = &["events", "skill"];

// ------------------------------------------------------------------- install

#[test]
fn skill_install_writes_the_asset_and_is_idempotent() {
    let env = Env::new("skill-install");
    let dir = TempDir::new("skill-install-target");
    let skill = dir.path().join("SKILL.md");

    let first = env.run(&[
        "skill",
        "install",
        "--path",
        &dir.path().display().to_string(),
    ]);
    assert!(
        first.ok().contains("wrote"),
        "a first install says what it wrote: {first:?}"
    );
    let written = std::fs::read_to_string(&skill).expect("the asset landed");
    assert!(
        written.starts_with("---\nname: amx\n"),
        "the asset is the skill, frontmatter and all:\n{written}"
    );
    assert!(
        first.stdout.contains("SKILL.md"),
        "the output names the path a user has to place: {first:?}"
    );

    // Idempotent means *unchanged*, not "written again with the same bytes":
    // an installer that cannot tell the difference cannot tell a user whether
    // their copy has just been replaced.
    let second = env.run(&[
        "skill",
        "install",
        "--path",
        &dir.path().display().to_string(),
    ]);
    assert!(
        second.ok().contains("unchanged"),
        "a reinstall over a current asset is a no-op: {second:?}"
    );
    assert_eq!(
        std::fs::read_to_string(&skill).expect("still there"),
        written
    );

    // And an asset that has drifted is restored, with the word for it.
    std::fs::write(&skill, "half a skill").expect("edit the asset");
    let third = env.run(&[
        "skill",
        "install",
        "--path",
        &dir.path().display().to_string(),
    ]);
    assert!(
        third.ok().contains("updated"),
        "a differing copy is replaced, and said so: {third:?}"
    );
    assert_eq!(
        std::fs::read_to_string(&skill).expect("restored"),
        written,
        "the shipped asset is the one that wins"
    );

    // No `--path`: the current directory, which is what a user in their
    // project gets when they type the command with no arguments.
    let here = TempDir::new("skill-install-cwd");
    let out = env
        .command()
        .args(["skill", "install"])
        .current_dir(here.path())
        .stdin(Stdio::null())
        .output()
        .expect("run amx skill install");
    assert!(out.status.success(), "{out:?}");
    assert_eq!(
        std::fs::read_to_string(here.path().join("SKILL.md")).expect("the asset landed"),
        written
    );
}

#[test]
fn skill_content_names_only_verbs_and_flags_that_exist() {
    let env = Env::new("skill-verbs");
    let dir = TempDir::new("skill-verbs-target");
    env.run(&[
        "skill",
        "install",
        "--path",
        &dir.path().display().to_string(),
    ])
    .ok();
    let text = std::fs::read_to_string(dir.path().join("SKILL.md")).expect("the asset landed");

    let invocations = invocations(&text);
    let mut named: Vec<Vec<String>> = Vec::new();
    for (path, _) in &invocations {
        if !named.contains(path) {
            named.push(path.clone());
        }
    }
    assert!(
        named.len() >= 10,
        "the extractor found only {} commands in the skill, which is fewer than \
         it teaches — it has stopped reading the file it is guarding: {named:?}",
        named.len()
    );
    let flagged = invocations
        .iter()
        .filter(|(_, flags)| !flags.is_empty())
        .count();
    assert!(
        flagged >= 6,
        "and flags on only {flagged} of them, which is fewer still"
    );

    let known: BTreeSet<Vec<&str>> = SPECS.iter().map(|spec| spec.cli.to_vec()).collect();
    for path in &named {
        let segments: Vec<&str> = path.iter().map(String::as_str).collect();
        if known.contains(&segments) {
            continue;
        }
        // Not a row: then it is one of the handful of verbs that cannot be
        // one, and it still has to exist in the tree the binary parses.
        assert!(
            NON_WIRE.contains(&segments[0]) && segments.len() == 1
                || NON_WIRE.contains(&segments[0]) && command_at(&segments).is_some(),
            "the skill names `amx {}`, which is neither a method row nor one of \
             the non-wire verbs {NON_WIRE:?}",
            segments.join(" "),
        );
    }

    // The other direction, kept deliberately loose: the skill is an
    // introduction, not the schema, so it does not owe a line per row. What it
    // does owe is the surface it exists to teach — the pane-driving verbs of
    // 04 §8 and the agent verbs of 04 §5.
    for must in [
        vec!["pane", "split"],
        vec!["pane", "run"],
        vec!["pane", "read"],
        vec!["pane", "send-keys"],
        vec!["pane", "wait-output"],
        vec!["agent", "start"],
        vec!["agent", "prompt"],
        vec!["wait"],
        vec!["session", "state"],
    ] {
        assert!(
            named.iter().any(|path| path == &must),
            "the skill never teaches `amx {}`",
            must.join(" ")
        );
    }

    // And every flag it writes after a verb is one that verb parses. `--params`
    // reaches every row, the typed flags reach the rows that grew them, and
    // `--session` is global — a skill teaching anything else sends an agent to
    // a usage error.
    let global = long_flags(&amx::cli::cli());
    for (path, flags) in &invocations {
        let segments: Vec<&str> = path.iter().map(String::as_str).collect();
        let Some(command) = command_at(&segments) else {
            continue; // a path the loop above has already refused
        };
        let carried = long_flags(&command);
        for flag in flags {
            assert!(
                carried.contains(flag) || global.contains(flag),
                "the skill teaches `amx {} {flag}`, which that verb does not take",
                segments.join(" ")
            );
        }
    }
}

/// Every `amx …` invocation the skill names: its verb path, and the long flags
/// written after it.
///
/// Prose is not scanned: the file talks *about* amx in sentences, and a test
/// that read those would be checking English. What it checks is the thing a
/// reader would copy — anything in backticks or a fenced block.
fn invocations(text: &str) -> Vec<(Vec<String>, Vec<String>)> {
    let mut chunks = Vec::new();
    let mut fenced = false;
    for line in text.lines() {
        if line.starts_with("```") {
            fenced = !fenced;
        } else if fenced {
            chunks.push(line.to_owned());
        } else {
            // Inline spans are the odd fields between backticks.
            chunks.extend(line.split('`').skip(1).step_by(2).map(str::to_owned));
        }
    }

    let mut found = Vec::new();
    for chunk in chunks {
        let words: Vec<&str> = chunk.split_whitespace().collect();
        for (n, word) in words.iter().enumerate() {
            if *word != "amx" {
                continue;
            }
            let rest = &words[n + 1..];
            let path: Vec<String> = rest
                .iter()
                .take_while(|next| {
                    // A verb, not a flag and not the JSON after one: lowercase
                    // to start with, then the `wait-output` vocabulary.
                    next.starts_with(|c: char| c.is_ascii_lowercase())
                        && next
                            .chars()
                            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
                })
                .take(2)
                .map(|segment| (*segment).to_owned())
                .collect();
            // Flags belong to this invocation until the next one starts. A
            // value that merely looks like a flag (`--model` inside an argv
            // after `--`) is not one, so the scan stops at a bare `--`.
            let flags: Vec<String> = rest
                .iter()
                .take_while(|next| **next != "amx" && **next != "--")
                .filter(|next| {
                    next.strip_prefix("--").is_some_and(|name| {
                        !name.is_empty()
                            && name
                                .chars()
                                .all(|c| c.is_ascii_lowercase() || c == '-' || c.is_ascii_digit())
                    })
                })
                .map(|flag| (*flag).to_owned())
                .collect();
            if !path.is_empty() {
                found.push((path, flags));
            }
        }
    }
    found
}

/// The binary's own command at this path, if it has one.
fn command_at(segments: &[&str]) -> Option<clap::Command> {
    let mut command = amx::cli::cli();
    for segment in segments {
        command = command.find_subcommand(segment)?.clone();
    }
    Some(command)
}

/// Every long flag a command carries, spelled as a user types it.
fn long_flags(command: &clap::Command) -> BTreeSet<String> {
    command
        .get_arguments()
        .filter_map(clap::Arg::get_long)
        .map(|long| format!("--{long}"))
        .collect()
}

// ------------------------------------------------------------------ notifier

/// A pane painting Claude Code's permission dialog, then holding it.
///
/// The lines are the shape of the real screen `docs/notes/hook-coverage.md`
/// recorded and `crates/amx-server/tests/fixtures/manifest/` keeps — the words
/// the shipped manifest's `permission_dialog` rule matches on — because a fake
/// that painted something else would be testing a rule nobody ships.
///
/// It repaints rather than painting once: a tracker ignores its screen for the
/// stanza's startup grace, so a fake that painted before the grace elapsed and
/// then went quiet would leave the hub nothing to evaluate. The loop is bounded
/// and ends holding the pane open.
const FAKE_CLAUDE: &str = r"#!/bin/sh
i=0
while [ $i -lt 30 ]; do
    printf '%s\n' 'Bash command' '' '  echo hello' '' 'Do you want to proceed?' ' 1. Yes' '  2. No' '' 'Esc to cancel'
    sleep 1
    i=$((i+1))
done
exec sleep 300
";

/// The `notify-send` the notifier will find first: a line per notification.
const STUB_NOTIFY_SEND: &str = "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$AMX_TEST_NOTIFY_LOG\"\n";

#[test]
fn notifier_emits_one_desktop_notification_per_attention_enqueue() {
    let env = Env::new("notify");
    let mut server = env
        .command()
        .args(["server", "--session", &env.session])
        // Pinned for the reason the other process-level suites pin it: a pane
        // on the developer's own login shell is slow to start, and an
        // interrupted zsh leaves a lock on their real history file.
        .env("SHELL", "/bin/sh")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn amx server");
    wait_until("the server binds", || {
        probe(&env.socket()).expect("probe").is_running()
    });

    let bin = stub_bin(&env);
    let work = TempDir::new("notify-work");
    let log = work.path().join("notified");
    let errors = work.path().join("notifier.err");
    let mut notifier = start_notifier(&env, bin.path(), &log, &errors);

    // Subscribed before anything blocks, or this would be testing the notifier
    // against an event that predates it. `amx events` announces the sequence it
    // subscribed at on stderr, which is exactly the readiness signal a
    // supervisor of this script would use.
    wait_until("the notifier subscribes", || {
        std::fs::read_to_string(&errors)
            .unwrap_or_default()
            .contains("subscribed at seq")
    });

    // A pane whose program is named `claude` is identified from its argv, gets
    // the shipped Claude Code manifest, and blocks the moment tier 2 sees the
    // permission dialog it paints. Nothing here fakes the queue: the enqueue is
    // the hub's, the event is the bus's, and the notifier reads it over the
    // same `amx events --json` any other program would.
    let pane = split(&env, &bin.path().join("claude"));

    wait_until("the notifier notifies", || lines(&log) >= 1);
    let notified = std::fs::read_to_string(&log).expect("the stub wrote its log");
    assert_eq!(
        lines(&log),
        1,
        "one enqueue is one notification, not one per event: {notified:?}"
    );
    assert!(
        notified.contains(&pane),
        "the notification names the pane that needs input: {notified:?}"
    );

    // And the queue the notifier consumed is the queue everything else reads —
    // D-M2-8's point, and the roadmap's requirement of the reference notifier.
    assert_eq!(
        state(&env)["attention"],
        json!([pane]),
        "`session.state` carries the same one waiting pane"
    );

    let _ = notifier.kill();
    let _ = notifier.wait();
    env.stop();
    let _ = server.kill();
    let _ = server.wait();
}

/// A directory holding the `amx`, `notify-send` and `claude` this test wants
/// found on a `$PATH`.
///
/// `notify-send` may well exist on the machine running this suite; it is
/// shadowed rather than detected, so the test asserts on what the script *did*
/// instead of on a real desktop's notification tray.
fn stub_bin(env: &Env) -> TempDir {
    let bin = TempDir::new("notify-bin");
    std::os::unix::fs::symlink(env.exe(), bin.path().join("amx")).expect("link amx onto the path");
    executable(&bin.path().join("notify-send"), STUB_NOTIFY_SEND);
    executable(&bin.path().join("claude"), FAKE_CLAUDE);
    bin
}

/// Write `text` at `path` and make it runnable.
fn executable(path: &Path, text: &str) {
    std::fs::write(path, text).expect("write the script");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .expect("make the script executable");
}

/// Run `examples/notify.sh` the way a user would: as a program, on a `$PATH`.
fn start_notifier(env: &Env, bin: &Path, log: &Path, errors: &Path) -> Child {
    let path = match std::env::var("PATH") {
        Ok(existing) => format!("{}:{existing}", bin.display()),
        Err(_) => bin.display().to_string(),
    };
    let root = home(env);
    let stderr = std::fs::File::create(errors).expect("open the notifier's error log");
    Command::new("/bin/sh")
        .arg(notifier_script())
        .env_clear()
        .env("PATH", path)
        .env("HOME", &root)
        .env("XDG_RUNTIME_DIR", root.join("run"))
        .env("XDG_STATE_HOME", root.join("state"))
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("AMX_SESSION", &env.session)
        .env("AMX_TEST_NOTIFY_LOG", log)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(stderr))
        .spawn()
        .expect("spawn the notifier")
}

/// The reference notifier, in the repository it ships from.
fn notifier_script() -> PathBuf {
    // `<workspace>/crates/amx/../../examples`: this package sits two levels
    // under the root the script is published at.
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/notify.sh")
}

/// The roots [`Env::command`] pins, rebuilt for a process that is not `amx`.
///
/// `Env` exposes its session's runtime directory rather than its private root,
/// and that directory is `<root>/run/amx/<session>`, so the root is three
/// levels up.
fn home(env: &Env) -> PathBuf {
    env.runtime_dir()
        .ancestors()
        .nth(3)
        .expect("the runtime dir sits under the environment's root")
        .to_owned()
}

/// Open a workspace and split off a pane running `program`, answering with its
/// UUID.
fn split(env: &Env, program: &Path) -> String {
    env.run(&["workspace", "create", "--params", "{}"]).ok();
    let root = state(env)["panes"][0]["pane"]
        .as_str()
        .expect("the new workspace has a pane")
        .to_owned();
    let params = json!({
        "pane": root,
        "direction": "vertical",
        "command": [program.display().to_string()],
    });
    let reply: Value = serde_json::from_str(
        env.run(&["pane", "split", "--params", &params.to_string()])
            .ok(),
    )
    .expect("pane.split replies with JSON");
    reply["pane"]
        .as_str()
        .expect("a split names its new pane")
        .to_owned()
}

/// The session's state.
fn state(env: &Env) -> Value {
    let out = env.run(&["session", "state", "--params", "{}"]);
    serde_json::from_str(out.ok()).expect("session.state replies with JSON")
}

/// How many notifications the stub has recorded.
fn lines(log: &Path) -> usize {
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .count()
}
