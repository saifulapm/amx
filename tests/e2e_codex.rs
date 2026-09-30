//! codex support, driven through mock codex.
//!
//! codex resumes and forks by subcommand, mints its session id at the first
//! turn, and reads amx's hooks from a hooks.json it shares with the person.
//! These tests cover the task landing after `--`, the record moving on
//! codex's hooks, turns with no Stop being closed from the pane, and a record
//! with no session refusing resume and fork.
//!
//! `tests/mock_codex/codex` is found on PATH and replays scenarios over the
//! screens captured from codex 0.157.1 in `tests/codex/screens`. It runs
//! `amx _hook` through a shell with the payload on stdin, as codex does, so
//! this build of amx is put on PATH too. It exits if a handler prints
//! anything, because codex would pass that output to the model.

mod common;

use common::{AMX, Harness, said_in, status, tree, until_read};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

const TASK: &str = "fix the login bug";

/// The answer in `takes-a-turn`'s Stop payload and rollout.
const ANSWERED: &str = "I moved the timeout into the config, and the tests pass.";

/// `rules::SETTLED_LOOKS`: how long a screen must hold still before a
/// quiescent rule may end a running turn. Tests age the record by this much
/// instead of waiting.
const SETTLED: u64 = 30;

/// The mock codex directory: the stand-in and its scenarios.
fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/mock_codex")
}

fn scenario(name: &str) -> PathBuf {
    fixtures()
        .join("scenarios")
        .join(format!("{name}.scenario"))
}

