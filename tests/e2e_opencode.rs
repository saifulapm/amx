//! opencode support, driven through mock opencode.
//!
//! opencode takes its task in a `--prompt=` flag, its model in the pane's
//! environment, needs two Escapes to cancel a turn, and ends a turn on a
//! signal. These tests cover the task arriving as one `--prompt=` argument,
//! the record moving on the plugin's reports, `stop` ending a turn before the
//! pane, and `setup` placing the plugin where the TUI loads it.
//!
//! `tests/mock_opencode/opencode` is found on PATH and replays scenarios over
//! the screens captured from opencode 2.0.16 in `tests/opencode/screens`. It
//! reports through `amx _hook` as the plugin does, so this build of amx is
//! put on PATH too.

mod common;

use common::{AMX, Harness, said_in, status, tree, until_read};
use serde_json::json;
use std::path::{Path, PathBuf};

const TASK: &str = "fix the login bug";

/// The answer in `takes-a-turn`'s Ended report and message list.
const ANSWERED: &str = "done";

/// The plugin as amx ships it.
const PLUGIN: &str = include_str!("../assets/opencode/tui.js");

/// The mock opencode directory: the stand-in and its scenarios.
fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/mock_opencode")
}

fn scenario(name: &str) -> PathBuf {
    fixtures()
        .join("scenarios")
        .join(format!("{name}.scenario"))
}

/// PATH with mock opencode and this build of amx (as `amx`, the name the
/// plugin runs) in front.
fn path_to_opencode(amx: &Harness) -> String {
    let bin = amx.home().join("bin");
    if !bin.join("amx").exists() {
        std::fs::create_dir_all(&bin).expect("a directory for amx");
        std::os::unix::fs::symlink(AMX, bin.join("amx")).expect("amx on the PATH");
    }
    let ours = format!("{}:{}", bin.display(), fixtures().display());
    match std::env::var("PATH") {
        Ok(rest) => format!("{ours}:{rest}"),
        Err(_) => ours,
    }
}

/// Run amx with mock opencode on PATH, playing `scenario_name`.
///
/// Both go in the environment, which a spawn passes on to its pane.
fn amx_with_opencode(amx: &Harness, scenario_name: &str, args: &[&str]) -> std::process::Output {
    amx.amx_command(args)
        .env("PATH", path_to_opencode(amx))
        .env("MOCK_OPENCODE_SCENARIO", scenario(scenario_name))
        .output()
        .expect("running amx")
}

