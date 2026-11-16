//! The buttons on a desktop notice: Open on every one when `[notify] open` is
//! set, and Yes and No on a permission box's.
//!
//! A fake `notify-send` goes first on the pane's PATH. It writes down its
//! arguments and prints the button a person would have clicked, as
//! `notify-send -A` does.

mod common;

use common::{Harness, a_logging, calls, calls_to};
use std::path::Path;
use std::process::{Output, Stdio};
use std::time::{Duration, Instant};

/// The config with Open set to the command from the docs.
const OPEN_IN_FOOT: &str = "notifications = \"desktop\"\n\
                            [notify]\n\
                            open = \"foot -e amx attach {id}\"\n";

/// The permission sentence both scenarios ask with.
const BASH: &str = "Claude needs your permission to use Bash";

/// Start agent `id` with `amx new`, playing `scenario`, with `path` as the
/// PATH its pane and hooks see.
fn start(amx: &Harness, id: &str, scenario: &str, path: &str) {
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
        .env("MOCK_CLAUDE_SCENARIO", amx.scenario(scenario))
        .env("PATH", path)
        .output()
        .expect("running amx new");
    assert!(
        out.status.success(),
        "amx new: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Run `amx _hook` for agent `id` with `payload` on stdin and `path` as PATH,
/// and answer with how long it took to exit and close its output.
fn deliver(amx: &Harness, id: &str, payload: &str, path: &str) -> Duration {
    use std::io::Write;

    let began = Instant::now();
    let mut child = amx
        .amx_command(&["_hook"])
        .env("AMX_ID", id)
        .env("PATH", path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("running amx _hook");
    child
        .stdin
        .take()
        .expect("stdin was asked for")
        .write_all(payload.as_bytes())
        .expect("delivering the payload");
    // To the end of its output: the vendor reads until every holder of the
    // pipe lets go, so a notifier that kept it would hold the vendor too.
    let out: Output = child.wait_with_output().expect("waiting for the hook");
    let took = began.elapsed();
    assert!(out.status.success(), "a hook always exits 0");
    took
}

/// The first line a fake wrote down, once there is one.
fn first_call(amx: &Harness, log: &Path) -> String {
    amx.until("the notice to go up", || calls(log).first().cloned())
}

#[test]
fn a_notice_carries_open_and_a_permission_box_also_yes_and_no() {
    let amx = Harness::new();
    amx.config(OPEN_IN_FOOT);
    let (bin, log) = a_logging(&amx, "notify-send", "");
    let path = common::path_with(&bin);

    start(&amx, "ask-a1b", "asks-a-question", &path);
    amx.until_state("ask-a1b", "waiting");
    assert_eq!(
        first_call(&amx, &log),
        format!("--app-name=amx -A open=Open -- ask-a1b needs an answer {BASH}"),
        "a question that is not a permission box gets Open alone"
    );

    start(&amx, "box-b2c", "asks-permission-and-carries-on", &path);
    amx.until_state("box-b2c", "waiting");
    let second = amx.until("the box's notice", || calls(&log).get(1).cloned());
    assert_eq!(
        second,
        format!(
            "--app-name=amx -A open=Open -A yes=Yes -A no=No -- box-b2c needs an answer {BASH}"
        )
    );
}

#[test]
fn a_notice_without_open_set_has_no_open_but_a_box_keeps_yes_and_no() {
    let amx = Harness::new();
    amx.config("notifications = \"desktop\"\n");
    let (bin, log) = a_logging(&amx, "notify-send", "");
    let path = common::path_with(&bin);

    start(&amx, "ask-a1b", "asks-a-question", &path);
    amx.until_state("ask-a1b", "waiting");
    assert_eq!(
        first_call(&amx, &log),
        format!("--app-name=amx -- ask-a1b needs an answer {BASH}")
    );

    start(&amx, "box-b2c", "asks-permission-and-carries-on", &path);
    amx.until_state("box-b2c", "waiting");
    let second = amx.until("the box's notice", || calls(&log).get(1).cloned());
    assert_eq!(
        second,
        format!("--app-name=amx -A yes=Yes -A no=No -- box-b2c needs an answer {BASH}")
    );
}

#[test]
fn yes_on_a_permission_notice_answers_the_box_and_the_agent_carries_on() {
    let amx = Harness::new();
    amx.config(OPEN_IN_FOOT);
    // A person clicks a moment after the notice goes up, by which time the
    // stand-in is reading keys.
    let (bin, _) = a_logging(&amx, "notify-send", "sleep 0.3; echo yes");

    start(
        &amx,
        "box-b2c",
        "asks-permission-and-carries-on",
        &common::path_with(&bin),
    );

    let state = amx.until_state("box-b2c", "idle");
    assert_eq!(state["result"], "removed the build directory");
    amx.until_shown("box-b2c", "removed the build directory");
    let answer = amx
        .events("box-b2c")
        .into_iter()
        .find(|event| event["kind"] == "answer")
        .expect("the click is written down as an answer");
    assert_eq!(answer["payload"]["key"], "1");
}

#[test]
fn yes_on_a_notice_for_a_box_asked_again_since_types_nothing() {
    let amx = Harness::new();
    amx.config(OPEN_IN_FOOT);
    let go = amx.home().join("go");
    let notifiers = amx.home().join("notifiers");
    let (tmux_bin, tmux_log) = common::a_logging_tmux(&amx);
    // Only the first notice is clicked, and only once the test says so; the
    // notice the second box raises is left alone.
    let (bin, log) = a_logging(
        &amx,
        "notify-send",
        &format!(
            "echo $PPID >> '{notifiers}'\n\
             [ \"$(wc -l < '{log}')\" -eq 1 ] || exit 0\n\
             i=0; until [ -e '{go}' ] || [ $i -ge 1000 ]; do sleep 0.02; i=$((i+1)); done\n\
             echo yes",
            notifiers = notifiers.display(),
            log = amx.home().join("logging-notify-send/calls.log").display(),
            go = go.display(),
        ),
    );
    let path = format!("{}:{}", bin.display(), common::path_with(&tmux_bin));

    start(&amx, "box-b2c", "asks-permission-and-carries-on", &path);
    amx.until_state("box-b2c", "waiting");
    first_call(&amx, &log);

    // Answered at the pane and asked again in the same words, within the
    // second the record counts in.
    deliver(
        &amx,
        "box-b2c",
        r#"{"hook_event_name":"PostToolUse","tool_name":"Bash","tool_input":{"command":"rm -rf build"}}"#,
        &path,
    );
    deliver(
        &amx,
        "box-b2c",
        &format!(
            r#"{{"hook_event_name":"Notification","message":"{BASH}","notification_type":"permission_prompt"}}"#
        ),
        &path,
    );
    amx.until_state("box-b2c", "waiting");

    std::fs::write(&go, "").unwrap();
    let first = amx.until("the first notifier to say who started it", || {
        std::fs::read_to_string(&notifiers)
            .ok()?
            .lines()
            .next()
            .map(str::to_string)
    });
    let proc = Path::new("/proc").join(first.trim());
    amx.until("the click to have been handled", || {
        (!proc.exists()).then_some(())
    });

    assert_eq!(
        calls_to(&tmux_log, "send-keys"),
        0,
        "a click on the old box types nothing: {:?}",
        calls(&tmux_log)
    );
    assert_eq!(amx.state("box-b2c")["state"], "waiting");
    assert!(
        !amx.event_kinds("box-b2c")
            .iter()
            .any(|kind| kind == "answer"),
        "{:?}",
        amx.event_kinds("box-b2c")
    );
}

#[test]
fn the_hook_that_posts_a_notice_returns_while_the_notice_waits_for_a_click() {
    let amx = Harness::new();
    amx.config(OPEN_IN_FOOT);
    let (bin, log) = a_logging(&amx, "notify-send", "sleep 5");
    let path = common::path_with(&bin);
    start(&amx, "ask-a1b", "asks-a-question", &path);
    amx.until_state("ask-a1b", "waiting");
    first_call(&amx, &log);

    deliver(
        &amx,
        "ask-a1b",
        r#"{"hook_event_name":"UserPromptSubmit","prompt":"write the file instead"}"#,
        &path,
    );
    let took = deliver(
        &amx,
        "ask-a1b",
        &format!(
            r#"{{"hook_event_name":"Notification","message":"{BASH}","notification_type":"permission_prompt"}}"#
        ),
        &path,
    );
    assert!(took < Duration::from_secs(1), "the hook took {took:?}");
    let posted = amx.until("the box's notice", || calls(&log).get(1).cloned());
    assert!(posted.contains("-A yes=Yes"), "{posted}");

    // The notifier is still waiting on its click; nothing it holds stops the
    // next hook or a send.
    let took = deliver(
        &amx,
        "ask-a1b",
        r#"{"hook_event_name":"PostToolUse","tool_name":"Bash"}"#,
        &path,
    );
    assert!(
        took < Duration::from_secs(1),
        "the second hook took {took:?}"
    );
    let began = Instant::now();
    let out = amx.amx(&["send", "ask-a1b", "and the docs"]);
    let took = began.elapsed();
    assert!(took < Duration::from_secs(4), "send took {took:?}");
    assert_eq!(
        common::code(&out),
        2,
        "refused for the box, not held: {}",
        common::stderr(&out)
    );
    assert_eq!(
        calls(&log).len(),
        2,
        "the notifier for the box is the one still waiting: {:?}",
        calls(&log)
    );
}

#[test]
fn open_on_a_notice_runs_the_configured_command_for_that_agent() {
    let amx = Harness::new();
    let opened = amx.home().join("opened");
    amx.config(&format!(
        "notifications = \"desktop\"\n[notify]\nopen = \"echo {{id}} > '{}'\"\n",
        opened.display()
    ));
    let (bin, _) = a_logging(&amx, "notify-send", "echo open");

    start(&amx, "ask-a1b", "asks-a-question", &common::path_with(&bin));

    let said = amx.until("the open command to run", || {
        std::fs::read_to_string(&opened)
            .ok()
            .filter(|said| said.ends_with('\n'))
    });
    assert_eq!(said, "ask-a1b\n");
}
