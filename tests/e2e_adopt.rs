//! Tests for claudes started outside amx: `amx ls --loose` lists them, and
//! `amx adopt` moves one into an amx pane on the conversation it had.

mod common;

use common::{Harness, code, stdout};

#[test]
fn ls_loose_lists_a_claude_started_outside_amx_with_its_pid_dir_and_status() {
    let amx = Harness::new();
    let dir = amx.home().join("app");
    std::fs::create_dir_all(&dir).unwrap();
    let claude = amx.loose("sits-loose", &dir, &[]);

    let out = amx.amx(&["ls", "--loose"]);
    assert_eq!(code(&out), 0, "{out:?}");
    let rows: Vec<String> = stdout(&out).lines().map(str::to_string).collect();
    assert_eq!(rows.len(), 1, "{rows:?}");
    let words: Vec<&str> = rows[0].split_whitespace().collect();
    assert_eq!(words[0], claude.pid().to_string(), "{rows:?}");
    assert_eq!(words[1], "idle", "{rows:?}");
    assert_eq!(
        *words.last().unwrap(),
        dir.display().to_string(),
        "{rows:?}"
    );
}

#[test]
fn ls_loose_leaves_out_a_claude_that_has_ended_and_one_a_record_holds() {
    let amx = Harness::new();
    let mut ended = amx.loose("sits-loose", amx.home(), &[]);
    let pid = ended.pid();
    crash(pid);
    amx.until("the stand-in to end", || ended.ended().then_some(()));
    assert!(
        amx.home()
            .join(format!(".claude/sessions/{pid}.json"))
            .exists(),
        "a killed claude leaves its sessions file behind"
    );

    let held = amx.loose(
        "sits-loose",
        amx.home(),
        &[("MOCK_CLAUDE_SESSION", "held-by-a-record")],
    );
    amx.record("an-agent", "%404");
    amx.set_meta(
        "an-agent",
        serde_json::json!({ "session": "held-by-a-record" }),
    );

    let out = amx.amx(&["ls", "--loose"]);
    assert_eq!(
        stdout(&out),
        "no claude is running outside amx\n",
        "{}",
        held.pid()
    );
}

/// SIGKILL `pid`, as a crash would end it, leaving its sessions file.
fn crash(pid: u32) {
    std::process::Command::new("kill")
        .args(["-KILL", &pid.to_string()])
        .status()
        .expect("kill");
}
