//! Driving the suite against a codex that is not codex.
//!
//! codex is the first entry in the table to resume and fork by subcommand, to
//! mint its own session ids and say them only at the first turn, and to run
//! the hooks amx merges into a file it shares with the person. Everything here
//! is about those: that a task reaches the argv after the `--` that ends
//! codex's options, that a turn moves the record by codex's own word, that
//! the turns codex sends no Stop for are closed off the pane, and that a
//! record which never heard a session refuses to carry one on.
//!
//! The vendor is `tests/mock_codex/codex`, reached through the PATH, replaying
//! scenarios beside it on the screens captured off codex 0.157.1 in
//! `tests/codex/screens`. It runs its hooks the way codex runs the handlers
//! amx's hooks.json names — `amx _hook`, through a shell, the payload on
//! stdin — so an `amx` of this build is put on the same PATH, and it ends
//! itself if a handler prints anything, because codex hands that to the
//! model.

mod common;

use common::{AMX, Harness, said_in, status, tree, until_read};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// The task every agent here is started on.
const TASK: &str = "fix the login bug";

/// What `takes-a-turn` answers, as its Stop and its rollout carry it.
const ANSWERED: &str = "I moved the timeout into the config, and the tests pass.";

/// How long a screen must hold still before a quiescent rule may end a turn
/// on the record as running: `rules::SETTLED_LOOKS` seconds. Spelled here, as
/// tests/e2e_pi.rs spells it, because the clock is aged rather than waited.
const SETTLED: u64 = 30;

/// Where the stand-in and its scenarios live.
fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/mock_codex")
}

fn scenario(name: &str) -> PathBuf {
    fixtures()
        .join("scenarios")
        .join(format!("{name}.scenario"))
}

/// A PATH with the stand-in in front of it, and this build of amx under the
/// name codex's hooks.json runs it by.
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

/// Run amx with codex on its PATH and the stand-in ready to play a scenario.
/// Both ride the environment, which is what a spawn hands its pane.
fn amx_with_codex(amx: &Harness, scenario_name: &str, args: &[&str]) -> std::process::Output {
    amx.amx_command(args)
        .env("PATH", path_to_codex(amx))
        .env("MOCK_CODEX_SCENARIO", scenario(scenario_name))
        .env_remove("CODEX_SESSION_ID")
        .output()
        .expect("running amx")
}

/// Start an agent on codex, the way a person starts one, on `task` or on none.
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
    assert_eq!(session.len(), 36, "a uuid: {session}");
    session
}

/// The rollout codex keeps a session in, under this harness's home.
fn rollout(amx: &Harness, session: &str) -> PathBuf {
    amx.home()
        .join(".codex/sessions")
        .join(format!("rollout-{session}.jsonl"))
}

/// Nothing a hook printed ended the stand-in, and it is still running.
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
    // clap reads a message that is one of codex's subcommands as that
    // subcommand, until `--`: `codex resume` would open the session picker.
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

    // The session and the rollout are the ones SessionStart named at the
    // first turn.
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

    // Four handlers ran, and `amx _hook` printed nothing for any of them.
    heard_nothing_back(&amx, id);
}

/// Age a record codex's hooks left working until a quiescent rule may end it:
/// nothing heard for an hour, then the screen on the pane first seen
/// [`SETTLED`] seconds ago, the way tests/e2e_pi.rs ages one rather than wait
/// out the clock. Answers with the reading taken after.
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

#[test]
fn an_esc_amx_types_ends_a_codex_turn_that_sends_no_stop() {
    let amx = Harness::new();
    let id = "fix-login-a1b";
    start(&amx, id, "is-interrupted", Some(TASK));
    until_read(&amx, id, "working");

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
    until_read(&amx, id, "working");

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

    // `--last` is a word codex's resume picks a session by, so the message
    // only reaches the model from behind a `--`.
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

    // codex names the copy's own session at its first turn.
    amx.until("the copy's session", || {
        (amx.meta(&copy)["session"] == json!(branched)).then_some(())
    });
    assert_eq!(amx.meta(id)["session"], session, "the origin keeps its own");
    assert!(rollout(&amx, &branched).exists());
}

#[test]
fn a_codex_that_never_took_a_turn_has_no_session_to_resume_or_fork() {
    // SessionStart fires at a session's first turn, never at launch, so a
    // codex stopped at its folder trust screen, in front of the turn its task
    // would start, has told amx of no session at all.
    let amx = Harness::new();
    let id = "untrusted-a1b";
    start(&amx, id, "stops-on-trust", Some(TASK));
    amx.until_shown(id, "Trust and continue");
    // Age the fresh record so the reader takes the pane now instead of after
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
    assert!(why.contains("no session was ever recorded"), "{why}");

    amx.amx(&["stop", id, "--force"]);
    let out = amx_with_codex(&amx, "takes-a-turn", &["resume", id]);
    assert!(!out.status.success(), "a resume of nothing");
    let why = String::from_utf8_lossy(&out.stderr);
    assert!(why.contains("no session was ever recorded"), "{why}");
}

/// A codex somebody started themselves, in a pane amx never opened, answering
/// with the pane once `up` is on it.
///
/// Started under the name that makes it codex: tmux answers for a pane with
/// the program its process was started as, so the shell reading the stand-in
/// is reached through a link called `codex`. The pane carries what a real one
/// would: amx on the PATH for the hooks, this harness's state and home, and
/// nothing naming an agent.
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
    let pane = a_codex_started_by_hand(&amx, "takes-a-message", "? for shortcuts");
    let session = amx.until("the session codex minted", || {
        said_in(&amx, &pane)
            .lines()
            .find_map(|line| line.strip_prefix("session: started "))
            .map(str::to_string)
    });

    // What `amx adopt` sees when codex's shell tool runs it: the root
    // session's id in CODEX_SESSION_ID, and the pane it is in.
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

    // Its hooks carry no AMX_ID, and still reach the record by the session
    // they name: the message is confirmed, and its turn answered.
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

/// `amx setup codex` or `amx uninstall`, with codex's home where `codex_home`
/// says, or under this harness's home where it says nothing.
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
/// listed them (docs/codex-screens.md, "Hook trust: the oracle").
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

/// Whether codex's files under `dir` hold amx's group for every event, at
/// the index `at` says, and the trust for each.
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
    // The copies setup kept before its first edit stay, as every wire's do.
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
