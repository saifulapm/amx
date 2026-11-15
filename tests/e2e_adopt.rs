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

/// A loose stand-in playing `scenario` on [`SESSION`] in `<home>/app`.
fn a_loose(amx: &Harness, scenario: &str) -> common::Loose {
    amx.config(&format!("agent = \"{}\"\n", amx.mock()));
    let dir = amx.home().join("app");
    std::fs::create_dir_all(&dir).unwrap();
    amx.loose(scenario, &dir, &[("MOCK_CLAUDE_SESSION", SESSION)])
}

/// Assert `out` is a refusal saying `reason`, with the claude still running
/// and no record written.
fn refused(amx: &Harness, out: &std::process::Output, reason: &str, claude: &mut common::Loose) {
    assert_eq!(code(out), 1, "{out:?}");
    assert!(common::stderr(out).contains(reason), "{out:?}");
    assert!(!claude.ended(), "a refusal leaves the claude running");
    assert!(common::agents(amx).is_empty(), "{:?}", common::agents(amx));
}

#[test]
fn adopt_refuses_a_claude_mid_turn_and_force_moves_it_anyway() {
    let amx = Harness::new();
    let mut claude = a_loose(&amx, "sits-busy");
    let pid = claude.pid().to_string();

    let out = adopt(&amx, &["--pid", &pid]);
    refused(
        &amx,
        &out,
        &format!("refused: {pid} is mid-turn; --force adopts anyway"),
        &mut claude,
    );

    moved(&adopt(&amx, &["--pid", &pid, "--force"]));
    amx.until("the old claude to end", || claude.ended().then_some(()));
}

#[test]
fn adopt_refuses_a_claude_with_background_work_running() {
    let amx = Harness::new();
    let mut claude = a_loose(&amx, "sits-with-a-shell");
    let pid = claude.pid().to_string();

    let out = adopt(&amx, &["--pid", &pid]);
    refused(
        &amx,
        &out,
        &format!("refused: {pid} has background work running; --force adopts anyway"),
        &mut claude,
    );
}

#[test]
fn adopt_refuses_when_run_from_inside_an_amx_agent() {
    let amx = Harness::new();
    let mut claude = a_loose(&amx, "sits-loose");
    amx.record("an-agent", "%404");

    let out = amx
        .amx_command(&["adopt", "--pid", &claude.pid().to_string()])
        .env("AMX_ID", "an-agent")
        .output()
        .expect("running amx adopt");
    assert_eq!(code(&out), 1, "{out:?}");
    assert!(
        common::stderr(&out).contains("refused: an amx agent cannot adopt another claude"),
        "{out:?}"
    );
    assert!(!claude.ended());
    assert_eq!(common::agents(&amx), ["an-agent"]);
}

#[test]
fn adopt_refuses_a_pid_whose_start_is_not_the_one_its_file_names() {
    let amx = Harness::new();
    let mut claude = a_loose(&amx, "sits-loose");
    let pid = claude.pid().to_string();
    // The file a claude before it left, for a pid the system has reused.
    let file = amx.home().join(format!(".claude/sessions/{pid}.json"));
    let mut forged: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    forged["procStart"] = "1".into();
    std::fs::write(&file, forged.to_string()).unwrap();

    let out = adopt(&amx, &["--pid", &pid]);
    refused(
        &amx,
        &out,
        &format!("refused: {pid} is no longer running"),
        &mut claude,
    );
}

#[test]
fn adopt_of_a_claude_that_will_not_end_starts_nothing() {
    let amx = Harness::new();
    let mut claude = a_loose(&amx, "sits-and-will-not-end");
    let pid = claude.pid().to_string();

    let out = adopt(&amx, &["--pid", &pid]);
    assert_eq!(code(&out), 1, "{out:?}");
    assert!(
        common::stderr(&out).contains(&format!("{pid} did not end; nothing was started")),
        "{out:?}"
    );
    assert!(!claude.ended());
    assert!(
        common::agents(&amx).is_empty(),
        "{:?}",
        common::agents(&amx)
    );
}

#[test]
fn resume_refuses_a_session_live_in_another_process() {
    let amx = Harness::new();
    let (claude, _) = a_loose_claude(&amx, &[]);
    let id = moved(&adopt(&amx, &["--pid", &claude.pid().to_string()]));
    amx.until_state(&id, "idle");
    assert_eq!(code(&amx.amx(&["stop", &id])), 0);
    // Someone runs `claude --resume` on the same conversation by hand.
    let session = amx.meta(&id)["session"].as_str().unwrap().to_string();
    let again = amx.loose(
        "sits-loose",
        amx.home(),
        &[("MOCK_CLAUDE_SESSION", &session)],
    );

    let out = amx
        .amx_command(&["resume", &id])
        .env("MOCK_CLAUDE_SCENARIO", amx.scenario("continues-a-session"))
        .output()
        .expect("running amx resume");
    assert_eq!(code(&out), 1, "{out:?}");
    assert!(
        common::stderr(&out).contains(&format!("session {session} is live in pid {}", again.pid())),
        "{out:?}"
    );
}

#[test]
fn adopt_typed_inside_a_loose_claude_moves_that_claude_and_says_where() {
    let amx = Harness::new();
    amx.config(&format!("agent = \"{}\"\n", amx.mock()));
    let dir = amx.home().join("app");
    std::fs::create_dir_all(&dir).unwrap();
    let next = amx.scenario("continues-a-session").display().to_string();
    let mut claude = amx.loose(
        "adopts-itself",
        &dir,
        &[
            ("MOCK_CLAUDE_SESSION", SESSION),
            ("MOCK_CLAUDE_NEXT_SCENARIO", &next),
        ],
    );

    let id = amx.until("amx adopt to say where the claude went", || {
        claude
            .said()
            .lines()
            .find_map(|line| line.strip_prefix("moved to amx: amx attach "))
            .map(str::to_string)
    });
    let started = std::time::Instant::now();
    amx.until("the old claude to end", || claude.ended().then_some(()));
    amx.until_state(&id, "idle");
    assert!(started.elapsed() < std::time::Duration::from_secs(10));
    let argv = common::argv_of(&amx, &id);
    assert!(argv.contains(&format!("--resume={SESSION}")), "{argv}");
    assert_eq!(amx.meta(&id)["dir"], dir.display().to_string());
}