/// Start an agent on opencode on `task`, with extra amx flags `more`.
///
/// amx's own parser would read a task starting with `-` as a flag, so the
/// task goes in a brief file instead.
fn start_with(amx: &Harness, id: &str, scenario_name: &str, more: &[&str], task: &str) {
    let dir = amx.home().to_string_lossy().into_owned();
    let brief = amx.home().join(format!("{id}.md"));
    let brief = brief.to_string_lossy().into_owned();
    let mut args = vec!["new", "--name", id, "--dir", &dir, "--agent", "opencode"];
    args.extend(more);
    if task.starts_with('-') {
        std::fs::write(&brief, task).expect("the brief");
        args.extend(["--file", &brief]);
    } else {
        args.push(task);
    }
    let out = amx_with_opencode(amx, scenario_name, &args);
    assert!(
        out.status.success(),
        "amx new: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn start(amx: &Harness, id: &str, scenario_name: &str, task: &str) {
    start_with(amx, id, scenario_name, &[], task);
}

/// The rest of the latest pane line that starts with `opening`, once there
/// is one.
fn said(amx: &Harness, id: &str, opening: &str) -> String {
    let pane = amx.pane_of(id);
    amx.until(&format!("the vendor to say {opening}"), || {
        said_in(amx, &pane)
            .lines()
            .rev()
            .find_map(|line| line.strip_prefix(opening))
            .map(str::to_string)
    })
}

/// The session id the stand-in printed.
fn minted(amx: &Harness, id: &str) -> String {
    let session = said(amx, id, "session: started ");
    assert!(session.starts_with("ses_"), "opencode's own id: {session}");
    session
}

/// Assert the stand-in reported no error and is still running.
fn went_right(amx: &Harness, id: &str) {
    let pane = amx.pane_of(id);
    let history = said_in(amx, &pane);
    assert!(
        !history.contains("mock opencode:"),
        "the stand-in said something went wrong:\n{history}"
    );
    assert!(amx.pane_alive(&pane), "the stand-in is still running");
}

#[test]
fn a_task_opening_with_a_dash_rides_on_one_prompt_word() {
    // The TUI never takes a word starting with `-` as a flag's value, so the
    // task must be joined to `--prompt=`.
    let amx = Harness::new();
    let id = "loud-a1b";
    start(&amx, id, "takes-a-turn", "-v is too loud");

    assert_eq!(
        said(&amx, id, "argv: "),
        "'--standalone' '--prompt=-v is too loud'"
    );
    assert_eq!(said(&amx, id, "prompt: "), "'-v is too loud'");
    minted(&amx, id);
    until_read(&amx, id, "idle");
    went_right(&amx, id);
}

#[test]
fn a_task_ending_on_a_popup_word_gets_a_space_after_it() {
    // `@` opens the file popup, which would take the Enter that submits.
    let amx = Harness::new();
    let id = "readme-a1b";
    start(&amx, id, "takes-a-turn", "look at @README");

    assert_eq!(
        said(&amx, id, "argv: "),
        "'--standalone' '--prompt=look at @README '"
    );
    until_read(&amx, id, "idle");
}

#[test]
fn an_opencode_turn_moves_the_record_by_its_plugin_and_answers_at_its_end() {
    let amx = Harness::new();
    let id = "fix-login-a1b";
    start(&amx, id, "takes-a-turn", TASK);
    let session = minted(&amx, id);
    assert_eq!(said(&amx, id, "model: "), "default");
    assert_eq!(said(&amx, id, "auto: "), "0");

    let agent = until_read(&amx, id, "idle");
    assert_eq!(agent["evidence"], "hooks", "the plugin's word: {agent}");
    assert_eq!(agent["result"], ANSWERED, "{agent}");

    // The session comes from the first session report, and the transcript is
    // the message list the plugin writes next to the record.
    let meta = amx.meta(id);
    assert_eq!(meta["session"], session, "{meta}");
    assert_eq!(
        meta["transcript"],
        json!(amx.agent_dir(id).join("opencode-messages.jsonl")),
        "{meta}"
    );
    let kinds = amx.event_kinds(id);
    for word in [
        "session.selected",
        "session.execution.started",
        "session.tool.called",
        "session.execution.ended",
    ] {
        assert!(kinds.iter().any(|kind| kind == word), "{word} in {kinds:?}");
    }

    let out = amx.amx(&["result", id, "--timeout", "30"]);
    assert!(
        out.status.success(),
        "amx result: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), ANSWERED);

    let out = amx.amx(&["logs", id]);
    let printed = String::from_utf8_lossy(&out.stdout);
    assert!(printed.contains("sleep 40"), "{printed}");
    went_right(&amx, id);
}

#[test]
fn an_interrupt_presses_escape_twice_and_the_plugin_ends_the_turn() {
    // The first Escape only arms opencode's cancel; the second one cancels.
    let amx = Harness::new();
    let id = "fix-login-a1b";
    start(&amx, id, "is-interrupted", TASK);
    until_read(&amx, id, "working");

    let out = amx.amx(&["interrupt", id]);
    assert!(
        out.status.success(),
        "amx interrupt: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let agent = until_read(&amx, id, "idle");
    assert_eq!(agent["evidence"], "hooks", "the plugin said so: {agent}");
    assert!(
        amx.event_kinds(id)
            .iter()
            .any(|kind| kind == "session.execution.ended")
    );

    let out = amx.amx(&["result", id, "--timeout", "5"]);
    assert_eq!(out.status.code(), Some(1), "no answer to hand back");
    went_right(&amx, id);
}

#[test]
fn stop_under_a_permission_card_ends_the_turn_by_signal_before_the_pane() {
    let amx = Harness::new();
    let id = "fix-login-a1b";
    start(&amx, id, "asks-permission", TASK);
    let session = minted(&amx, id);
    let agent = until_read(&amx, id, "waiting");
    assert_eq!(agent["evidence"], "hooks", "{agent}");

    let out = amx.amx(&["stop", id]);
    let why = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "amx stop: {why}");
    assert!(
        !why.contains("did not end"),
        "the turn ended when asked: {why}"
    );
    assert!(!why.contains(&session), "{why}");

    // The plugin reported the turn's end, in answer to the signal, before the
    // stop.
    let kinds = amx.event_kinds(id);
    let ended = kinds
        .iter()
        .position(|kind| kind == "session.execution.ended")
        .unwrap_or_else(|| panic!("the turn ended: {kinds:?}"));
    let asked = kinds
        .iter()
        .position(|kind| kind == "permission.asked")
        .expect("the card was reported");
    assert!(asked < ended, "{kinds:?}");
    assert_eq!(amx.state(id)["state"], "stopped");
}

#[test]
fn an_opencode_asking_a_question_is_waiting_on_it_and_the_card_shows_it() {
    let amx = Harness::new();
    let id = "fix-login-a1b";
    start(&amx, id, "asks-a-question", TASK);

    let agent = amx.until("the question on the record", || {
        let agent = status(&amx, id);
        (agent["question"] == json!("Tea or coffee?")).then_some(agent)
    });
    assert_eq!(agent["state"], "waiting", "{agent}");
    assert_eq!(agent["evidence"], "hooks", "the form said so: {agent}");

    let out = amx.amx(&["send", id, "and now the linter"]);
    assert_eq!(
        out.status.code(),
        Some(2),
        "a message typed at a question would answer it: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let view = amx.in_a_terminal(&[], &[]);
    let card = common::card_on(&amx, &view, id);
    for said in ["Tea or coffee?", "Tea", "Coffee"] {
        assert!(card.contains(said), "{said} on the card:\n{card}");
    }
}

#[test]
fn a_message_ending_on_a_popup_word_is_typed_with_a_space_after_it() {
    let amx = Harness::new();
    let id = "fix-login-a1b";
    start(&amx, id, "takes-a-message", TASK);
    amx.until("the first turn to end", || {
        (status(&amx, id)["result"] == json!("the tests pass now")).then_some(())
    });

    let out = amx.amx(&["send", id, "and now @README"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "opencode took the message: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(said(&amx, id, "typed: "), "'and now @README '");

    let out = amx.amx(&["result", id, "--timeout", "30"]);
    assert!(
        out.status.success(),
        "amx result: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "the linter is clean",
        "the turn the message started, not the one before it"
    );
    went_right(&amx, id);
}

#[test]
fn a_model_rides_in_the_env_on_new_and_a_resume_keeps_the_sessions_own() {
    let model = "opencode/longcat-2.5-preview-free";
    let amx = Harness::new();
    let id = "fix-login-a1b";
    start_with(&amx, id, "takes-a-turn", &["--model", model], TASK);
    assert_eq!(
        said(&amx, id, "argv: "),
        format!("'--standalone' '--prompt={TASK}'"),
        "the TUI takes no model flag"
    );
    assert_eq!(said(&amx, id, "model: "), model);
    let session = minted(&amx, id);
    until_read(&amx, id, "idle");
    amx.amx(&["stop", id, "--force"]);

    let out = amx_with_opencode(&amx, "takes-a-turn", &["resume", id, "carry on"]);
    assert!(
        out.status.success(),
        "amx resume: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        said(&amx, id, "argv: "),
        format!("'--standalone' '--session' '{session}' '--prompt=carry on'"),
        "the session on two words, `--standalone` once, and the message"
    );
    assert_eq!(said(&amx, id, "session: resumed "), session);
    assert_eq!(said(&amx, id, "model: "), "default");
    assert_eq!(said(&amx, id, "prompt: "), "'carry on'");
    let agent = until_read(&amx, id, "idle");
    assert_eq!(agent["result"], ANSWERED, "{agent}");
    assert_eq!(amx.meta(id)["session"], session, "the same conversation");
    went_right(&amx, id);
}

/// Run `amx setup opencode` or `amx uninstall`, with `OPENCODE_CONFIG_DIR`
/// set to `config_dir` if given.
fn wire(amx: &Harness, args: &[&str], config_dir: Option<&Path>) -> String {
    let mut command = amx.amx_command(args);
    if let Some(dir) = config_dir {
        command.env("OPENCODE_CONFIG_DIR", dir);
    }
    let out = command.output().expect("running amx");
    let printed = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        out.status.success(),
        "amx {args:?}: {printed}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    printed
}

#[test]
fn setup_places_the_plugin_under_the_home_and_uninstall_leaves_it_as_it_was() {
    let amx = Harness::new();
    let before = tree(amx.home());

    let printed = wire(&amx, &["setup", "opencode"], None);
    let plugin = amx.home().join(".config/opencode/plugins/amx/tui.js");
    assert!(printed.contains("tui.js"), "{printed}");
    assert_eq!(
        std::fs::read_to_string(&plugin).expect("the plugin"),
        PLUGIN
    );

    wire(&amx, &["uninstall"], None);
    assert!(!plugin.exists(), "the plugin went");
    // Directories setup made may stay; files may not.
    let after: Vec<_> = tree(amx.home())
        .into_iter()
        .filter(|(_, bytes)| bytes.is_some())
        .collect();
    assert_eq!(after, before, "nothing of amx's is left");
}

#[test]
fn setup_places_the_plugin_in_the_config_dir_opencode_is_told_of() {
    let amx = Harness::new();
    let dir = amx.home().join("elsewhere");
    let theirs = dir.join("plugins/theirs/tui.js");
    std::fs::create_dir_all(theirs.parent().unwrap()).unwrap();
    std::fs::write(&theirs, "export default {}\n").unwrap();

    wire(&amx, &["setup", "opencode"], Some(&dir));
    assert!(
        !amx.home().join(".config/opencode").exists(),
        "OPENCODE_CONFIG_DIR is where opencode's config lives"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("plugins/amx/tui.js")).expect("the plugin"),
        PLUGIN
    );

    wire(&amx, &["uninstall"], Some(&dir));
    assert!(!dir.join("plugins/amx/tui.js").exists(), "the plugin went");
    assert_eq!(
        std::fs::read_to_string(&theirs).unwrap(),
        "export default {}\n",
        "theirs is kept"
    );
}
