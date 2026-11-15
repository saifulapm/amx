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

/// The session the loose stand-in is on.
const SESSION: &str = "0c9e1d2f-4a5b-4c6d-8e7f-9a0b1c2d3e4f";

/// A loose stand-in on [`SESSION`] in `<home>/app`, with mock-claude as the
/// configured agent so the move starts the stand-in again.
fn a_loose_claude(amx: &Harness, env: &[(&str, &str)]) -> (common::Loose, std::path::PathBuf) {
    amx.config(&format!("agent = \"{}\"\n", amx.mock()));
    let dir = amx.home().join("app");
    std::fs::create_dir_all(&dir).unwrap();
    let env = [&[("MOCK_CLAUDE_SESSION", SESSION)][..], env].concat();
    (amx.loose("sits-loose", &dir, &env), dir)
}

/// Run `amx adopt` with `args`, the moved claude set to continue a session.
fn adopt(amx: &Harness, args: &[&str]) -> std::process::Output {
    amx.amx_command(&[&["adopt"], args].concat())
        .env("MOCK_CLAUDE_SCENARIO", amx.scenario("continues-a-session"))
        .output()
        .expect("running amx adopt")
}

/// The id in `moved to amx: amx attach <id>`, which must be all it printed.
fn moved(out: &std::process::Output) -> String {
    assert_eq!(code(out), 0, "{out:?}");
    let said = stdout(out);
    let id = said
        .strip_prefix("moved to amx: amx attach ")
        .and_then(|rest| rest.strip_suffix('\n'))
        .unwrap_or_else(|| panic!("printed {said:?}"));
    id.to_string()
}

#[test]
fn adopt_pid_ends_a_loose_claude_and_resumes_its_conversation_in_an_amx_pane() {
    let amx = Harness::new();
    let (mut claude, dir) = a_loose_claude(&amx, &[]);

    let id = moved(&adopt(&amx, &["--pid", &claude.pid().to_string()]));

    amx.until("the old claude to end", || claude.ended().then_some(()));
    let meta = amx.meta(&id);
    assert_eq!(meta["dir"], dir.display().to_string());
    assert!(meta["worktree"].is_null(), "{meta}");
    let argv = common::argv_of(&amx, &id);
    assert!(argv.contains(&format!("--resume={SESSION}")), "{argv}");
    amx.until_state(&id, "idle");
}

#[test]
fn adopt_session_moves_the_same_claude_as_its_pid() {
    let amx = Harness::new();
    let (mut claude, _) = a_loose_claude(&amx, &[]);

    let id = moved(&adopt(&amx, &["--session", SESSION]));

    amx.until("the old claude to end", || claude.ended().then_some(()));
    let argv = common::argv_of(&amx, &id);
    assert!(argv.contains(&format!("--resume={SESSION}")), "{argv}");
}

#[test]
fn adopt_pid_keeps_the_account_dir_the_old_claude_ran_under() {
    let amx = Harness::new();
    let account = amx.home().join("accounts/work");
    let account = account.display().to_string();
    // The config names another account, as the dots wrapper's pick would.
    let (mut claude, _) = a_loose_claude(&amx, &[("CLAUDE_CONFIG_DIR", &account)]);
    amx.config(&format!(
        "agent = \"{}\"\n[claude.env]\nCLAUDE_CONFIG_DIR = \"/elsewhere\"\n",
        amx.mock()
    ));

    let out = amx
        .amx_command(&["adopt", "--pid", &claude.pid().to_string()])
        .env("MOCK_CLAUDE_SCENARIO", amx.scenario("says-its-account"))
        .output()
        .expect("running amx adopt");
    let id = moved(&out);

    amx.until("the old claude to end", || claude.ended().then_some(()));
    let pane = amx.pane_of(&id);
    amx.until_shown(&id, "env: CLAUDE_CONFIG_DIR=");
    let said = amx.capture(&pane);
    assert!(
        said.lines()
            .any(|line| line.trim_end() == format!("env: CLAUDE_CONFIG_DIR={account}")),
        "{said}"
    );
}

#[test]
fn adopt_pid_keeps_the_old_flags_once_and_drops_the_old_resume_and_first_prompt() {
    let amx = Harness::new();
    amx.config(&format!("agent = \"{}\"\n", amx.mock()));
    let dir = amx.home().join("app");
    std::fs::create_dir_all(&dir).unwrap();
    let claude = amx.loose_with_args(
        "sits-loose",
        &dir,
        &[("MOCK_CLAUDE_SESSION", SESSION)],
        &["--model", "haiku", "--resume", "OLD", "say hi"],
    );

    let id = moved(&adopt(&amx, &["--pid", &claude.pid().to_string()]));

    let argv = common::argv_of(&amx, &id);
    assert_eq!(argv.matches("--model haiku").count(), 1, "{argv}");
    assert_eq!(argv.matches("--resume").count(), 1, "{argv}");
    assert!(!argv.contains("OLD"), "{argv}");
    assert!(!argv.contains("say hi"), "{argv}");
}

#[test]
fn a_moved_claude_stopped_by_amx_resumes_from_its_handoff() {
    let amx = Harness::new();
    let (claude, _) = a_loose_claude(&amx, &[]);
    let id = moved(&adopt(&amx, &["--pid", &claude.pid().to_string()]));
    amx.until_state(&id, "idle");

    let stopped = amx.amx(&["stop", &id]);
    assert_eq!(code(&stopped), 0, "{stopped:?}");
    let out = amx
        .amx_command(&["resume", &id])
        .env("MOCK_CLAUDE_SCENARIO", amx.scenario("continues-a-session"))
        .output()
        .expect("running amx resume");
    assert_eq!(code(&out), 0, "{out:?}");
}

#[test]
fn two_adopts_of_one_claude_at_once_make_one_record() {
    let amx = Harness::new();
    let (claude, _) = a_loose_claude(&amx, &[]);
    let pid = claude.pid().to_string();
    let racers: Vec<_> = (0..2)
        .map(|_| {
            amx.amx_command(&["adopt", "--pid", &pid])
                .env("MOCK_CLAUDE_SCENARIO", amx.scenario("continues-a-session"))
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .expect("running amx adopt")
        })
        .collect();
    let codes: Vec<i32> = racers
        .into_iter()
        .map(|racer| code(&racer.wait_with_output().expect("waiting for amx adopt")))
        .collect();

    let mut sorted = codes.clone();
    sorted.sort();
    assert_eq!(sorted, [0, 1], "{codes:?}");
    assert_eq!(common::agents(&amx).len(), 1);
}
