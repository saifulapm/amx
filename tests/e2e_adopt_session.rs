//! Tests for a claude adopted in its own pane, which has no `AMX_ID`, keeping
//! its record after `/clear`, `/resume` and `/compact` give it a new session.

mod common;

use common::{Harness, socket_dir, something_else_on_the_server, stdout};
use serde_json::json;
use std::io::Write;
use std::process::Stdio;

/// The session the claude had when it was adopted.
const ADOPTED: &str = "9f3c1d20-5a44-4e7b-8c19-6d0a2b5f7e31";

/// The session a clear, resume or compact moves it to.
const MOVED: &str = "4e8b2f61-7c90-4d3a-b5e2-1f6a9c0d8e47";

/// Start a pane on the harness's server and adopt it as agent `id` with
/// session `session`, the way `amx adopt` run inside a claude does. Answers
/// with the pane.
fn adopted(amx: &Harness, id: &str, session: &str) -> String {
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
        .env("TMUX_PANE", &pane)
        .env("CLAUDE_CODE_SESSION_ID", session)
        .output()
        .expect("running amx adopt");
    assert!(
        out.status.success(),
        "amx adopt: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    pane
}

/// Deliver one hook payload from `pane`, as claude's hook would from a pane
/// with `TMUX` and `TMUX_PANE` but no `AMX_ID`.
fn hook_from(amx: &Harness, pane: &str, payload: serde_json::Value) {
    let tmux = format!("{},1,0", socket_dir().join(amx.socket()).display());
    let mut child = amx
        .amx_command(&["_hook"])
        .env("TMUX", tmux)
        .env("TMUX_PANE", pane)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("running amx _hook");
    child
        .stdin
        .take()
        .expect("stdin was asked for")
        .write_all(payload.to_string().as_bytes())
        .expect("writing the payload");
    let out = child.wait_with_output().expect("waiting for amx _hook");
    assert!(out.status.success(), "amx _hook: {out:?}");
}

#[test]
fn an_adopted_claude_keeps_its_record_through_a_new_session() {
    for source in ["clear", "resume", "compact"] {
        let amx = Harness::new();
        something_else_on_the_server(&amx);
        let id = "fix-login-a1b";
        let pane = adopted(&amx, id, ADOPTED);
        assert_eq!(amx.meta(id)["session"], ADOPTED, "{source}");

        hook_from(
            &amx,
            &pane,
            json!({
                "session_id": MOVED,
                "hook_event_name": "SessionStart",
                "source": source,
            }),
        );
        hook_from(
            &amx,
            &pane,
            json!({
                "session_id": MOVED,
                "hook_event_name": "UserPromptSubmit",
                "prompt": "and the signup bug",
            }),
        );
        hook_from(
            &amx,
            &pane,
            json!({
                "session_id": MOVED,
                "hook_event_name": "Stop",
                "last_assistant_message": "two",
            }),
        );

        assert_eq!(amx.meta(id)["session"], MOVED, "after a {source}");
        assert_eq!(
            stdout(&amx.amx(&["result", id])).trim(),
            "two",
            "after a {source}"
        );
    }
}

#[test]
fn a_fresh_claude_in_a_stamped_pane_leaves_the_record_alone() {
    let amx = Harness::new();
    something_else_on_the_server(&amx);
    let id = "fix-login-a1b";
    let pane = adopted(&amx, id, ADOPTED);

    hook_from(
        &amx,
        &pane,
        json!({
            "session_id": MOVED,
            "hook_event_name": "SessionStart",
            "source": "startup",
        }),
    );

    assert_eq!(amx.meta(id)["session"], ADOPTED);
}

#[test]
fn an_ended_record_is_not_found_by_its_stamp() {
    let amx = Harness::new();
    something_else_on_the_server(&amx);
    let id = "fix-login-a1b";
    let pane = adopted(&amx, id, ADOPTED);
    let mut state = amx.state(id);
    state["state"] = json!("stopped");
    amx.set_state(id, state);
    let events = amx.event_kinds(id);

    hook_from(
        &amx,
        &pane,
        json!({
            "session_id": MOVED,
            "hook_event_name": "SessionStart",
            "source": "clear",
        }),
    );

    assert_eq!(amx.meta(id)["session"], ADOPTED);
    assert_eq!(amx.event_kinds(id), events, "nothing is written to it");
}