/// PATH with mock codex and this build of amx (as `amx`, the name hooks.json
/// runs) in front.
fn path_to_codex(amx: &Harness) -> String {
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

/// Run amx with mock codex on PATH, playing `scenario_name`.
///
/// Both go in the environment, which a spawn passes on to its pane.
fn amx_with_codex(amx: &Harness, scenario_name: &str, args: &[&str]) -> std::process::Output {
    amx.amx_command(args)
        .env("PATH", path_to_codex(amx))
        .env("MOCK_CODEX_SCENARIO", scenario(scenario_name))
        .env_remove("CODEX_SESSION_ID")
        .output()
        .expect("running amx")
}

/// Start an agent on codex with `amx new`, with or without a task.
fn start(amx: &Harness, id: &str, scenario_name: &str, task: Option<&str>) {
    let dir = amx.home().to_string_lossy().into_owned();
    let mut args = vec!["new", "--name", id, "--dir", &dir, "--agent", "codex"];
    args.extend(task);
    let out = amx_with_codex(amx, scenario_name, &args);
    assert!(
        out.status.success(),
        "amx new: {}",
        String::from_utf8_lossy(&out.stderr)
    );
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
    assert_eq!(session.len(), 36, "a uuid: {session}");
    session
}

/// The rollout file for `session` under this harness's home.
fn rollout(amx: &Harness, session: &str) -> PathBuf {
    amx.home()
        .join(".codex/sessions")
        .join(format!("rollout-{session}.jsonl"))
}

/// Assert the stand-in reported no error and is still running.
fn heard_nothing_back(amx: &Harness, id: &str) {
    let pane = amx.pane_of(id);
    let history = said_in(amx, &pane);
    assert!(
        !history.contains("mock codex:"),
        "the stand-in said something went wrong:\n{history}"
    );
    assert!(amx.pane_alive(&pane), "the stand-in is still running");
}

#[test]
fn new_hands_codex_a_subcommand_word_as_the_prompt_after_its_options_end() {
    // Without `--`, codex's clap parses a task like `resume` as its
    // subcommand and opens the session picker.
    let amx = Harness::new();
    let id = "resume-a1b";
    start(&amx, id, "takes-a-turn", Some("resume"));

    assert_eq!(said(&amx, id, "argv: "), "--no-daemon -- resume");
    assert_eq!(said(&amx, id, "prompt: "), "resume");
    minted(&amx, id);
}

#[test]
fn a_codex_turn_moves_the_record_by_its_hooks_and_answers_on_its_stop() {
    let amx = Harness::new();
    let id = "fix-login-a1b";
    start(&amx, id, "takes-a-turn", Some(TASK));
    assert_eq!(said(&amx, id, "argv: "), format!("--no-daemon -- {TASK}"));
    let session = minted(&amx, id);

    let agent = until_read(&amx, id, "idle");
    assert_eq!(agent["evidence"], "hooks", "codex's own word: {agent}");
    assert_eq!(agent["result"], ANSWERED, "{agent}");

    // SessionStart names the session and rollout at the first turn.
    let meta = amx.meta(id);
    assert_eq!(meta["session"], session, "{meta}");
    assert_eq!(meta["transcript"], json!(rollout(&amx, &session)), "{meta}");
    let kinds = amx.event_kinds(id);
    for word in ["SessionStart", "UserPromptSubmit", "PreToolUse", "Stop"] {
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
    assert!(printed.contains(TASK), "{printed}");
    assert!(printed.contains(ANSWERED), "{printed}");

    // Four handlers ran and `amx _hook` printed nothing.
    heard_nothing_back(&amx, id);
}

/// Age a working record until a quiescent rule may end it, and answer with
/// the status read after.
///
/// First the record is made an hour stale, then the screen's first-seen time
/// is moved [`SETTLED`] seconds back.
fn held_still(amx: &Harness, id: &str) -> Value {
    let mut state = amx.state(id);
    state["since"] = json!(1);
    state["last_event"] = json!(1);
    amx.set_state(id, state);

    let agent = status(amx, id);
    assert_eq!(
        agent["state"], "working",
        "a screen amx has only just laid eyes on ends no turn: {agent}"
    );
    let mut state = amx.state(id);
    let since = state["still"]["since"]
        .as_u64()
        .unwrap_or_else(|| panic!("the look wrote down what it saw: {state}"));
    state["still"]["since"] = json!(since - SETTLED);
    amx.set_state(id, state);
    status(amx, id)
}

/// Waits until the scenario's tool call is on the record. Reading `working`
/// is not enough: under load the PreToolUse hook can land after the Esc, and
/// the reader then trusts that fresh call over the interrupted screen.
fn until_calling(amx: &Harness, id: &str) {
    amx.until(&format!("{id} to record its tool call"), || {
        amx.event_kinds(id)
            .iter()
            .any(|kind| kind == "PreToolUse")
            .then_some(())
    });
}

#[test]
fn an_esc_amx_types_ends_a_codex_turn_that_sends_no_stop() {
    let amx = Harness::new();
    let id = "fix-login-a1b";
    start(&amx, id, "is-interrupted", Some(TASK));
    until_calling(&amx, id);

    let out = amx.amx(&["interrupt", id]);
    assert!(
        out.status.success(),
        "amx interrupt: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    amx.until_shown(id, "Conversation interrupted");

    let agent = until_read(&amx, id, "idle");
    assert_eq!(agent["evidence"], "screen", "{agent}");
    assert_eq!(agent["rule"], "interrupted", "{agent}");
    assert!(
        !amx.event_kinds(id).iter().any(|kind| kind == "Stop"),
        "codex said nothing: the pane did"
    );

    let out = amx.amx(&["result", id, "--timeout", "5"]);
    assert_eq!(out.status.code(), Some(1), "no answer to hand back");
    let why = String::from_utf8_lossy(&out.stderr);
    assert!(why.contains("interrupted"), "{why}");
}

#[test]
fn an_esc_somebody_types_at_codex_is_read_off_the_pane_once_it_holds_still() {
    let amx = Harness::new();
    let id = "fix-login-a1b";
    start(&amx, id, "is-interrupted", Some(TASK));
    until_calling(&amx, id);

    amx.tmux(&["send-keys", "-t", &amx.pane_of(id), "Escape"]);
    amx.until_shown(id, "Conversation interrupted");

    let agent = held_still(&amx, id);
    assert_eq!(agent["state"], "idle", "{agent}");
    assert_eq!(agent["evidence"], "screen", "{agent}");
    assert_eq!(agent["rule"], "interrupted", "{agent}");

    let out = amx.amx(&["result", id, "--timeout", "5"]);
    assert_eq!(out.status.code(), Some(1), "no answer to hand back");
    let why = String::from_utf8_lossy(&out.stderr);
    assert!(why.contains("aborted"), "the rollout's turn_aborted: {why}");
}

#[test]
fn a_codex_turn_the_provider_failed_is_read_off_the_pane_once_it_holds_still() {
    let amx = Harness::new();
    let id = "fix-login-a1b";
    start(&amx, id, "fails-on-the-provider", Some(TASK));
    amx.until_shown(id, "invalid_request_error");
    assert_eq!(amx.state(id)["state"], "working", "no Stop came");

    let agent = held_still(&amx, id);
    assert_eq!(agent["state"], "idle", "{agent}");
    assert_eq!(agent["evidence"], "screen", "{agent}");
    assert_eq!(
        agent["rule"], "prompt",
        "the composer under the error: {agent}"
    );

    let out = amx.amx(&["result", id, "--timeout", "5"]);
    assert_eq!(out.status.code(), Some(1), "no answer to hand back");
    let why = String::from_utf8_lossy(&out.stderr);
    assert!(
        why.contains("the provider failed: The 'no-such-model-xyz' model is not supported"),
        "the error the rollout carries: {why}"
    );
}

#[test]
fn a_codex_asking_a_question_is_waiting_on_it_and_the_card_shows_it() {
    let amx = Harness::new();
    let id = "fix-login-a1b";
    start(&amx, id, "asks-a-question", Some(TASK));

    let agent = amx.until("the question on the record", || {
        let agent = status(&amx, id);
        (agent["question"] == json!("Tea or coffee?")).then_some(agent)
    });
    assert_eq!(agent["state"], "waiting", "{agent}");
    assert_eq!(
        agent["evidence"], "hooks",
        "request_user_input said so: {agent}"
    );

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
fn a_message_sent_to_codex_is_confirmed_by_its_prompt_and_answered_by_its_stop() {
    let amx = Harness::new();
    let id = "fix-login-a1b";
    start(&amx, id, "takes-a-message", Some(TASK));
    amx.until("the first turn to end", || {
        (status(&amx, id)["result"] == json!("the tests pass now")).then_some(())
    });

    let out = amx.amx(&["send", id, "and now the linter"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "codex took the message: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        said(&amx, id, "typed: "),
        "and now the linter",
        "a paste and Enter, with the brackets taken off"
    );

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
    heard_nothing_back(&amx, id);
}

#[test]
fn resume_carries_codex_on_with_the_message_after_its_options_end() {
    let amx = Harness::new();
    let id = "fix-login-a1b";
    start(&amx, id, "takes-a-turn", Some(TASK));
    let session = minted(&amx, id);
    until_read(&amx, id, "idle");
    amx.amx(&["stop", id, "--force"]);

    // `--last` is a codex resume flag, so the message must follow `--`.
    let out = amx_with_codex(&amx, "takes-a-turn", &["resume", id, "--", "--last"]);
    assert!(
        out.status.success(),
        "amx resume: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        said(&amx, id, "argv: "),
        format!("resume {session} --no-daemon -- --last"),
        "the subcommand and the session right after the program, and the \
         task asked for once"
    );
    assert_eq!(said(&amx, id, "session: resumed "), session);
    assert_eq!(said(&amx, id, "prompt: "), "--last");
    until_read(&amx, id, "idle");
    assert_eq!(amx.meta(id)["session"], session, "the same conversation");
}

#[test]
fn fork_copies_codex_with_the_prompt_after_its_options_end() {
    let amx = Harness::new();
    let id = "fix-login-a1b";
    start(&amx, id, "takes-a-turn", Some(TASK));
    let session = minted(&amx, id);
    until_read(&amx, id, "idle");

    let out = amx_with_codex(&amx, "takes-a-turn", &["fork", id, "resume"]);
    assert!(
        out.status.success(),
        "amx fork: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let copy = String::from_utf8_lossy(&out.stdout).trim().to_string();
    assert_ne!(copy, id);

    assert_eq!(
        said(&amx, &copy, "argv: "),
        format!("fork {session} --no-daemon -- resume")
    );
    let forked = said(&amx, &copy, "session: forked ");
    let branched = forked
        .strip_suffix(&format!(" from {session}"))
        .unwrap_or_else(|| panic!("forked from the origin: {forked}"))
        .to_string();
    assert_eq!(said(&amx, &copy, "prompt: "), "resume");

    amx.until("the copy's session", || {
        (amx.meta(&copy)["session"] == json!(branched)).then_some(())
    });
    assert_eq!(amx.meta(id)["session"], session, "the origin keeps its own");
    assert!(rollout(&amx, &branched).exists());
}

#[test]
fn a_codex_that_never_took_a_turn_has_no_session_to_resume_or_fork() {
    // SessionStart fires at the first turn, so a codex stopped at its folder
    // trust screen has reported no session.
    let amx = Harness::new();
    let id = "untrusted-a1b";
    start(&amx, id, "stops-on-trust", Some(TASK));
    amx.until_shown(id, "Trust and continue");
    // Age the record so the reader looks at the pane now instead of after
    // the freshness window.
    let mut state = amx.state(id);
    state["since"] = json!(1);
    state["last_event"] = json!(1);
    amx.set_state(id, state);
    let agent = until_read(&amx, id, "waiting");
    assert_eq!(agent["rule"], "folder_trust", "{agent}");
    assert_eq!(amx.meta(id)["session"], Value::Null);

    let out = amx_with_codex(&amx, "takes-a-turn", &["fork", id]);
    assert!(!out.status.success(), "a fork of nothing");
    let why = String::from_utf8_lossy(&out.stderr);
    assert!(why.contains("no session was recorded"), "{why}");

    amx.amx(&["stop", id, "--force"]);
    let out = amx_with_codex(&amx, "takes-a-turn", &["resume", id]);
    assert!(!out.status.success(), "a resume of nothing");
    let why = String::from_utf8_lossy(&out.stderr);
    assert!(why.contains("no session was recorded"), "{why}");
}

/// A codex started outside amx, in a pane amx did not open. Answers with the
/// pane once it shows `up`.
///
/// tmux reports a pane's command by the name its process was started as, so
/// the shell running the stand-in is started through a symlink called
/// `codex`. The pane has amx on PATH and this harness's state and home, but
/// no agent id.
fn a_codex_started_by_hand(amx: &Harness, scenario_name: &str, up: &str) -> String {
    let named = amx.home().join("codex");
    std::os::unix::fs::symlink("/bin/sh", &named).expect("a shell called codex");
    let state = amx.state_root();
    let state = state.parent().expect("the state directory");
    let pane = amx.tmux(&[
        "new-session",
        "-d",
        "-c",
        &amx.home().to_string_lossy(),
        "-P",
        "-F",
        "#{pane_id}",
        "--",
        "env",
        "-u",
        "AMX_ID",
        "-u",
        "AMX_DIR",
        &format!("MOCK_CODEX_SCENARIO={}", scenario(scenario_name).display()),
        &format!("PATH={}", path_to_codex(amx)),
        &format!("AMX_STATE_DIR={}", state.display()),
        &format!("HOME={}", amx.home().display()),
        &named.to_string_lossy(),
        &fixtures().join("codex").to_string_lossy(),
        "--no-daemon",
        "--",
        TASK,
    ]);
    amx.until("the screen codex stops on", || {
        amx.capture(&pane).contains(up).then_some(())
    });
    pane
}

#[test]
fn adopt_takes_over_a_codex_by_the_session_its_tool_shell_names() {
    let amx = Harness::new();
    // The screen codex draws once its first turn is over: a message sent
    // during that turn would be answered by it.
    let pane = a_codex_started_by_hand(&amx, "takes-a-message", "steered");
    let session = amx.until("the session codex minted", || {
        said_in(&amx, &pane)
            .lines()
            .find_map(|line| line.strip_prefix("session: started "))
            .map(str::to_string)
    });

    // Run `amx adopt` as codex's shell tool would: CODEX_SESSION_ID holds the
    // root session id.
    let id = "their-codex-a1b";
    let out = amx
        .amx_command(&["adopt", "--name", id, "--task", TASK])
        .env_remove("CLAUDE_CODE_SESSION_ID")
        .env_remove("PI_SESSION_ID")
        .env("CODEX_SESSION_ID", &session)
        .env("TMUX_PANE", &pane)
        .output()
        .expect("running amx adopt");
    assert!(
        out.status.success(),
        "amx adopt: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(amx.meta(id)["agent"], "codex");
    assert_eq!(amx.meta(id)["session"], session);

    // The hooks carry no AMX_ID and reach the record by session id.
    let out = amx.amx(&["send", id, "and now the linter"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "codex took the message: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let out = amx.amx(&["result", id, "--timeout", "30"]);
    assert!(
        out.status.success(),
        "amx result: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "the linter is clean"
    );
}

#[test]
fn a_model_only_codex_lists_starts_codex_and_a_hidden_one_is_refused() {
    let amx = Harness::new();
    let dir = amx.home().to_string_lossy().into_owned();
    let new = |id: &str, model: &str| {
        amx_with_codex(
            &amx,
            "takes-a-turn",
            &["new", "--name", id, "--dir", &dir, "--model", model, TASK],
        )
    };

    let out = new("listed-a1b", "gpt-6-luna");
    assert!(
        out.status.success(),
        "amx new: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(amx.meta("listed-a1b")["agent"], "codex");
    assert_eq!(
        said(&amx, "listed-a1b", "argv: "),
        format!("--no-daemon --model gpt-6-luna -- {TASK}")
    );

    let out = new("hidden-b2c", "codex-auto-review");
    assert!(!out.status.success(), "a model codex does not list");
    let why = String::from_utf8_lossy(&out.stderr);
    assert!(why.contains("codex debug models"), "{why}");
}

/// Run `amx setup codex` or `amx uninstall`, with `CODEX_HOME` set to
/// `codex_home` if given.
fn wire(amx: &Harness, args: &[&str], codex_home: Option<&Path>) -> String {
    let mut command = amx.amx_command(args);
    if let Some(dir) = codex_home {
        command.env("CODEX_HOME", dir);
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

/// The trusted hashes of amx's four handlers, as codex 0.157.1's app-server
/// lists them (docs/codex-screens.md, "Hook trust: the oracle").
const TRUSTED: [(&str, &str, &str); 4] = [
    (
        "SessionStart",
        "session_start",
        "sha256:4add63b3f2cf92a907d37f2e9b7b5d7972234d4357db8dd35851c32c02603c53",
    ),
    (
        "UserPromptSubmit",
        "user_prompt_submit",
        "sha256:ddebddeb874f57494d55880d6f70adbee7136bb60988e2a9616f5d59ab304738",
    ),
    (
        "PreToolUse",
        "pre_tool_use",
        "sha256:62ca2d9301c902140298733d9e0be308345dda945b627cb0385a231adebdc012",
    ),
    (
        "Stop",
        "stop",
        "sha256:4de4283a553cfcda6ea020eec3ae1d66555d7f3bce089906e6723de26975e6d7",
    ),
];

/// Assert codex's files under `dir` hold amx's handler for every event, at
/// index `at(event)`, and trust each.
fn wired_in(dir: &Path, at: impl Fn(&str) -> usize) {
    let hooks: Value = serde_json::from_str(
        &std::fs::read_to_string(dir.join("hooks.json")).expect("the hooks file"),
    )
    .expect("the hooks file is json");
    let config: toml::Table = std::fs::read_to_string(dir.join("config.toml"))
        .expect("the config")
        .parse()
        .expect("the config is toml");
    let real = dir.canonicalize().expect("codex's home").join("hooks.json");
    for (event, snake, hash) in TRUSTED {
        let index = at(event);
        assert_eq!(
            hooks["hooks"][event][index],
            json!({ "hooks": [{ "type": "command", "command": "amx _hook" }] }),
            "{event}: {hooks}"
        );
        let key = format!("{}:{snake}:{index}:0", real.display());
        assert_eq!(
            config["hooks"]["state"][&key]["trusted_hash"].as_str(),
            Some(hash),
            "{key}: {config}"
        );
    }
}

#[test]
fn setup_wires_codex_under_the_home_and_uninstall_leaves_it_as_it_was() {
    let amx = Harness::new();
    let before = tree(amx.home());

    let printed = wire(&amx, &["setup", "codex"], None);
    let codex = amx.home().join(".codex");
    assert!(printed.contains("hooks.json"), "{printed}");
    wired_in(&codex, |_| 0);

    wire(&amx, &["uninstall"], None);
    let after: Vec<_> = tree(amx.home())
        .into_iter()
        .filter(|(path, _)| path != &codex)
        .collect();
    assert_eq!(after, before, "nothing of amx's is left");
}

#[test]
fn setup_wires_codex_beside_their_hooks_and_uninstall_puts_both_files_back() {
    let amx = Harness::new();
    let codex = amx.home().join("elsewhere");
    std::fs::create_dir_all(&codex).unwrap();
    let theirs = "{\n  \"hooks\": {\n    \"Stop\": [\n      { \"hooks\": [{ \"type\": \"command\", \"command\": \"notify-send done\" }] }\n    ]\n  }\n}\n";
    let config = "model = \"gpt-6-luna\"\n\n[projects.\"/srv/work\"]\ntrust_level = \"trusted\"\n";
    std::fs::write(codex.join("hooks.json"), theirs).unwrap();
    std::fs::write(codex.join("config.toml"), config).unwrap();

    wire(&amx, &["setup", "codex"], Some(&codex));
    assert!(
        !amx.home().join(".codex").exists(),
        "CODEX_HOME is where codex lives"
    );
    wired_in(&codex, |event| usize::from(event == "Stop"));
    let hooks = std::fs::read_to_string(codex.join("hooks.json")).unwrap();
    assert!(
        hooks.contains("notify-send done"),
        "theirs is kept: {hooks}"
    );
    let written = std::fs::read_to_string(codex.join("config.toml")).unwrap();
    assert!(written.starts_with(config), "theirs is kept: {written}");

    wire(&amx, &["uninstall"], Some(&codex));
    assert_eq!(
        std::fs::read_to_string(codex.join("hooks.json")).unwrap(),
        theirs
    );
    assert_eq!(
        std::fs::read_to_string(codex.join("config.toml")).unwrap(),
        config
    );
    // setup's backups stay behind.
    for (path, bytes) in tree(&codex) {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        match name.split_once(".amx-backup-") {
            None => assert!(name == "hooks.json" || name == "config.toml", "{name}"),
            Some((file, _)) => assert_eq!(
                bytes,
                std::fs::read(codex.join(file)).ok(),
                "{name} is the file as it was"
            ),
        }
    }
}
