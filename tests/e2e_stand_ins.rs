//! The vendor stand-ins under `tests/mock_*`, checked apart from amx.

use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;
use std::io::{BufRead, BufReader};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};

/// mock-claude keeps no file on disk that it would have to remove at exit.
///
/// It used to remove its steps file from an EXIT trap. With one set, bash 5.3
/// catches SIGHUP, and a pane closing while the stand-in read a command
/// substitution left it orphaned and spinning at full CPU.
#[test]
fn mock_claude_leaves_no_steps_file_while_it_runs() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/mock_claude");
    let tmp = tempfile::tempdir().expect("a scratch TMPDIR");
    let mut child = Command::new(fixtures.join("mock-claude"))
        .env(
            "MOCK_CLAUDE_SCENARIO",
            fixtures.join("scenarios/works-without-end.scenario"),
        )
        .env(
            "MOCK_CLAUDE_TRANSCRIPT",
            tmp.path().join("transcript.jsonl"),
        )
        .env("AMX_BIN", "true")
        .env("TMPDIR", tmp.path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .process_group(0)
        .spawn()
        .expect("starting mock-claude");
    let group = Pid::from_raw(child.id() as i32);

    let stdout = child.stdout.take().expect("the stand-in's stdout");
    let reached = BufReader::new(stdout)
        .lines()
        .any(|line| line.is_ok_and(|line| line == "watching the log"));
    let left: Vec<String> = std::fs::read_dir(tmp.path())
        .expect("the scratch TMPDIR")
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("mock-claude-steps."))
        .collect();
    let _ = killpg(group, Signal::SIGKILL);
    let _ = child.wait();

    assert!(reached, "mock-claude never reached its last step");
    assert!(left.is_empty(), "left behind: {left:?}");
}

/// No stand-in sets an EXIT trap, for the reason above.
#[test]
fn no_stand_in_sets_an_exit_trap() {
    let tests = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    for script in [
        "mock_claude/mock-claude",
        "mock_codex/codex",
        "mock_opencode/opencode",
        "mock_pi/pi",
    ] {
        let text = std::fs::read_to_string(tests.join(script)).expect("the stand-in");
        for line in text.lines().map(str::trim_start) {
            let sets_exit =
                line.starts_with("trap ") && line.split_whitespace().any(|w| w == "EXIT");
            assert!(!sets_exit, "{script}: {line}");
        }
    }
}
