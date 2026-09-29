//! Driving the suite against an opencode that is not opencode.
//!
//! opencode is the first entry in the table to take its task on a flag, to
//! carry its model in the pane's env, to cut a turn in two presses, and to
//! end one on a signal before its pane is taken down. Everything here is
//! about those: that a task reaches the argv as one `--prompt=` word whatever
//! it opens or ends with, that a turn moves the record by the plugin's words,
//! that `stop` ends a turn under a card before it ends the pane, and that
//! `setup` places the plugin where the TUI loads one.
//!
//! The vendor is `tests/mock_opencode/opencode`, reached through the PATH,
//! replaying scenarios beside it on the screens captured off opencode 2.0.16
//! in `tests/opencode/screens`. It reports the way amx's plugin does, `amx
//! _hook` in the pane's own env, so an `amx` of this build is put on the same
//! PATH.

mod common;

use common::{AMX, Harness};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// The task every agent here is started on.
const TASK: &str = "fix the login bug";

/// What `takes-a-turn` answers, as its Ended and its message list carry it.
const ANSWERED: &str = "done";

/// The plugin amx places, as it ships.
const PLUGIN: &str = include_str!("../assets/opencode/tui.js");

/// Where the stand-in and its scenarios live.
fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/mock_opencode")
}

fn scenario(name: &str) -> PathBuf {
    fixtures()
        .join("scenarios")
        .join(format!("{name}.scenario"))
}

/// A PATH with the stand-in in front of it, and this build of amx under the
/// name the plugin runs it by.
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

/// Run amx with opencode on its PATH and the stand-in ready to play a
/// scenario. Both ride the environment, which is what a spawn hands its pane.
fn amx_with_opencode(amx: &Harness, scenario_name: &str, args: &[&str]) -> std::process::Output {
    amx.amx_command(args)
        .env("PATH", path_to_opencode(amx))
        .env("MOCK_OPENCODE_SCENARIO", scenario(scenario_name))
        .output()
        .expect("running amx")
}

/// Start an agent on opencode on `task`, with `more` of amx's own flags. A
/// task opening with `-` is one amx's own command line would read as a flag,
/// so it goes in a brief file instead, the way a person would hand it over.
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

/// What amx makes of one agent, as a caller reads it.
fn status(amx: &Harness, id: &str) -> Value {
    let out = amx.amx(&["status", id, "--json"]);
    assert!(
        out.status.success(),
        "amx status: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("the status is json")
}

/// Wait until `amx status` reads the agent as `want`.
fn until_read(amx: &Harness, id: &str, want: &str) -> Value {
    amx.until(&format!("{id} to read {want}"), || {
        let agent = status(amx, id);
        (agent["state"] == want).then_some(agent)
    })
}

/// Everything the stand-in has said in this pane, history included.
fn said_in(amx: &Harness, pane: &str) -> String {
    amx.tmux(&["capture-pane", "-p", "-J", "-S", "-", "-t", pane])
}

/// The rest of the line the stand-in opened with `opening`, once it has.
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

/// The session the stand-in minted, as it said it.
fn minted(amx: &Harness, id: &str) -> String {
    let session = said(amx, id, "session: started ");
    assert!(session.starts_with("ses_"), "opencode's own id: {session}");
    session
}

/// Nothing went wrong in the stand-in, and it is still running.
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
    // The TUI never reads a word opening with `-` as a flag's value, so a
    // `--prompt` split from its task would be a prompt with nothing in it.
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

    // The session is the one the first session route named, and the
    // transcript the message list the plugin wrote beside the record.
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

    // The conversation is read off the message list.
    let out = amx.amx(&["logs", id]);
    let printed = String::from_utf8_lossy(&out.stdout);
    assert!(printed.contains("sleep 40"), "{printed}");
    went_right(&amx, id);
}

#[test]
fn an_interrupt_presses_escape_twice_and_the_plugin_ends_the_turn() {
    // The first Escape only arms opencode's cancel; the stand-in holds the
    // turn open until a second one comes.
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

    // The plugin's Ended came in answer to the signal, ahead of the stop.
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

/// Every path under `dir` with what each file holds, sorted.
fn tree(dir: &Path) -> Vec<(PathBuf, Option<Vec<u8>>)> {
    let (mut found, mut left) = (Vec::new(), vec![dir.to_path_buf()]);
    while let Some(here) = left.pop() {
        for entry in std::fs::read_dir(&here).into_iter().flatten().flatten() {
            let path = entry.path();
            let bytes = match path.is_dir() && !path.is_symlink() {
                true => {
                    left.push(path.clone());
                    None
                }
                false => std::fs::read(&path).ok(),
            };
            found.push((path, bytes));
        }
    }
    found.sort();
    found
}

/// `amx setup opencode` or `amx uninstall`, with opencode's config dir where
/// `config_dir` says, or under this harness's home where it says nothing.
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
    // The directories setup made stay, as codex's home does; no file does.
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
