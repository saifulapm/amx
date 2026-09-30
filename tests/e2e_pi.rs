//! End-to-end tests for amx's pi entry, driven against the stand-in
//! `tests/mock_pi/pi`.
//!
//! pi is the first vendor with a start flag, so amx mints its session id and
//! passes it on the argv. The tests check that the id reaches the pane's argv,
//! is recorded as soon as the pane exists, is reused on resume, and sits
//! beside a new id on a fork.
//!
//! - amx keys its vendor table by the program an agent command runs, so
//!   `--agent pi` with `tests/mock_pi` at the front of `PATH` makes an agent
//!   pi's on a machine without pi.
//! - pi reports through an extension amx installs, one `amx _hook` per event,
//!   and the stand-in delivers the same reports from a scenario. Screens are
//!   still read where no report comes: the gates pi draws before a turn, and
//!   a pi without the extension.
//! - The stand-in paints each screen in one write. Every wait settles on one
//!   anchor, and the rest of the screen is read from that same capture.

mod common;

use common::{AMX, Harness, check_line, ls, now, said_in, status};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The task every agent here is started with.
const TASK: &str = "fix the login bug";

/// The directory holding the stand-in and its scenarios.
fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/mock_pi")
}

fn scenario(name: &str) -> PathBuf {
    fixtures()
        .join("scenarios")
        .join(format!("{name}.scenario"))
}

/// `PATH` with the stand-in's directory in front, so `pi` resolves to it.
fn path_to_pi() -> String {
    let ours = fixtures().to_string_lossy().into_owned();
    match std::env::var("PATH") {
        Ok(rest) => format!("{ours}:{rest}"),
        Err(_) => ours,
    }
}

/// Run amx with the stand-in on `PATH`, set to play `scenario_name`.
///
/// Both are passed in the environment because a spawn snapshots its
/// environment and the pane starts from that snapshot.
fn amx_with_pi(amx: &Harness, scenario_name: &str, args: &[&str]) -> std::process::Output {
    amx_playing(amx, &scenario(scenario_name), args)
}

/// [`amx_with_pi`] with a scenario at any path, such as a [`timeline`].
fn amx_playing(amx: &Harness, scenario: &Path, args: &[&str]) -> std::process::Output {
    amx.amx_command(args)
        .env("PATH", path_to_pi())
        .env("MOCK_PI_SCENARIO", scenario)
        .output()
        .expect("running amx")
}

/// Write a scenario of the test's own under home, and answer with its path.
///
/// The shared scenarios each walk to one screen and hold there. A test that
/// follows one pane through several screens writes its own sequence.
fn timeline(amx: &Harness, name: &str, steps: &str) -> PathBuf {
    let path = amx.home().join(format!("{name}.scenario"));
    std::fs::write(&path, steps).expect("writing a scenario");
    path
}

/// Run `amx new --agent pi` for `id` in home, playing `scenario_name`.
fn start(amx: &Harness, id: &str, scenario_name: &str) {
    start_playing(amx, id, &scenario(scenario_name));
}

/// [`start`] with a scenario at any path.
fn start_playing(amx: &Harness, id: &str, scenario: &Path) {
    let out = amx_playing(
        amx,
        scenario,
        &[
            "new",
            "--name",
            id,
            "--dir",
            &amx.home().to_string_lossy(),
            "--agent",
            "pi",
            TASK,
        ],
    );
    assert!(
        out.status.success(),
        "amx new: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// `PATH` with both stand-ins on it: pi, and mock-claude copied in as
/// `claude`.
///
/// A typed model picks the harness that offers it, so testing that choice
/// needs both programs on `PATH`. `new_as_claude` in tests/e2e_spawn.rs
/// installs mock-claude the same way.
fn path_to_both(amx: &Harness) -> String {
    let bin = amx.home().join("bin");
    std::fs::create_dir_all(&bin).expect("a directory for the stand-in");
    std::fs::copy(amx.mock(), bin.join("claude")).expect("the stand-in under claude's name");
    format!("{}:{}", bin.display(), path_to_pi())
}

/// A config with pi as the default agent and extra args for each harness.
///
/// pi's extra arg is `--approve`, the flag that answers its folder-trust
/// screen. No test here sets `trust`, so an `--approve` on pi's argv under
/// this config came from the `[pi]` table.
const BOTH_HARNESSES: &str = "agent = \"pi\"\n\n\
     [claude]\nargs = [\"--add-dir\", \"/srv/shared\"]\n\n\
     [pi]\nargs = [\"--approve\"]\n";

/// Run `amx new` with both stand-ins on `PATH`, each given a scenario, so
/// whichever harness is picked prints how it was called.
fn new_on_either(amx: &Harness, id: &str, dials: &[&str]) -> std::process::Output {
    let dir = amx.home().to_string_lossy().into_owned();
    let mut line = vec!["new", "--name", id, "--dir", &dir];
    line.extend_from_slice(dials);
    line.push(TASK);

    amx.amx_command(&line)
        .env("PATH", path_to_both(amx))
        .env("MOCK_PI_SCENARIO", scenario("takes-a-turn"))
        .env("MOCK_CLAUDE_SCENARIO", amx.scenario("a-dispatched-worker"))
        .output()
        .expect("running amx new")
}

/// Session ids in the terminal where `amx adopt` runs: the claude session it
/// is typed in, and a pi started from inside that claude.
const A_CLAUDE: &str = "4c1e8b73-2f60-4a15-9d38-7e2b6c0f9a54";
const THEIR_PI: &str = "9f3c1d20-5a44-4e7b-8c19-6d0a2b5f7e31";

/// Start pi by hand in a pane amx did not open, playing `scenario_name` on
/// session `session`, and answer with the pane once it shows `up`.
///
/// `up` must be a row only the final screen has, since adopt reads the pane
/// once. tmux names a pane's command after the program its process started
/// as, which for a script is its shebang shell, so the stand-in runs under a
/// `/bin/sh` symlink called `pi`. The pane gets `AMX_BIN` for the extension,
/// this harness's state and home, and no `AMX_ID` or `AMX_DIR`: a spawn sets
/// those, and the suite may itself be running inside one.
fn a_pi_started_by_hand(amx: &Harness, scenario_name: &str, session: &str, up: &str) -> String {
    let named_pi = amx.home().join("pi");
    std::os::unix::fs::symlink("/bin/sh", &named_pi).expect("a shell called pi");
    let scenario = format!("MOCK_PI_SCENARIO={}", scenario(scenario_name).display());
    let bin = format!("AMX_BIN={AMX}");
    let state = format!(
        "AMX_STATE_DIR={}",
        amx.state_root()
            .parent()
            .expect("the state directory")
            .display()
    );
    let home = format!("HOME={}", amx.home().display());
    let (named_pi, stand_in) = (
        named_pi.to_string_lossy().into_owned(),
        fixtures().join("pi").to_string_lossy().into_owned(),
    );
    let pane = amx.tmux(&[
        "new-session",
        "-d",
        "-P",
        "-F",
        "#{pane_id}",
        "--",
        "env",
        "-u",
        "AMX_ID",
        "-u",
        "AMX_DIR",
        &scenario,
        &bin,
        &state,
        &home,
        &named_pi,
        &stand_in,
        "--session-id",
        session,
    ]);

    amx.until("the screen pi stops on", || {
        amx.capture(&pane).contains(up).then_some(())
    });
    pane
}

/// Run `amx adopt` as if typed in `pane`, with the vendor session variables
/// in `named`.
///
/// The suite often runs inside a real agent, so inherited
/// `CLAUDE_CODE_SESSION_ID` and `PI_SESSION_ID` are cleared first.
fn adopt(amx: &Harness, id: &str, pane: &str, named: &[(&str, &str)]) -> std::process::Output {
    amx.amx_command(&["adopt", "--name", id, "--task", TASK])
        .env_remove("CLAUDE_CODE_SESSION_ID")
        .env_remove("PI_SESSION_ID")
        .env("TMUX_PANE", pane)
        .envs(named.iter().copied())
        .output()
        .expect("running amx adopt")
}

/// Run the stand-in directly, with `HOME` set to this harness's home.
///
/// For an argv amx never builds: `--no-session` is on pi's conflicts list, so
/// amx never mints an id beside it, but the fixture must still handle both.
fn stand_in(amx: &Harness, args: &[&str]) -> std::process::Output {
    std::process::Command::new(fixtures().join("pi"))
        .args(args)
        .env("HOME", amx.home())
        .env("MOCK_PI_SCENARIO", scenario("one-screen"))
        .output()
        .expect("running the stand-in")
}

/// Run the stand-in's `--list-models` as amx does, with no scenario set.
fn listing(amx: &Harness) -> std::process::Output {
    std::process::Command::new(fixtures().join("pi"))
        .arg("--list-models")
        .env("HOME", amx.home())
        .env_remove("MOCK_PI_SCENARIO")
        .output()
        .expect("running the stand-in")
}

/// The agent's row in `amx ls --json`, one look by a process that then
/// exits.
fn listed(amx: &Harness, id: &str) -> Value {
    ls(amx)
        .into_iter()
        .find(|row| row["id"] == id)
        .unwrap_or_else(|| panic!("a row for {id}"))
}

/// Seconds a screen must hold still before a quiescent rule may end a turn
/// the record has as running: `rules::SETTLED_LOOKS`.
const SETTLED: u64 = 30;

/// Seconds a vendor report is trusted before a reader looks at the pane:
/// `derive::FRESH`. Until then a turn reads as running after its last hook.
const FRESH: u64 = 8;

/// amx's event names for the prompt and turn-end edges a pane reading places.
///
/// Hard-coded because the event log is a contract callers read with `jq`, so
/// a rename in amx must fail these tests.
const READ_PROMPT: &str = "read.prompt";
const READ_TURN_END: &str = "read.turn-end";

/// amx's event name for a sent message, `send::SEND`, hard-coded for the same
/// reason.
const SENT: &str = "send";

/// Seconds a send waits for its message to be taken: `send::CONFIRM`.
const CONFIRM: u64 = 5;

/// The event names pi's extension reports a turn's start and end under.
const PIS_WORDS: [&str; 2] = ["agent_start", "agent_settled"];

/// A turn's answer, as pi reports it when the turn settles and as the
/// reporting scenarios write it.
const ANSWERED: &str = "I moved the timeout into the config, and the tests pass.";

/// Wait for the stand-in's line starting with `opening`, and answer with it.
fn until_said(amx: &Harness, id: &str, opening: &str) -> String {
    let pane = amx.pane_of(id);
    amx.until(&format!("the vendor to say {opening}"), || {
        said_in(amx, &pane)
            .lines()
            .find(|line| line.starts_with(opening))
            .map(str::to_string)
    })
}

/// The `argv:` line the stand-in prints.
fn argv_of(amx: &Harness, id: &str) -> String {
    until_said(amx, id, "argv:")
}

/// The `session:` line the stand-in prints: what it did with the session id.
fn session_of(amx: &Harness, id: &str) -> String {
    until_said(amx, id, "session:")
}

/// The stand-in's file for `session` under home.
fn session_file(amx: &Harness, session: &str) -> PathBuf {
    amx.home()
        .join(".pi/sessions")
        .join(format!("{session}.jsonl"))
}

/// The pane's rows, right-trimmed, with trailing blank rows dropped.
fn drawn(amx: &Harness, pane: &str) -> Vec<String> {
    let mut rows: Vec<String> = amx
        .capture(pane)
        .lines()
        .map(|row| row.trim_end().to_string())
        .collect();
    while rows.last().is_some_and(String::is_empty) {
        rows.pop();
    }
    rows
}

/// Indexes of the rows that are pi's composer border, topmost first.
///
/// A border row is 20 or more `─` and nothing else, the anchor every rule in
/// `assets/screen-rules-pi.toml` uses.
fn borders(rows: &[String]) -> Vec<usize> {
    rows.iter()
        .enumerate()
        .filter(|(_, row)| {
            let drawn = row.trim();
            drawn.chars().count() >= 20 && drawn.chars().all(|glyph| glyph == '─')
        })
        .map(|(at, _)| at)
        .collect()
}

/// Index of the first row containing `text`.
fn row_of(rows: &[String], text: &str) -> Option<usize> {
    rows.iter().position(|row| row.contains(text))
}

/// Whether a row starts with a frame of pi's status-line spinner.
///
/// The frames are ten braille glyphs cycled eight times a second. Every
/// status line carries one whatever its message, so
/// `assets/screen-rules-pi.toml` anchors its spinner rule on them.
fn spins(row: &str) -> bool {
    row.trim_start()
        .starts_with(|glyph: char| ('\u{2800}'..='\u{28ff}').contains(&glyph))
}

/// Whether a row is the composer's top border with the working indicator in
/// it, as pi 0.85.1 draws it: `── `, a spinner frame, the message, then `─`
/// to the pane's edge.
fn framed_border(row: &str) -> bool {
    row.strip_prefix("── ")
        .is_some_and(|rest| spins(rest) && row.trim_end().ends_with('─'))
}

/// pi's status lines other than `Working`: a label, the scenario that shows
/// it, its text, and whether pi 0.85.1 draws it in the composer's top border.
///
/// Compaction and retry stay on the row above the box. An extension's own
/// working message replaces `Working` and is drawn where it would be.
const OTHER_STATUS_LINES: [(&str, &str, &str, bool); 3] = [
    (
        "a compacting turn",
        "compacts-the-context",
        "Compacting context...",
        false,
    ),
    ("a retrying turn", "retries-a-turn", "Retrying (1/3)", false),
    (
        "a turn under an extension's own working message",
        "renames-the-working-line",
        "Reviewing the diff",
        true,
    ),
];

/// Two panes showing one of pi's selectors: a label, the scenario, and the
/// distance from the topmost border a rule can see to the stats line.
///
/// The selector's box is three rows in both. With a transcript above it, the
/// bottom border of `!cmd`'s box is also on the pane, and a rule that reads
/// the topmost border starts from that one.
const SELECTORS: [(&str, &str, usize); 2] = [
    ("a selector with nothing above it", "opens-a-selector", 5),
    (
        "the same selector under a transcript",
        "opens-a-selector-under-a-transcript",
        7,
    ),
];

/// Run `amx doctor --fix` with pi on `PATH` and `typed` on stdin, and answer
/// with its stdout.
fn doctor_fix(amx: &Harness, typed: &str) -> String {
    use std::io::Write;
    use std::process::Stdio;

    let mut child = amx
        .amx_command(&["doctor", "--fix"])
        .env("PATH", path_to_pi())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("running amx doctor");
    child
        .stdin
        .take()
        .expect("stdin was asked for")
        .write_all(typed.as_bytes())
        .expect("typing at amx");
    let out = child.wait_with_output().expect("waiting for amx");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Every path under `dir`, files and directories alike, sorted.
fn everything_under(dir: &Path) -> Vec<PathBuf> {
    let (mut found, mut left) = (Vec::new(), vec![dir.to_path_buf()]);
    while let Some(here) = left.pop() {
        let Ok(entries) = std::fs::read_dir(&here) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                left.push(path.clone());
            }
            found.push(path);
        }
    }
    found.sort();
    found
}

/// The rule names in `assets/screen-rules-pi.toml`, in file order.
///
/// Each `[[rule]]` table opens with its `name = "..."` line, so a line scan
/// finds them without a TOML parser.
fn rules_declared() -> Vec<String> {
    let doc = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/screen-rules-pi.toml"),
    )
    .expect("pi's screens document");
    doc.lines()
        .filter_map(|line| line.trim().strip_prefix("name = \""))
        .filter_map(|rest| rest.strip_suffix('"'))
        .map(str::to_string)
        .collect()
}

/// Every rule `docs/pi-screens.md` records a screen as reading as.
///
/// The Reads column is the last cell of each table row. A rule that claimed
/// the screen is in bold code there (`**`dialog`**`), which tells it apart
/// from other code spans in the cell.
fn rules_read() -> Vec<String> {
    let inventory =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/pi-screens.md"))
            .expect("pi's screen inventory");
    let mut found = Vec::new();
    for row in inventory.lines() {
        let cells: Vec<&str> = row.split('|').collect();
        if cells.len() < 5 || row.contains("| ---") {
            continue;
        }
        let mut rest = cells[cells.len() - 2];
        while let Some((_, after)) = rest.split_once("**`") {
            let Some((name, tail)) = after.split_once("`**") else {
                break;
            };
            if !found.contains(&name.to_string()) {
                found.push(name.to_string());
            }
            rest = tail;
        }
    }
    found
}

/// The bullets of pi's field-test section in `docs/vendors.md`, one string
/// each with its continuation lines joined.
fn field_test_findings() -> Vec<String> {
    let vendors =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/vendors.md"))
            .expect("the vendors document");
    let section = vendors
        .split("\n## What the dogfood saw\n")
        .nth(1)
        .expect("the field-test section")
        .split("\n## ")
        .next()
        .expect("everything under that heading");

    let mut found: Vec<String> = Vec::new();
    for row in section.lines() {
        match (row.strip_prefix("- **"), found.last_mut()) {
            (Some(opening), _) => found.push(opening.to_string()),
            // A continuation line, indented under its bullet.
            (None, Some(last)) if row.starts_with("  ") => {
                last.push(' ');
                last.push_str(row.trim());
            }
            _ => {}
        }
    }
    found
}

#[test]
fn the_field_test_says_what_closed_each_of_its_findings() {
    // Five verbs broke on a word pi never reports. Each bullet must say what
    // fixed it, so a reader does not have to drive pi again to find out.
    let findings = field_test_findings();
    assert_eq!(findings.len(), 5, "the findings left: {findings:#?}");
    for finding in &findings {
        assert!(
            finding.contains("Closed:"),
            "every one of them says what closed it: {finding}"
        );
    }

    // Without hooks only a pane reading places a turn's edges, so the log
    // carries amx's event names there and the doc must use them.
    let closed = findings.concat();
    for word in [READ_PROMPT, READ_TURN_END] {
        assert!(
            closed.contains(word),
            "the section names `{word}`, which is the word a reader of the log \
             finds: {closed}"
        );
    }
}

#[test]
fn the_inventory_measured_a_screen_for_every_rule_pi_has() {
    // `docs/pi-screens.md` lists every screen pi can draw and what it reads
    // as. A rule with no row there has unmeasured coverage, and a row naming a
    // removed rule is stale. The rules and their order are asserted in
    // `src/rules.rs`.
    let mut declared = rules_declared();
    let mut read = rules_read();
    assert!(!declared.is_empty(), "pi's document declares rules");
    declared.sort();
    read.sort();
    assert_eq!(
        read, declared,
        "docs/pi-screens.md's Reads column and assets/screen-rules-pi.toml's \
         rules are the same set, or one of them was written without the other"
    );
}

#[test]
fn spawn_hands_pi_the_start_flag_and_the_id_amx_minted_for_the_agent() {
    // pi is the first vendor with a start flag, so this is the check that the
    // flag and id reach a real pane, beyond the argv unit tests build.
    let amx = Harness::new();
    let id = "fix-login-a1b";
    start(&amx, id, "takes-a-turn");

    let called = argv_of(&amx, id);
    assert!(
        called.contains(&format!("--session-id {id}")),
        "the flag and the agent's own id, two words the way pi spells them: {called}"
    );
    assert!(
        called.ends_with(TASK),
        "and the task is still the last word: {called}"
    );
    assert_eq!(
        amx.meta(id)["session"],
        id,
        "recorded at the moment it spawned, because no hook is coming to say it"
    );
}

#[test]
fn spawn_hands_pi_a_task_opening_with_an_at_sign_as_words() {
    // In pi 0.87.1 `--` ends flags but not file arguments: a task starting
    // with `@` still names a file, and a missing file makes pi exit before
    // its session opens.
    let amx = Harness::new();
    let id = "at-sign-a1b";
    let out = amx_with_pi(
        &amx,
        "takes-a-turn",
        &[
            "new",
            "--name",
            id,
            "--dir",
            &amx.home().to_string_lossy(),
            "--agent",
            "pi",
            "@alice asked for this",
        ],
    );
    assert!(
        out.status.success(),
        "amx new: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let opened = session_of(&amx, id);
    assert!(
        opened.contains(id),
        "pi read the task as words and went on to open its session: {opened}"
    );
}

#[test]
fn spawn_asks_pi_to_create_the_session_when_there_is_no_file_under_that_id() {
    // `--session-id` opens the session if it exists and creates it otherwise.
    // Nothing exists yet under a fresh id.
    let amx = Harness::new();
    let id = "fix-login-a1b";
    start(&amx, id, "takes-a-turn");

    assert_eq!(session_of(&amx, id), format!("session: created {id}"));
    assert!(
        session_file(&amx, id).exists(),
        "and the conversation is on disk under the id amx chose"
    );
}

#[test]
fn a_spawn_told_to_keep_no_session_is_minted_no_id_to_offer_back() {
    // `--no-session` is the one flag on pi's conflicts list that pi does not
    // refuse: it runs the turn and keeps the conversation in memory. amx must
    // mint no id beside it and record no session.
    let amx = Harness::new();
    let id = "fix-login-a1b";
    let out = amx_with_pi(
        &amx,
        "takes-a-turn",
        &[
            "new",
            "--name",
            id,
            "--dir",
            &amx.home().to_string_lossy(),
            "--agent",
            "pi",
            TASK,
            "--",
            "--no-session",
        ],
    );
    assert!(
        out.status.success(),
        "amx new: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let called = argv_of(&amx, id);
    assert!(
        called.contains("--no-session"),
        "the flag reached the vendor as it was typed: {called}"
    );
    assert!(
        !called.contains("--session-id"),
        "and amx minted nothing to put beside it: {called}"
    );
    assert!(
        amx.meta(id)["session"].is_null(),
        "so the record names no conversation: {}",
        amx.meta(id)
    );
    assert!(
        !amx.home().join(".pi/sessions").exists(),
        "and none was written anywhere under the person's home"
    );
}

#[test]
fn a_pi_under_the_trust_key_is_answered_on_its_argv_and_in_nobodys_file() {
    // pi's folder-trust screen is answered by `--approve` on the argv. The
    // spawn goes into a repository so it cuts a worktree, where `trust` also
    // writes claude's `.claude.json`; that must not happen for a pi agent.
    let amx = Harness::new();
    amx.config("trust = true\n");
    let repo = amx.a_repo();
    let id = "fix-login-a1b";

    let out = amx_with_pi(
        &amx,
        "takes-a-turn",
        &[
            "new",
            "--name",
            id,
            "--dir",
            &repo.to_string_lossy(),
            "--agent",
            "pi",
            TASK,
        ],
    );
    assert!(
        out.status.success(),
        "amx new: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let called = argv_of(&amx, id);
    assert!(
        called.contains("--approve"),
        "the flag pi's own --help documents: {called}"
    );
    assert!(
        called.ends_with(TASK),
        "and the task is still the last word: {called}"
    );

    let stores: Vec<PathBuf> = everything_under(amx.home())
        .into_iter()
        .filter(|path| path.file_name().is_some_and(|name| name == ".claude.json"))
        .collect();
    assert!(
        stores.is_empty(),
        "another vendor's store, written for a pi agent: {stores:?}"
    );
}

#[test]
fn a_pi_spawned_without_the_trust_key_is_left_to_answer_its_own_screen() {
    // `trust` is the only consent for `--approve`. Without it, pi asks the
    // person at the keyboard whether to trust the directory.
    let amx = Harness::new();
    let id = "fix-login-a1b";
    start(&amx, id, "takes-a-turn");

    let called = argv_of(&amx, id);
    assert!(!called.contains("--approve"), "{called}");
}

#[test]
fn a_model_only_claude_offers_starts_claude_under_an_amx_configured_for_pi() {
    // No agent is named and the config says pi, but `opus` is in claude's
    // models and not in pi's listing, so claude starts with its `[claude]`
    // args.
    let amx = Harness::new();
    amx.config(BOTH_HARNESSES);
    let id = "fix-login-a1b";

    let out = new_on_either(&amx, id, &["--model", "opus"]);
    assert!(
        out.status.success(),
        "amx new: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    assert_eq!(amx.meta(id)["agent"], "claude --add-dir /srv/shared");
    let called = argv_of(&amx, id);
    assert!(
        called.contains("--add-dir /srv/shared"),
        "the words [claude] carries: {called}"
    );
    assert!(
        called.contains("--model opus"),
        "and the dial the word turned: {called}"
    );
    assert!(
        called.ends_with(TASK),
        "and the task is still the last word: {called}"
    );
}

#[test]
fn a_model_pis_own_listing_holds_starts_pi_with_the_words_its_table_carries() {
    // `gpt-5-mini` is the model half of `github-copilot/gpt-5-mini`, as a
    // person would type it. pi is asked first because the config names it.
    let amx = Harness::new();
    amx.config(BOTH_HARNESSES);
    let id = "fix-login-a1b";

    let out = new_on_either(&amx, id, &["--model", "gpt-5-mini"]);
    assert!(
        out.status.success(),
        "amx new: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    assert_eq!(amx.meta(id)["agent"], "pi --approve");
    let called = argv_of(&amx, id);
    assert!(
        called.contains("--approve"),
        "the words [pi] carries: {called}"
    );
    assert!(
        called.contains("--model gpt-5-mini"),
        "and the id as it was typed, which pi's open dial takes: {called}"
    );
    assert!(
        called.contains(&format!("--session-id {id}")),
        "and the harness is pi down to the start flag only pi declares: {called}"
    );
}

#[test]
fn a_model_neither_harness_offers_is_refused_naming_what_each_takes() {
    let amx = Harness::new();
    amx.config(BOTH_HARNESSES);
    let id = "fix-login-a1b";

    let refused = new_on_either(&amx, id, &["--model", "gpt-9"]);

    assert_eq!(
        refused.status.code(),
        Some(64),
        "a malformed command line, not a state a caller branches on"
    );
    let said = String::from_utf8_lossy(&refused.stderr);
    assert!(
        said.contains("pi lists 2 models (`pi --list-models`)"),
        "how many models pi has and what prints them: {said}"
    );
    assert!(
        said.contains("claude accepts") && said.contains("opus"),
        "and claude's own words, which amx holds itself: {said}"
    );
    assert!(
        !amx.agent_dir(id).exists(),
        "and nothing was made for a spawn that never happened"
    );
}

#[test]
fn an_agent_somebody_named_runs_the_model_typed_beside_it() {
    // With the harness named, `--model` is only pi's dial, and pi takes any
    // pattern. Looking up who offers `opus` would send this spawn to claude.
    let amx = Harness::new();
    amx.config(BOTH_HARNESSES);
    let id = "fix-login-a1b";

    let out = new_on_either(&amx, id, &["--agent", "pi", "--model", "opus"]);
    assert!(
        out.status.success(),
        "amx new: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    assert_eq!(amx.meta(id)["agent"], "pi --approve");
    let called = argv_of(&amx, id);
    assert!(called.contains("--model opus"), "{called}");
    assert!(
        called.contains("--approve"),
        "and the table's words ride however the harness was picked: {called}"
    );
}

#[test]
fn the_stand_in_prints_its_listing_before_it_looks_for_a_scenario() {
    // amx lists a harness's models before starting anything, with no scenario
    // set. A fixture that read its scenario first would print a screen, or
    // hold the spawn open for a whole timeline.
    let amx = Harness::new();

    let out = listing(&amx);

    assert!(
        out.status.success(),
        "pi prints its models and exits: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let printed = String::from_utf8_lossy(&out.stdout);
    let rows: Vec<&str> = printed.lines().collect();
    assert_eq!(rows.len(), 3, "a header and two models: {printed}");
    assert!(
        rows[0].to_lowercase().starts_with("provider"),
        "the header pi prints over them, which amx reads past: {printed}"
    );

    let models: Vec<String> = rows[1..]
        .iter()
        .map(|row| {
            let mut columns = row.split_whitespace();
            let provider = columns.next().expect("a provider");
            let model = columns.next().expect("a model");
            format!("{provider}/{model}")
        })
        .collect();
    assert_eq!(
        models,
        ["github-copilot/gpt-5-mini", "cerebras/qwen-3-coder"]
    );
}

#[test]
fn the_stand_in_parts_the_flags_pi_refuses_from_the_one_it_throws_away() {
    // Of the six flags that conflict with `--session-id`, five make pi exit
    // and `--no-session` takes the id and drops it. A fixture that exited on
    // all six would hide the real failure: a session recorded but never
    // written.
    let amx = Harness::new();
    let id = "fix-login-a1b";

    let out = stand_in(&amx, &["--no-session", "--session-id", id]);
    assert!(
        out.status.success(),
        "pi runs the turn: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(
        !said.contains("session:"),
        "and says nothing about a session: {said}"
    );
    assert!(
        !session_file(&amx, id).exists(),
        "and leaves no conversation on disk under the id it was handed"
    );

    for refusal in ["-c", "-r", "--continue", "--resume", "--session"] {
        let out = stand_in(&amx, &[refusal, "--session-id", id]);
        assert_eq!(
            out.status.code(),
            Some(2),
            "pi exits rather than take a minted id beside {refusal}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

#[test]
fn resume_brings_the_agent_back_onto_the_id_it_was_started_under() {
    // The same flag with the same id opens the existing session. This is how
    // amx resumes a vendor that reports nothing.
    let amx = Harness::new();
    let id = "fix-login-a1b";
    start(&amx, id, "takes-a-turn");
    assert_eq!(session_of(&amx, id), format!("session: created {id}"));

    amx.amx(&["stop", id, "--force"]);
    assert_eq!(amx.state(id)["state"], "stopped");

    let out = amx_with_pi(&amx, "takes-a-turn", &["resume", id]);
    assert!(
        out.status.success(),
        "amx resume: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let called = argv_of(&amx, id);
    assert!(called.contains(&format!("--session-id {id}")), "{called}");
    assert!(
        !called.contains(TASK),
        "and not the task, which was asked for once: {called}"
    );
    assert_eq!(
        session_of(&amx, id),
        format!("session: opened {id}"),
        "the file was already there, so this is the conversation carried on"
    );
    assert_eq!(
        amx.meta(id)["session"],
        id,
        "still the id it was minted with"
    );
}

#[test]
fn fork_asks_pi_for_the_origin_id_and_a_new_one_in_the_same_argv() {
    // pi names the session to copy with `--fork`, which leaves the start flag
    // free for the copy's minted id, so amx knows the fork's session.
    let amx = Harness::new();
    let id = "fix-login-a1b";
    start(&amx, id, "takes-a-turn");
    assert_eq!(session_of(&amx, id), format!("session: created {id}"));

    let out = amx_with_pi(&amx, "takes-a-turn", &["fork", id]);
    assert!(
        out.status.success(),
        "amx fork: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let copy = String::from_utf8_lossy(&out.stdout).trim().to_string();
    assert_ne!(copy, id, "a fork is a second agent, with an id of its own");

    let called = argv_of(&amx, &copy);
    assert!(
        called.contains(&format!("--fork {id} --session-id {copy}")),
        "the session to branch from and the one to open, in that order: {called}"
    );
    assert_eq!(
        session_of(&amx, &copy),
        format!("session: branched {copy} from {id}"),
        "and the vendor branched one into the other"
    );
    assert_eq!(
        amx.meta(&copy)["session"],
        copy,
        "the copy's record names the copy's own conversation"
    );
    assert!(session_file(&amx, &copy).exists());
}

#[test]
fn the_stand_in_spins_pis_line_in_pis_own_top_border() {
    // The `spinner` screen as pi 0.85.1 draws it: `── `, the frame, `Working`
    // and `─` to the edge in the composer's top border, with no status row
    // above the box. The bottom border, which the rule anchors on, is two
    // rows below.
    let amx = Harness::new();
    let id = "watch-log-c3d";
    start(&amx, id, "works-without-end");
    let pane = amx.pane_of(id);

    // The stand-in paints the whole screen in one write, so the box is
    // asserted from the capture that shows the spinner.
    let rows = amx.until("the turn to be under way", || {
        let rows = drawn(&amx, &pane);
        row_of(&rows, "Working").is_some().then_some(rows)
    });
    let working = row_of(&rows, "Working").expect("the line pi spins");
    assert!(
        framed_border(&rows[working]),
        "the frame and the word are in the box's top border: {rows:?}"
    );
    let bottom = *borders(&rows)
        .first()
        .unwrap_or_else(|| panic!("pi's composer box: {rows:?}"));
    assert_eq!(
        bottom - working,
        2,
        "the top border, one blank row, and the bottom border: {rows:?}"
    );
    assert!(
        !rows.iter().any(|row| row.contains("Working...")),
        "0.85.1 dropped the ellipsis: {rows:?}"
    );
}

#[test]
fn the_stand_in_spins_the_status_lines_that_do_not_say_working() {
    // pi shows one status indicator of four kinds at a time. Compaction and
    // retry replace the working indicator with their own on the row above the
    // box, so `Working` is gone. `ctx.ui.setWorkingMessage` rewrites the
    // working indicator's message, which 0.85.1 draws in the top border. All
    // three keep the spinner frame, which is what the spinner rule reads.
    for (what, scenario, message, embedded) in OTHER_STATUS_LINES {
        let amx = Harness::new();
        let id = "watch-log-c3d";
        start(&amx, id, scenario);
        let pane = amx.pane_of(id);

        let rows = amx.until("the status line to be drawn", || {
            let rows = drawn(&amx, &pane);
            row_of(&rows, message).is_some().then_some(rows)
        });

        let line = row_of(&rows, message).expect("the status line");
        let bottom = *borders(&rows)
            .last()
            .unwrap_or_else(|| panic!("pi's composer box: {rows:?}"));
        if embedded {
            assert!(
                framed_border(&rows[line]),
                "{what}: the message is in the box's top border, where the \
                 vendor's own word goes: {rows:?}"
            );
            assert_eq!(
                bottom - line,
                2,
                "{what}: the top border, one blank row, and the bottom border: {rows:?}"
            );
        } else {
            let top = borders(&rows)[0];
            assert_eq!(
                top - line,
                2,
                "{what}: the line, one blank row, and the top of the box: {rows:?}"
            );
            assert!(
                spins(&rows[line]),
                "{what}: the row opens with the frame pi spins: {rows:?}"
            );
        }
        assert!(
            !rows.iter().any(|row| row.contains("Working")),
            "{what}: the word the spinner rule used to stand on is nowhere on \
             the pane: {rows:?}"
        );
        assert_eq!(
            rows.len() - bottom,
            3,
            "{what}: the working directory and the stats line under the box, \
             same as any other screen: {rows:?}"
        );
    }
}

#[test]
fn a_pi_whose_status_line_stopped_saying_working_is_still_working() {
    // Compacting, retrying, and a turn under an extension's working message
    // all run with no `Working` on the pane, and a rule anchored on that word
    // read them `unknown`. The spinner frame is present on all three.
    for (what, scenario, message, _) in OTHER_STATUS_LINES {
        let amx = Harness::new();
        let id = "watch-log-c3d";
        start(&amx, id, scenario);
        let pane = amx.pane_of(id);

        amx.until("the status line to be drawn", || {
            row_of(&drawn(&amx, &pane), message).is_some().then_some(())
        });

        // Silent for an hour with nothing outstanding, so only the screen is
        // read.
        amx.set_state(
            id,
            json!({ "state": "starting", "since": 1, "last_event": 1 }),
        );

        let agent = status(&amx, id);
        assert_eq!(agent["state"], "working", "{what}: {agent}");
        assert_eq!(agent["evidence"], "screen", "{what}: {agent}");
        assert_eq!(
            agent["rule"], "spinner",
            "pi's own rule, out of pi's own document: {agent}"
        );
    }
}

#[test]
fn the_stand_in_draws_the_box_and_the_footer_pi_keeps_under_every_screen() {
    // The chrome the other rules anchor on and `src/furniture.rs` skips: the
    // box's two borders, then the working directory and the stats line
    // directly below.
    let amx = Harness::new();
    let id = "fix-login-a1b";
    start(&amx, id, "takes-a-turn");
    let pane = amx.pane_of(id);

    // Only a finished turn shows `Took`.
    let rows = amx.until("the turn to be over", || {
        let rows = drawn(&amx, &pane);
        row_of(&rows, "Took").is_some().then_some(rows)
    });

    assert!(
        row_of(&rows, "Working").is_none(),
        "the spinner went with the turn: {rows:?}"
    );
    assert_eq!(borders(&rows).len(), 2, "the box is drawn whole: {rows:?}");
    let (top, bottom) = (borders(&rows)[0], borders(&rows)[1]);
    assert_eq!(bottom - top, 2, "a row for what is staged in it: {rows:?}");
    assert_eq!(
        rows.len() - bottom,
        3,
        "the working directory and the stats line, and nothing else: {rows:?}"
    );
    let stats = rows.last().expect("a stats line");
    assert!(
        ['↑', '$'].iter().any(|opening| stats.starts_with(*opening)),
        "the stats line opens on one of the parts pi truncates towards: {stats}"
    );
}

#[test]
fn the_stand_in_draws_the_dialog_in_pis_box_with_the_turn_still_over_it() {
    // The `dialog` screen as measured: inside the composer box, with the usual
    // footer below and the turn that raised it still running. The rule
    // anchors on the `↑↓ navigate` hint row, so this checks the layout as well
    // as the words.
    let amx = Harness::new();
    let id = "watch-log-c3d";
    start(&amx, id, "asks-a-question");
    let pane = amx.pane_of(id);

    let rows = amx.until("the dialog to be drawn", || {
        let rows = drawn(&amx, &pane);
        row_of(&rows, "navigate").is_some().then_some(rows)
    });

    assert_eq!(borders(&rows).len(), 2, "the box is drawn whole: {rows:?}");
    let (top, bottom) = (borders(&rows)[0], borders(&rows)[1]);
    let hint = row_of(&rows, "navigate").expect("the hint row");
    assert!(
        top < hint && hint < bottom,
        "the hint row sits inside the box, where the editor usually is: {rows:?}"
    );
    // pi 0.85.1 keeps the working indicator in the editor the dialog
    // replaces, so no frame shows though the turn is running. With a
    // compaction row also on screen, rule order keeps the spinner rule off it.
    assert!(
        !rows.iter().any(|row| spins(row) || framed_border(row)),
        "no frame anywhere on a dialog raised mid-turn: {rows:?}"
    );
    assert_eq!(
        rows.len() - bottom,
        3,
        "the working directory and the stats line under it, same as any other screen: {rows:?}"
    );
    let stats = rows.last().expect("a stats line");
    assert!(
        ['↑', '$'].iter().any(|opening| stats.starts_with(*opening)),
        "the stats line opens on one of the parts pi truncates towards: {stats}"
    );
}

#[test]
fn the_stand_in_draws_the_two_screens_a_caller_asks_for_words_on() {
    // The `input` and `editor` screens as measured: the caller's title two rows
    // below the box's top border, the text field below it, and a hint row
    // starting `enter submit` (the dialog's starts `↑↓ navigate`). The editor
    // draws a second box for its text block, which tells the two rules apart.
    for (what, scenario, title, boxes) in [
        (
            "a line",
            "asks-for-a-line",
            "Which branch should I push to?",
            2,
        ),
        ("a block", "asks-for-a-block", "Write the commit message", 4),
    ] {
        let amx = Harness::new();
        let id = "fix-login-a1b";
        start(&amx, id, scenario);
        let pane = amx.pane_of(id);

        let rows = amx.until("the caller's question to be drawn", || {
            let rows = drawn(&amx, &pane);
            row_of(&rows, title).is_some().then_some(rows)
        });

        let drawn_borders = borders(&rows);
        assert_eq!(
            drawn_borders.len(),
            boxes,
            "asking for {what}, the box is drawn whole: {rows:?}"
        );
        let (top, bottom) = (drawn_borders[0], drawn_borders[boxes - 1]);
        assert_eq!(
            row_of(&rows, title).expect("the title") - top,
            2,
            "the border, one blank row, and then the title: {rows:?}"
        );
        assert!(
            row_of(&rows, "enter submit").is_some_and(|hint| top < hint && hint < bottom),
            "the hint row sits inside the box, where the editor usually is: {rows:?}"
        );
        // Either can be raised mid-turn or between turns. Both were measured
        // with no turn running.
        assert!(
            row_of(&rows, "Working").is_none(),
            "no turn is under way behind this question: {rows:?}"
        );
        assert_eq!(
            rows.len() - bottom,
            3,
            "the working directory and the stats line under it, same as any \
             other screen: {rows:?}"
        );
    }
}

#[test]
fn a_pi_stopped_by_a_caller_carries_the_question_that_caller_asked() {
    // An extension can block pi three ways, and two of them used to read
    // `unknown`. All three put the caller's sentence at the top of pi's box,
    // and no hook carries it, so the question is read from that row.
    for (what, scenario, rule, question, options) in [
        (
            "a choice",
            "asks-a-question",
            "dialog",
            "Run echo hi?",
            &["Allow once", "Allow always", "Deny"][..],
        ),
        (
            "a line",
            "asks-for-a-line",
            "input",
            "Which branch should I push to?",
            &[][..],
        ),
        (
            "a block",
            "asks-for-a-block",
            "editor",
            "Write the commit message",
            &[][..],
        ),
    ] {
        let amx = Harness::new();
        let id = "fix-login-a1b";
        start(&amx, id, scenario);
        let pane = amx.pane_of(id);

        amx.until("the caller's question to be drawn", || {
            row_of(&drawn(&amx, &pane), question)
                .is_some()
                .then_some(())
        });

        // Silent for an hour with nothing outstanding, so only the screen is
        // read.
        amx.set_state(
            id,
            json!({ "state": "starting", "since": 1, "last_event": 1 }),
        );

        let agent = status(&amx, id);
        assert_eq!(agent["state"], "waiting", "asking for {what}: {agent}");
        assert_eq!(
            agent["rule"], rule,
            "pi's own rule, out of pi's own document: {agent}"
        );
        assert_eq!(agent["kind"], "question", "asking for {what}: {agent}");
        assert_eq!(
            agent["question"], question,
            "the sentence the caller passed, off the pane it is drawn on: {agent}"
        );
        assert_eq!(
            agent["options"],
            json!(options),
            "the choices off the arrow, numbered by amx, and none where the \
             caller asked for words instead: {agent}"
        );
        assert_eq!(
            agent["walked"],
            json!(!options.is_empty()),
            "a list amx numbered itself says so, and a screen with no list \
             does not: {agent}"
        );
    }
}

#[test]
fn a_pi_on_the_folder_trust_question_reads_trust_and_not_a_tool_call() {
    // pi draws this in the same box as a gated tool call's dialog, with the
    // same `↑↓ navigate` hint row, so the dialog rule used to claim it. The
    // kind decides what may be sent back: this one takes a trust decision
    // about the directory, not a choice from a caller's menu.
    let amx = Harness::new();
    let id = "fix-login-a1b";
    start(&amx, id, "stops-on-trust");
    let pane = amx.pane_of(id);

    let rows = amx.until("the trust question to be drawn", || {
        let rows = drawn(&amx, &pane);
        row_of(&rows, "Project trust").is_some().then_some(rows)
    });

    assert_eq!(borders(&rows).len(), 2, "the box is drawn whole: {rows:?}");
    let (top, bottom) = (borders(&rows)[0], borders(&rows)[1]);
    let title = row_of(&rows, "Project trust").expect("the title");
    assert_eq!(
        title - top,
        2,
        "the border, one blank row, and then the title: {rows:?}"
    );
    // The hint row sits where a tool call's does, which the dialog rule
    // anchors on, but says `enter save` where a tool call's says `enter
    // select`.
    assert!(
        row_of(&rows, "enter save").is_some_and(|hint| top < hint && hint < bottom),
        "the hint row sits inside the box, where the editor usually is: {rows:?}"
    );
    // pi shows this before a turn starts, so no turn is running behind it.
    assert!(
        row_of(&rows, "Working").is_none(),
        "no turn is under way behind this question: {rows:?}"
    );
    assert_eq!(
        rows.len() - bottom,
        3,
        "the working directory and the stats line under it, same as any other \
         screen: {rows:?}"
    );

    // Silent for an hour with nothing outstanding, so only the screen is
    // read.
    amx.set_state(
        id,
        json!({ "state": "starting", "since": 1, "last_event": 1 }),
    );

    let agent = status(&amx, id);
    assert_eq!(agent["state"], "waiting", "{agent}");
    assert_eq!(
        agent["kind"], "trust",
        "the folder-trust question, and not the tool call the dialog rule \
         would have made of it: {agent}"
    );
    assert_eq!(
        agent["rule"], "project_trust",
        "pi's own rule, out of pi's own document: {agent}"
    );
}

#[test]
fn a_walked_list_on_a_pi_is_offered_by_its_numbers_and_answered_with_one() {
    // pi draws this selector with an arrow on the cursor row and no numbers,
    // so the numbers `status` prints are amx's own and `answer` must take the
    // same ones.
    //
    // Measured on pi 0.85.1 at 100 columns: `1`, `2`, `y` and `n` do nothing
    // to the selector, and `enter` takes the cursor row, the first until it
    // moves. An unintended `Trust` cannot be undone, so amx refuses `enter`
    // and a digit selects a row.
    let amx = Harness::new();
    let id = "fix-login-a1b";
    start(&amx, id, "stops-on-trust");
    let pane = amx.pane_of(id);

    amx.until("the trust question to be drawn", || {
        row_of(&drawn(&amx, &pane), "Project trust")
    });
    // Silent for an hour with nothing outstanding, so only the screen is
    // read.
    amx.set_state(
        id,
        json!({ "state": "starting", "since": 1, "last_event": 1 }),
    );

    // The `status` look that finds the question also records its choices.
    let printed = amx.until("the trust question to reach the record", || {
        let out = amx.amx(&["status", id]);
        assert!(
            out.status.success(),
            "amx status: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let printed = String::from_utf8_lossy(&out.stdout).into_owned();
        printed.contains("1. Trust").then_some(printed)
    });

    let dir = amx.home().to_string_lossy().to_string();
    let parent = dir.rsplit_once('/').expect("a parent folder").0.to_string();
    let second = format!("Trust parent folder ({parent})");
    for choice in ["1. Trust", &format!("2. {second}"), "3. Do not trust"] {
        assert!(
            printed.contains(choice),
            "the rows of the run the arrow is in, numbered in the order pi drew \
             them: {printed}"
        );
    }
    assert!(
        printed.contains(&format!("answer    amx answer {id} <1-3|esc>")),
        "and the command under them takes those numbers and the key that \
         cancels, and offers no key this screen swallows: {printed}"
    );

    // `enter` takes whatever row the cursor is on, so it is refused.
    let out = amx.amx(&["answer", id, "enter"]);
    assert_eq!(
        out.status.code(),
        Some(64),
        "amx answer enter: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    let refused = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        refused.contains("press 1-3"),
        "refused naming the numbers that do reach a row: {refused}"
    );
    assert!(
        !amx.event_kinds(id).iter().any(|kind| kind == "answer"),
        "and nothing was answered on the way to refusing it"
    );

    // A digit walks to its row and selects it. The event records the row's
    // text.
    let out = amx.amx(&["answer", id, "2"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "amx answer 2: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let answered = amx
        .events(id)
        .into_iter()
        .find(|event| event["kind"] == "answer")
        .expect("the answer on the record");
    assert_eq!(
        answered["payload"],
        json!({ "key": "2", "answer": second }),
        "the digit that was typed and the row it chose, rather than the keys \
         amx walked to reach it: {answered}"
    );
}

#[test]
fn the_stand_in_draws_the_gate_pi_puts_in_front_of_a_first_run() {
    // The `first_time_setup` screen is the only one in pi's rules with none of
    // pi's chrome below it: pi asks for a theme before it draws a session, so
    // the pane holds a box with pi's banner and nothing else.
    let amx = Harness::new();
    let id = "fix-login-a1b";
    start(&amx, id, "stops-at-setup");
    let pane = amx.pane_of(id);

    let rows = amx.until("the setup gate to be drawn", || {
        let rows = drawn(&amx, &pane);
        row_of(&rows, "Welcome to pi,").is_some().then_some(rows)
    });

    assert_eq!(borders(&rows).len(), 2, "the box is drawn whole: {rows:?}");
    let (top, bottom) = (borders(&rows)[0], borders(&rows)[1]);
    let banner = row_of(&rows, "Welcome to pi,").expect("the vendor's banner");
    assert_eq!(
        banner - top,
        7,
        "the border, a blank row, the four rows pi draws its mark in, another \
         blank row, and then the banner: {rows:?}"
    );
    assert!(
        row_of(&rows, "navigate").is_some_and(|hint| top < hint && hint < bottom),
        "the hint row sits inside the box: {rows:?}"
    );
    assert_eq!(
        rows.len() - bottom,
        1,
        "and nothing under it: no working directory and no stats line, because \
         this is the vendor's own startup screen and not the pane a session \
         runs in: {rows:?}"
    );
}

#[test]
fn the_stand_in_draws_the_login_dialog_in_the_slot_pis_composer_had() {
    // The `login` screen as measured: the box where the composer was, the
    // usual footer, and the title on the row directly under the top border.
    // The missing blank row is its one difference from a caller's dialogs.
    let amx = Harness::new();
    let id = "fix-login-a1b";
    start(&amx, id, "stops-on-login");
    let pane = amx.pane_of(id);

    let rows = amx.until("the login dialog to be drawn", || {
        let rows = drawn(&amx, &pane);
        row_of(&rows, "Login to").is_some().then_some(rows)
    });

    assert_eq!(borders(&rows).len(), 2, "the box is drawn whole: {rows:?}");
    let (top, bottom) = (borders(&rows)[0], borders(&rows)[1]);
    let title = row_of(&rows, "Login to").expect("the vendor's title");
    assert_eq!(title - top, 1, "the border and then the title: {rows:?}");
    assert!(
        row_of(&rows, "escape/ctrl+c to").is_some_and(|hint| top < hint && hint < bottom),
        "the hint row sits inside the box, where the editor usually is: {rows:?}"
    );
    assert_eq!(
        rows.len() - bottom,
        3,
        "the working directory and the stats line under it, same as any other \
         screen: {rows:?}"
    );
    let stats = rows.last().expect("a stats line");
    assert!(
        stats.starts_with("0.0%/"),
        "a pi nobody has logged in has no cost and no tokens to show, so its \
         stats line opens on the context indicator: {stats}"
    );
}

#[test]
fn the_two_screens_a_fresh_pi_stops_on_each_read_waiting() {
    // Both used to read wrong. The setup gate has the dialog rule's hint row,
    // so it read as a tool call. The login box is short enough that it and
    // the stats line matched `prompt`, so a pi waiting for a key read idle.
    for (what, scenario, drawn_row, rule, question, options) in [
        (
            "the gate a first run stops at",
            "stops-at-setup",
            "Welcome to pi,",
            "first_time_setup",
            "Pick a theme. Detected system appearance: dark",
            &["Dark", "Light"][..],
        ),
        (
            "a pi waiting for a provider's key",
            "stops-on-login",
            "Login to",
            "login",
            "Enter Cerebras API key",
            &[][..],
        ),
    ] {
        let amx = Harness::new();
        let id = "fix-login-a1b";
        start(&amx, id, scenario);
        let pane = amx.pane_of(id);

        amx.until("the screen to be drawn", || {
            row_of(&drawn(&amx, &pane), drawn_row)
                .is_some()
                .then_some(())
        });

        // Silent for an hour with nothing outstanding, so only the screen is
        // read.
        amx.set_state(
            id,
            json!({ "state": "starting", "since": 1, "last_event": 1 }),
        );

        let agent = status(&amx, id);
        assert_eq!(agent["state"], "waiting", "{what}: {agent}");
        assert_eq!(
            agent["rule"], rule,
            "pi's own rule, out of pi's own document: {agent}"
        );
        assert_eq!(agent["kind"], "question", "{what}: {agent}");
        assert_eq!(
            agent["question"], question,
            "the sentence the vendor is waiting on, off the pane it is drawn \
             on: {agent}"
        );
        assert_eq!(
            agent["options"],
            json!(options),
            "the two themes off the arrow on the gate, numbered by amx, and no \
             list on a box waiting for a key: {agent}"
        );
        assert_eq!(
            agent["walked"],
            json!(!options.is_empty()),
            "a list amx numbered itself says so, and a screen with no list \
             does not: {agent}"
        );
    }
}

#[test]
fn the_stand_in_draws_a_selector_in_the_slot_pis_composer_had() {
    // No rule is named for this screen, and `docs/pi-screens.md` counts
    // fourteen like it: a widget the person opened, drawn between the
    // composer's borders above the usual footer. It has no hint row or title
    // any rule knows, so it reaches the last rule.
    //
    // The topmost border a rule can see is five rows above the stats line
    // with nothing above the box, and seven under a transcript, because `!cmd`
    // leaves its box's bottom border on the pane.
    for (what, scenario, span) in SELECTORS {
        let amx = Harness::new();
        let id = "fix-login-a1b";
        start(&amx, id, scenario);
        let pane = amx.pane_of(id);

        let rows = amx.until("the selector to be drawn", || {
            let rows = drawn(&amx, &pane);
            row_of(&rows, "Show images inline")
                .is_some()
                .then_some(rows)
        });

        let drawn_borders = borders(&rows);
        let (top, bottom) = (
            drawn_borders[0],
            *drawn_borders
                .last()
                .unwrap_or_else(|| panic!("{what}: pi's composer box: {rows:?}")),
        );
        let choice = row_of(&rows, "Show images inline").expect("the first choice");
        assert!(
            drawn_borders[drawn_borders.len() - 2] < choice && choice < bottom,
            "{what}: the choices sit inside the box, where the editor usually \
             is: {rows:?}"
        );
        assert_eq!(
            rows.len() - 1 - top,
            span,
            "{what}: the topmost border a rule can see, and the stats line \
             under the box: {rows:?}"
        );
        assert_eq!(
            rows.len() - bottom,
            3,
            "{what}: the working directory and the stats line under it, same \
             as any other screen: {rows:?}"
        );
        // pi shows no key hints for this widget, so no hint-row rule matches.
        for hint in ["navigate", "enter submit", "escape/ctrl+c"] {
            assert!(
                row_of(&rows, hint).is_none(),
                "{what}: no hint row for a rule to claim it by: {rows:?}"
            );
        }
    }
}

#[test]
fn a_widget_in_the_slot_pis_composer_had_is_not_pis_prompt() {
    // `prompt`, the last rule, was measured on an empty composer: box, working
    // directory and stats line, four rows. A selector has the same borders
    // with a list between them, and at five or seven rows it fit the old
    // window of eight, so a pi that would take the next key as a menu choice
    // read idle and `send` would have typed into the widget.
    //
    // The verdict must also not depend on how much transcript is above the
    // box.
    let mut verdicts = Vec::new();
    for (what, scenario, _) in SELECTORS {
        let amx = Harness::new();
        let id = "fix-login-a1b";
        start(&amx, id, scenario);
        let pane = amx.pane_of(id);

        amx.until("the selector to be drawn", || {
            row_of(&drawn(&amx, &pane), "Show images inline")
                .is_some()
                .then_some(())
        });

        // Silent for an hour with nothing outstanding, so only the screen is
        // read.
        amx.set_state(
            id,
            json!({ "state": "starting", "since": 1, "last_event": 1 }),
        );

        let agent = status(&amx, id);
        assert_eq!(
            agent["state"], "unknown",
            "{what}: a widget in the composer's slot is not a prompt: {agent}"
        );
        assert!(
            agent["rule"].is_null(),
            "{what}: and no rule in pi's document claims it: {agent}"
        );
        verdicts.push(agent["state"].clone());
    }

    assert_eq!(
        verdicts[0], verdicts[1],
        "the same widget, read the same way with a transcript above it as with \
         none"
    );
}

#[test]
fn a_quiet_pi_is_read_against_pis_own_document() {
    // Readers used to apply claude's rules to every pane. pi draws none of
    // claude's anchors, so an idle pi read `unknown` with its prompt on
    // screen. The command recorded at spawn now picks the rules.
    let amx = Harness::new();
    let id = "fix-login-a1b";
    start(&amx, id, "takes-a-turn");
    assert_eq!(amx.meta(id)["agent"], "pi", "the record says which vendor");
    let pane = amx.pane_of(id);

    amx.until("the turn to be over", || {
        row_of(&drawn(&amx, &pane), "Took").is_some().then_some(())
    });

    // Silent for an hour with nothing outstanding, so a quiescent rule
    // decides at once.
    amx.set_state(
        id,
        json!({ "state": "starting", "since": 1, "last_event": 1 }),
    );

    let agent = status(&amx, id);
    assert_eq!(agent["state"], "idle", "{agent}");
    assert_eq!(agent["evidence"], "screen", "{agent}");
    assert_eq!(
        agent["rule"], "prompt",
        "pi's own rule, out of pi's own document: {agent}"
    );
}

#[test]
fn a_fresh_pi_under_its_update_notice_still_reads_idle_and_working() {
    // pi draws an Update Available box, with the composer's borders, above the
    // composer whenever a newer pi exists. Rules counting from the topmost
    // border anchored on the notice, so a fresh pi read `unknown` idle and
    // mid-turn until the transcript scrolled the box away.
    let amx = Harness::new();
    let id = "fix-login-a1b";
    start(&amx, id, "boots-under-the-notice");
    let pane = amx.pane_of(id);

    amx.until("the notice on the pane", || {
        row_of(&drawn(&amx, &pane), "Update Available")
            .is_some()
            .then_some(())
    });
    // Silent for an hour with nothing outstanding, so only the screen is read.
    amx.set_state(
        id,
        json!({ "state": "starting", "since": 1, "last_event": 1 }),
    );
    let agent = status(&amx, id);
    assert_eq!(agent["state"], "idle", "{agent}");
    assert_eq!(
        agent["rule"], "prompt",
        "the composer under the box: {agent}"
    );

    // Then a turn under the same notice, with the frame in the composer's top
    // border as 0.85.1 draws it.
    amx.until("the turn under the notice", || {
        let rows = drawn(&amx, &pane);
        (row_of(&rows, "Update Available").is_some() && rows.iter().any(|row| framed_border(row)))
            .then_some(())
    });
    let agent = status(&amx, id);
    assert_eq!(agent["state"], "working", "{agent}");
    assert_eq!(agent["rule"], "spinner", "the frame under the box: {agent}");
}

#[test]
fn a_pi_that_has_held_still_settles_for_whichever_process_looks_next() {
    // Stillness used to be counted as consecutive looks in one process's
    // memory, and every verb but the view looks once and exits. So `ls` and
    // `status` never let the quiescent `prompt` rule end a turn, and a pi
    // whose turn ended without a hook read `working` indefinitely. The screen
    // and when it was first seen now live on the record.
    let amx = Harness::new();
    let id = "fix-login-a1b";
    start(&amx, id, "takes-a-turn");
    let pane = amx.pane_of(id);

    amx.until("the turn to be over", || {
        row_of(&drawn(&amx, &pane), "Took").is_some().then_some(())
    });

    // Running on the record and silent for an hour: `prompt` may end the turn
    // only once the screen has held still.
    amx.set_state(
        id,
        json!({ "state": "working", "since": 1, "last_event": 1 }),
    );

    let looked = now();
    let out = amx.amx(&["result", id, "--timeout", "1"]);
    assert_eq!(
        out.status.code(),
        Some(3),
        "a screen amx has only just laid eyes on ends no turn: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // The wait recorded the screen's hash and when it first saw it.
    let state = amx.state(id);
    let seen = state["still"]["screen"].clone();
    assert!(seen.is_u64(), "the screen it saw, hashed: {state}");
    let since = state["still"]["since"]
        .as_u64()
        .unwrap_or_else(|| panic!("when it first saw that screen: {state}"));
    assert!(since >= looked, "stamped at the look that saw it: {state}");
    assert_eq!(
        state["last_event"], 1,
        "and the record is no fresher for having been looked at: {state}"
    );

    // Backdate the first sighting by `SETTLED` seconds instead of waiting.
    let mut aged = state.clone();
    aged["still"]["since"] = json!(since - SETTLED);
    amx.set_state(id, aged);

    // One look from a new process ends the turn from the recorded stillness.
    let row = listed(&amx, id);
    assert_eq!(row["state"], "idle", "{row}");
    assert_eq!(row["evidence"], "screen", "{row}");
    assert_eq!(
        row["rule"], "prompt",
        "pi's own rule, out of pi's own document: {row}"
    );

    // A changed screen restarts the clock: the recorded hash does not match
    // the pane, so its old stamp does not count.
    amx.set_state(
        id,
        json!({
            "state": "working",
            "since": 1,
            "last_event": 1,
            "still": { "screen": 0, "since": 1 },
        }),
    );

    let agent = status(&amx, id);
    assert_eq!(
        agent["state"], "working",
        "the record stands over a screen amx is seeing for the first time: {agent}"
    );
    let state = amx.state(id);
    assert_eq!(state["still"]["screen"], seen, "the screen on the pane now");
    assert!(
        state["still"]["since"]
            .as_u64()
            .is_some_and(|at| at >= looked),
        "stamped at this look rather than at the one it replaced: {state}"
    );
}

#[test]
fn adopt_takes_the_pi_in_the_pane_over_and_not_the_claude_in_the_terminal() {
    // `adopt` used to check vendor session variables in table order, so a pi
    // started from a terminal holding claude's session id was adopted as
    // claude, with claude's id on the record and claude's rules on pi's pane.
    let amx = Harness::new();
    // The dialog's hint row, which pi's rules anchor that screen on.
    let pane = a_pi_started_by_hand(&amx, "asks-a-question", THEIR_PI, "↑↓ navigate");

    // claude's variable alone comes from the terminal and says nothing about
    // what runs in this pane.
    let out = adopt(
        &amx,
        "read-as-claude-c3d",
        &pane,
        &[("CLAUDE_CODE_SESSION_ID", A_CLAUDE)],
    );
    assert!(
        !out.status.success(),
        "a pi pane adopted as claude: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    let why = String::from_utf8_lossy(&out.stderr);
    assert!(
        why.contains("pi"),
        "the refusal names what was in the pane: {why}"
    );
    assert!(
        why.contains("PI_SESSION_ID"),
        "and the variable that would have said which pi conversation: {why}"
    );

    // A pi started from that terminal has both: its own id and the inherited
    // one.
    let id = "their-own-pi-a1b";
    let out = adopt(
        &amx,
        id,
        &pane,
        &[
            ("CLAUDE_CODE_SESSION_ID", A_CLAUDE),
            ("PI_SESSION_ID", THEIR_PI),
        ],
    );
    assert!(
        out.status.success(),
        "amx adopt: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    assert_eq!(
        amx.meta(id)["agent"],
        "pi",
        "the program tmux says is running in the pane"
    );
    assert_eq!(
        amx.meta(id)["session"],
        THEIR_PI,
        "the conversation pi named, which is how anything ever finds this \
         record again"
    );
    assert_eq!(
        amx.state(id)["state"],
        "waiting",
        "read by pi's own document, which is the only one that claims this \
         screen"
    );
    assert_eq!(
        amx.state(id)["question"]["text"],
        "Run echo hi?",
        "the sentence the caller passed, off the pane it is drawn on — and \
         written whole rather than as its words alone, because the words alone \
         are how the document says a hook carried a question and an adoption \
         read this one off the screen"
    );
}

#[test]
fn an_adopted_pi_streams_what_it_is_saying_to_the_record_the_hook_named() {
    // A pane amx did not start has no `AMX_DIR`, so the extension had nowhere
    // to stream and an adopted pi's row stayed blank all turn. The hook now
    // answers each report with the record's directory, and the stand-in, like
    // the extension, streams there.
    let amx = Harness::new();
    // `(auto)` is on the stats line, which the idle screen ends on.
    let pane = a_pi_started_by_hand(&amx, "streams-an-answer", THEIR_PI, "(auto)");
    let id = "their-own-pi-a1b";
    let out = adopt(&amx, id, &pane, &[("PI_SESSION_ID", THEIR_PI)]);
    assert!(
        out.status.success(),
        "amx adopt: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let agent = amx.until("the words being written, on the row", || {
        let agent = status(&amx, id);
        agent["summary"]
            .as_str()
            .is_some_and(|line| line.starts_with("the login bug is in the redirect"))
            .then_some(agent)
    });
    assert_eq!(agent["state"], "working", "{agent}");
    assert!(
        amx.agent_dir(id).join("live").exists(),
        "streamed to the record the hook named"
    );

    let agent = amx.until("the turn to settle", || {
        let agent = status(&amx, id);
        (agent["state"] == json!("idle")).then_some(agent)
    });
    assert_eq!(
        agent["result"], "the login bug is in the redirect, and the fixture hides it",
        "{agent}"
    );
    assert!(
        !amx.agent_dir(id).join("live").exists(),
        "and the stream is taken away before the turn settles"
    );
}

#[test]
fn a_beating_pi_is_working_past_the_window_and_unknown_once_the_beating_stops() {
    // A vendor report is trusted for `FRESH` seconds, then the pane is read.
    // A turn is silent during a tool call, and an extension can redraw both
    // mid-turn markers (the braille frames and the stats line), so a long
    // call read `unknown` after ten seconds.
    //
    // The extension beats beside the record while the turn runs, and a reader
    // that sees the beat skips the pane. Neither timeline has a screen step,
    // so both panes show only the stand-in's opening line and no rule matches.
    let amx = Harness::new();

    // Beats for about 20 seconds, longer than the test needs.
    let beating = "fix-login-a1b";
    let mut steps = String::from("hook session_start {}\nhook agent_start {}\n");
    for _ in 0..50 {
        steps.push_str("heartbeat\nsleep 400\n");
    }
    steps.push_str("sleep 600000\n");
    start_playing(&amx, beating, &timeline(&amx, beating, &steps));

    // Stops beating mid-turn without reporting anything else.
    let stopped = "fix-logout-c3d";
    let ended = timeline(
        &amx,
        stopped,
        "hook session_start {}\nhook agent_start {}\n\
         heartbeat\nsleep 2000\nheartbeat off\nsleep 600000\n",
    );
    start_playing(&amx, stopped, &ended);

    for id in [beating, stopped] {
        amx.until_state(id, "working");
    }
    // Wait for the beat file to appear before waiting for it to go: before
    // the first beat it is just as absent.
    let beat = amx.agent_dir(stopped).join("heartbeat");
    amx.until("the second pi to beat", || beat.exists().then_some(()));
    amx.until("and to stop beating", || (!beat.exists()).then_some(()));

    // Both records: running, and silent for an hour, far past `FRESH`. Only
    // the beat can keep either one working.
    for id in [beating, stopped] {
        amx.set_state(
            id,
            json!({ "state": "working", "since": 1, "last_event": 1 }),
        );
    }

    let agent = status(&amx, beating);
    assert_eq!(agent["state"], "working", "{agent}");
    assert_eq!(
        agent["evidence"], "hooks",
        "a beat is the vendor's own report that the turn goes on: {agent}"
    );
    assert!(
        agent["rule"].is_null(),
        "and nothing on the pane says so: no rule claims it: {agent}"
    );
    assert!(
        agent["age"].as_u64().is_some_and(|age| age < FRESH),
        "the beat is the last thing heard, not the hour-old record: {agent}"
    );
    assert!(
        amx.agent_dir(beating).join("heartbeat").exists(),
        "beaten beside the record, which is where a reader looks"
    );

    let agent = status(&amx, stopped);
    assert_eq!(
        agent["state"], "unknown",
        "a record nothing speaks for, over a screen no rule claims: {agent}"
    );
    assert_eq!(agent["evidence"], "unknown", "{agent}");
    assert!(agent["rule"].is_null(), "{agent}");
    assert!(
        agent["age"].as_u64().is_some_and(|age| age > FRESH),
        "with how long it has been since anything was heard: {agent}"
    );
}

#[test]
fn a_pi_reports_its_turn_and_the_record_moves_by_its_word() {
    // pi's extension moves the record the way claude's hooks do: the session
    // and its file, turn start, the running tool, and turn end with the
    // answer. Nothing is read from the pane.
    let amx = Harness::new();
    let id = "fix-login-a1b";
    start(&amx, id, "reports-a-turn");

    // The command comes from pi's transcript; the hook alone names only the
    // tool.
    let agent = amx.until("the call to be on the row", || {
        let agent = status(&amx, id);
        (agent["summary"] == json!("bash cargo test")).then_some(agent)
    });
    assert_eq!(agent["state"], "working", "{agent}");
    assert_eq!(
        agent["evidence"], "hooks",
        "the vendor's own word, not a reading of its screen: {agent}"
    );

    let agent = amx.until("the turn to settle", || {
        let agent = status(&amx, id);
        (agent["state"] == json!("idle")).then_some(agent)
    });
    assert_eq!(agent["result"], ANSWERED, "{agent}");
    assert_eq!(
        agent["source"], "payload",
        "the answer came with the report: {agent}"
    );

    // The reported session and its file go on the record; `logs` reads that
    // file back.
    let meta = amx.meta(id);
    let session = meta["session"]
        .as_str()
        .unwrap_or_else(|| panic!("the session pi opened: {meta}"));
    assert_eq!(
        meta["transcript"],
        json!(session_file(&amx, session)),
        "{meta}"
    );

    let kinds = amx.event_kinds(id);
    for word in PIS_WORDS {
        assert!(kinds.iter().any(|kind| kind == word), "{word} in {kinds:?}");
    }
    for word in [READ_PROMPT, READ_TURN_END] {
        assert!(
            !kinds.iter().any(|kind| kind == word),
            "a reading places no edge on a vendor that places its own: {kinds:?}"
        );
    }

    // `result` and `logs` work from those reports.
    let out = amx.amx(&["result", id, "--timeout", "30"]);
    assert!(
        out.status.success(),
        "amx result: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), ANSWERED);

    let out = amx.amx(&["logs", id]);
    let printed = String::from_utf8_lossy(&out.stdout);
    assert!(printed.contains("❯ fix the login bug"), "{printed}");
    assert!(printed.contains("› bash cargo test"), "{printed}");
    assert!(printed.contains(ANSWERED), "{printed}");
    assert!(
        !printed.contains("Took") && !printed.contains("$0.0"),
        "the conversation, not the pane: {printed}"
    );
}

#[test]
fn a_pi_stopped_on_a_question_it_asked_reads_waiting_by_its_own_word() {
    // An extension's prompt is the one stop pi reports: opening it reports
    // the question, closing it resumes the turn. The choices are only on the
    // pane, and a reading adds them beside the reported question.
    let amx = Harness::new();
    let id = "fix-login-a1b";
    start(&amx, id, "reports-a-question");

    let agent = amx.until("the question to reach the record", || {
        let agent = status(&amx, id);
        (agent["question"] == json!("Run echo hi?")).then_some(agent)
    });
    assert_eq!(agent["state"], "waiting", "{agent}");
    assert_eq!(agent["evidence"], "hooks", "{agent}");

    let out = amx.amx(&["send", id, "and now the linter"]);
    assert_eq!(
        out.status.code(),
        Some(2),
        "a message typed at a question would answer it: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let agent = amx.until("the prompt to close", || {
        let agent = status(&amx, id);
        (agent["state"] == json!("working")).then_some(agent)
    });
    assert_eq!(
        agent["question"],
        Value::Null,
        "and nothing is outstanding once it has: {agent}"
    );
}

#[test]
fn a_pi_driven_through_its_own_gates_offers_the_question_it_is_on() {
    // pi fires no event for four screens (`/login`, `/trust`, `/model` and
    // the startup trust gate), so their questions reach the record only from
    // the pane. When the vendor's `Hooks` decided whether a reading may
    // replace a question, every question on a pi counted as reported and the
    // first one read stuck: on pi 0.85.1 an agent on the login box still
    // offered the startup gate's question while `state` and `rule` moved on.
    let amx = Harness::new();
    let id = "fix-login-a1b";
    let gates = timeline(
        &amx,
        "walks-its-own-gates",
        // Three of pi's screens on one pane, with no hook for any of them.
        "screen login\nsleep 8000\nscreen trust\nsleep 8000\nscreen dialog\nsleep 600000\n",
    );
    start_playing(&amx, id, &gates);

    // Silent for an hour, so only the screen is read. A reading does not
    // refresh these stamps, so every look below reads the pane.
    amx.set_state(
        id,
        json!({ "state": "starting", "since": 1, "last_event": 1 }),
    );

    // Poll `status`: a look is what writes the question to the record.
    let agent = amx.until("the login box to reach the record", || {
        let agent = status(&amx, id);
        (agent["question"] == json!("Enter Cerebras API key")).then_some(agent)
    });
    assert_eq!(agent["state"], "waiting", "{agent}");
    assert_eq!(
        agent["rule"], "login",
        "pi's own rule, out of pi's own document: {agent}"
    );

    // The trust selector's question is its title plus the directory. The
    // login box's question used to stay on the record here.
    let dir = amx.home().to_string_lossy().to_string();
    let parent = dir.rsplit_once('/').expect("a parent folder").0.to_string();
    let agent = amx.until("the trust selector to take the pane", || {
        let agent = status(&amx, id);
        (agent["rule"] == json!("project_trust")).then_some(agent)
    });
    assert_eq!(agent["state"], "waiting", "{agent}");
    assert_eq!(
        agent["question"],
        json!(format!("Project trust {dir}")),
        "this screen's own question, and nothing of the box before it: {agent}"
    );
    assert_eq!(
        agent["options"],
        json!([
            "Trust",
            format!("Trust parent folder ({parent})"),
            "Do not trust"
        ]),
        "with the three rows of the run the arrow is in under it: {agent}"
    );
    assert_eq!(
        amx.state(id)["question"]["text"],
        json!(format!("Project trust {dir}")),
        "written down, rather than concluded and forgotten"
    );

    // Each later question replaces the one before.
    let agent = amx.until("the dialog to replace it", || {
        let agent = status(&amx, id);
        (agent["question"] == json!("Run echo hi?")).then_some(agent)
    });
    assert_eq!(agent["rule"], "dialog", "{agent}");
    assert_eq!(
        amx.state(id)["question"]["text"],
        json!("Run echo hi?"),
        "and the record is what a caller reads it from"
    );
}

#[test]
fn a_pi_that_reported_its_question_keeps_its_own_words_over_a_reading() {
    // The screen does not always win: `ui_prompt_start` carries the title the
    // caller passed, while the pane shows whatever pi drew of it. A reading
    // must not replace the reported words.
    let amx = Harness::new();
    let id = "fix-login-a1b";
    let reported = timeline(
        &amx,
        "reports-then-holds",
        // The pane says `Run echo hi?` while the hook reports a different
        // sentence.
        "screen boot\nhook session_start {}\nsleep 50\nhook agent_start {}\nscreen dialog\n\
         hook ui_prompt_start {\"kind\":\"confirm\",\"message\":\"Allow the bash tool?\"}\n\
         sleep 600000\n",
    );
    start_playing(&amx, id, &reported);

    let agent = amx.until("the reported question to reach the record", || {
        let agent = status(&amx, id);
        (agent["question"] == json!("Allow the bash tool?")).then_some(agent)
    });
    assert_eq!(agent["state"], "waiting", "{agent}");

    // Age the record past `FRESH` so a reader looks at the pane. The reading
    // keeps the hook's words and adds the choices.
    let mut aged = amx.state(id);
    aged["since"] = json!(1);
    aged["last_event"] = json!(1);
    amx.set_state(id, aged);

    let agent = amx.until("a reading to claim the dialog", || {
        let agent = status(&amx, id);
        (agent["rule"] == json!("dialog")).then_some(agent)
    });
    assert_eq!(agent["state"], "waiting", "{agent}");
    assert_eq!(
        agent["question"],
        json!("Allow the bash tool?"),
        "the vendor's own words, and not the reading of the pane that claimed \
         the same screen: {agent}"
    );
    assert_eq!(
        amx.state(id)["question"],
        json!({
            "text": "Allow the bash tool?",
            "options": ["Allow once", "Allow always", "Deny"],
            "walked": true,
            "reported": true,
            "kind": "question",
        }),
        "written down whole, the way claude's reported questions always were: \
         the vendor's words with the choices the screen filled in under them, \
         and `reported` to say which half came from where"
    );
}

#[test]
fn a_message_a_pi_takes_is_confirmed_by_its_own_word() {
    // send waits for the vendor to confirm the message. pi confirms with the
    // `agent_start` its extension reports when the message's turn begins.
    let amx = Harness::new();
    let id = "fix-login-a1b";
    start(&amx, id, "reports-a-message");

    amx.until("the first turn to settle", || {
        (status(&amx, id)["result"] == json!("the tests pass now")).then_some(())
    });

    let out = amx.amx(&["send", id, "and now the linter"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "the message landed and the turn ran: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.stderr.is_empty(),
        "and nothing to warn about: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let kinds = amx.event_kinds(id);
    let sent = kinds
        .iter()
        .rposition(|kind| kind == SENT)
        .unwrap_or_else(|| panic!("the message on the record: {kinds:?}"));
    assert!(
        kinds[sent..].iter().any(|kind| kind == "agent_start"),
        "the turn pi says the message began: {kinds:?}"
    );
    assert!(
        !kinds.iter().any(|kind| kind == READ_PROMPT),
        "and no reading placed it: {kinds:?}"
    );
    assert_eq!(amx.state(id)["seq"], 1, "the send is on the record");
}

#[test]
fn a_message_a_pi_holds_behind_its_turn_is_queued_until_it_goes_in() {
    // A message sent to pi mid-turn is steered: held until the turn reaches
    // it and delivered with no new `agent_start`. pi's `message_start` is the
    // only sign it went in, and must clear the queued line in `amx status`.
    let amx = Harness::new();
    let id = "fix-login-a1b";
    start(&amx, id, "takes-a-message-mid-turn");
    amx.until_state(id, "working");

    let out = amx.amx(&["send", id, "hi"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "a send to a working agent is queued, not failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let report = |amx: &Harness| {
        let out = amx.amx(&["status", id]);
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    amx.until("the message held on the record", || {
        report(&amx).contains("queued    hi").then_some(())
    });
    amx.until("the message to go in", || {
        (!report(&amx).contains("queued")).then_some(())
    });
    let kinds = amx.event_kinds(id);
    assert!(
        kinds.iter().any(|kind| kind == "message_start"),
        "pi's word for it, on the record: {kinds:?}"
    );
    assert_eq!(
        amx.state(id)["state"],
        "working",
        "and the turn goes on as it was"
    );
}

#[test]
fn a_message_that_starts_no_turn_on_a_pi_is_a_send_that_says_so() {
    // With no turn start reported, amx cannot confirm delivery, and a caller
    // told it succeeded would wait out its deadline for nothing. Here the
    // pane stays at its prompt and nothing reports.
    let amx = Harness::new();
    let id = "fix-login-c3d";
    start(&amx, id, "takes-a-turn");
    let pane = amx.pane_of(id);

    amx.until("the pi to be at its prompt", || {
        row_of(&drawn(&amx, &pane), "Took").is_some().then_some(())
    });
    amx.set_state(id, json!({ "state": "idle", "since": 1, "last_event": 1 }));

    let waited = Instant::now();
    let out = amx.amx(&["send", id, "and now the linter"]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "a success here is a caller waiting on a turn that never started: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        waited.elapsed() < Duration::from_secs(CONFIRM * 3),
        "and it says so on its own patience rather than sitting there: {:?}",
        waited.elapsed()
    );
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(said.contains("did not start working"), "{said}");

    assert!(
        amx.capture(&pane).contains("and now the linter"),
        "the text reached the pane; what did not happen is a turn starting"
    );
}

#[test]
fn a_result_after_a_message_ends_on_the_turn_pi_reports_ending() {
    // result waits for a turn that ended after the last message. pi reports
    // one with `agent_settled`, which carries the answer.
    let amx = Harness::new();
    let id = "fix-login-a1b";
    start(&amx, id, "reports-a-message");

    amx.until("the first turn to settle", || {
        (status(&amx, id)["result"] == json!("the tests pass now")).then_some(())
    });

    let out = amx.amx(&["send", id, "and now the linter"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "the message landed and the turn ran: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let out = amx.amx(&["result", id, "--timeout", "60"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "the turn ended, so the answer is on stdout: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let said = String::from_utf8_lossy(&out.stdout);
    assert_eq!(said.trim(), "the linter is clean");
    assert!(
        !said.contains("the tests pass now"),
        "and not the answer of the turn before the message: {said}"
    );

    let kinds = amx.event_kinds(id);
    let sent = kinds
        .iter()
        .rposition(|kind| kind == SENT)
        .unwrap_or_else(|| panic!("the message on the record: {kinds:?}"));
    assert!(
        kinds[sent..].iter().any(|kind| kind == "agent_settled"),
        "the turn pi says ended is the one after the message: {kinds:?}"
    );
    assert!(
        !kinds.iter().any(|kind| kind == READ_TURN_END),
        "and no reading placed it: {kinds:?}"
    );
}

#[test]
fn a_message_leaves_result_waiting_beside_the_answer_it_will_not_serve() {
    // The recorded answer belongs to the turn before the message, and after a
    // message `result` must wait for the next turn. Nothing here reports a
    // turn ending, so the wait ends on the caller's deadline.
    let amx = Harness::new();
    let id = "fix-login-c3d";
    start(&amx, id, "takes-a-turn");
    let pane = amx.pane_of(id);

    amx.until("the turn to be over", || {
        row_of(&drawn(&amx, &pane), "Took").is_some().then_some(())
    });
    let at = now();
    amx.set_state(
        id,
        json!({
            "state": "idle",
            "since": at,
            "last_event": at,
            "result": ANSWERED,
            "source": "payload",
        }),
    );

    let out = amx.amx(&["result", id, "--timeout", "30"]);
    assert!(
        out.status.success(),
        "amx result: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), ANSWERED);

    // send records the message before typing it. The stand-in reports
    // nothing.
    amx.amx(&["send", id, "and the tests?"]);

    let out = amx.amx(&["result", id, "--timeout", "1"]);
    assert_eq!(
        out.status.code(),
        Some(3),
        "the caller's own deadline: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.stdout.is_empty(),
        "and nothing on stdout, since exit 0 is the only thing that means there \
         is an answer on it: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert_eq!(
        amx.state(id)["result"],
        ANSWERED,
        "while the answer is still where it was put"
    );
}

#[test]
fn logs_cut_the_furniture_pi_drew_and_print_the_work_above_it() {
    // The walk that strips vendor chrome used claude's anchors on every pane,
    // and pi's box, working directory and stats line match none of them, so
    // `amx logs` printed pi's chrome. The record's vendor now picks the
    // anchors, as it picks the rules.
    let amx = Harness::new();
    let id = "fix-login-a1b";
    start(&amx, id, "takes-a-turn");
    let pane = amx.pane_of(id);

    let rows = amx.until("the turn to be over", || {
        let rows = drawn(&amx, &pane);
        row_of(&rows, "Took").is_some().then_some(rows)
    });

    // The agent's output is everything above pi's box.
    let top = *borders(&rows)
        .first()
        .unwrap_or_else(|| panic!("pi's composer box: {rows:?}"));
    let mut work: Vec<String> = rows[..top].to_vec();
    while work.last().is_some_and(String::is_empty) {
        work.pop();
    }

    // Ask for exactly those rows: pi repaints its pane, so any more would come
    // from earlier screens in tmux's history.
    let out = amx.amx(&["logs", id, "--lines", &work.len().to_string()]);
    assert!(
        out.status.success(),
        "amx logs: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let printed: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|row| row.trim_end().to_string())
        .collect();
    assert_eq!(
        printed, work,
        "the rows the agent earned, and none of the box, working directory or \
         stats line pi drew under them"
    );
}

#[test]
fn logs_cut_the_status_line_pi_spins_whatever_it_says_on_it() {
    // The status line is pi's chrome whatever its message. The walk matched
    // only `Working...`, so `amx logs` on a compacting turn printed pi's
    // compaction line. On 0.85.1 an extension's message is in the top border
    // and is cut with the box.
    for (what, scenario, message, _) in OTHER_STATUS_LINES {
        let amx = Harness::new();
        let id = "fix-login-a1b";
        start(&amx, id, scenario);
        let pane = amx.pane_of(id);

        let rows = amx.until("the status line to be drawn", || {
            let rows = drawn(&amx, &pane);
            row_of(&rows, message).is_some().then_some(rows)
        });

        // The agent's output is everything above the status line.
        let line = row_of(&rows, message).expect("the status line");
        let mut work: Vec<String> = rows[..line].to_vec();
        while work.last().is_some_and(String::is_empty) {
            work.pop();
        }

        let out = amx.amx(&["logs", id, "--lines", &work.len().to_string()]);
        assert!(
            out.status.success(),
            "amx logs: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let printed: Vec<String> = String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(|row| row.trim_end().to_string())
            .collect();
        assert_eq!(
            printed, work,
            "{what}: the rows the agent earned, and none of the status line \
             or the chrome pi drew under it"
        );
    }
}

#[test]
fn doctor_offers_the_trust_key_to_a_pi_stopped_on_its_folder_trust_screen() {
    // doctor checks whether amx could have answered the gate, which it now
    // can for pi, so the remedy names the `trust` key instead of only saying
    // to attach and look.
    let amx = Harness::new();
    let id = "fix-login-a1b";
    start(&amx, id, "stops-on-trust");
    let pane = amx.pane_of(id);

    amx.until("the trust question to be drawn", || {
        row_of(&drawn(&amx, &pane), "Project trust")
    });
    // Silent for an hour with nothing outstanding, so only the screen is read.
    amx.set_state(
        id,
        json!({ "state": "starting", "since": 1, "last_event": 1 }),
    );

    let printed = doctor_fix(&amx, "\n");
    let (ok, line) = check_line(&printed, "gate");
    assert!(!ok, "the agent is stopped in front of its work: {line}");
    assert!(line.contains(id), "{line}");
    assert!(
        printed.contains("trust = true"),
        "the key amx would have answered it with: {printed}"
    );
}

/// The path of pi's opt-in subagent tool extension under home.
fn pi_tool(amx: &Harness) -> PathBuf {
    amx.home().join(".pi/agent/extensions/amx-subagent.ts")
}

/// doctor's `hooks pi:` lines in print order, each with whether it passed.
///
/// There can be one per extension file, so a test picks the line naming its
/// file.
fn pi_hooks_lines(printed: &str) -> Vec<(bool, String)> {
    printed
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let verdict = fields.next()?;
            let named = fields.next()? == "hooks" && fields.next()? == "pi:";
            named.then(|| (verdict == "ok", line.to_string()))
        })
        .collect()
}

#[test]
fn setup_writes_pis_subagent_tool_only_when_it_is_asked_for() {
    // `amx setup pi` installs only the reporting extension. `--subagent` opts
    // into the tool, written as a separate file that pi loads on its own.
    let amx = Harness::new();
    let hook = amx.home().join(".pi/agent/extensions/amx.ts");
    let tool = pi_tool(&amx);

    let out = amx.amx(&["setup", "pi"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(hook.exists(), "the reporting wire is written");
    assert!(!tool.exists(), "and nothing was opted into");

    let out = amx.amx(&["setup", "pi", "--subagent"]);
    let printed = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(out.status.success(), "{printed}");
    assert!(
        printed.contains(&tool.display().to_string()),
        "it names the file it wrote: {printed}"
    );
    let written = std::fs::read_to_string(&tool).expect("the tool");
    assert!(written.starts_with("// installed by amx\n"), "{written}");
    assert!(
        written.contains("registerTool") && written.contains("\"subagent\""),
        "it gives the agent the tool: {written}"
    );
    assert!(
        written.contains("params.role") && written.contains("\"--role\""),
        "and it can ask for a role by name: {written}"
    );
    assert!(
        !written.contains("\"_hook\""),
        "the tool reports nothing itself: {written}"
    );

    let out = amx.amx(&["setup", "pi", "--subagent"]);
    let printed = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(out.status.success(), "{printed}");
    assert!(printed.contains("nothing to do"), "{printed}");

    let out = amx.amx(&["uninstall"]);
    let printed = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(out.status.success(), "{printed}");
    assert!(printed.contains(&tool.display().to_string()), "{printed}");
    assert!(!hook.exists() && !tool.exists(), "both went");
}

#[test]
fn setup_refuses_the_subagent_flag_for_a_vendor_that_carries_none() {
    // The flag is accepted for every vendor, so one without a subagent tool
    // refuses it and writes nothing.
    let amx = Harness::new();

    let out = amx.amx(&["setup", "claude", "--subagent"]);
    let printed = String::from_utf8_lossy(&out.stdout).into_owned();
    assert_eq!(out.status.code(), Some(64), "{printed}");
    assert!(printed.contains("no subagent"), "{printed}");
    assert!(
        printed.contains("pi"),
        "it names who carries one: {printed}"
    );
    assert_eq!(
        std::fs::read_dir(amx.home()).unwrap().count(),
        0,
        "and nothing under the home was written"
    );
}

#[test]
fn doctor_judges_the_opt_in_wire_only_where_it_stands() {
    // A missing tool file means nobody opted in, which is not a fault. A
    // stale one is: amx wrote it, and the verb it calls has changed.
    let amx = Harness::new();
    amx.config("agent = \"pi\"\n");
    let tool = pi_tool(&amx);

    let out = amx.amx(&["setup", "pi", "--subagent"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let printed = String::from_utf8_lossy(&amx.amx(&["doctor"]).stdout).into_owned();
    let named = pi_hooks_lines(&printed)
        .into_iter()
        .find(|(_, line)| line.contains("amx-subagent.ts"))
        .unwrap_or_else(|| panic!("doctor said nothing about the tool:\n{printed}"));
    assert!(named.0, "the tool this amx ships is green: {}", named.1);

    // A file from an older amx fails, and the remedy is the command that
    // rewrites it.
    std::fs::write(&tool, "// installed by amx\n// an older one\n").unwrap();
    let printed = String::from_utf8_lossy(&amx.amx(&["doctor"]).stdout).into_owned();
    let named = pi_hooks_lines(&printed)
        .into_iter()
        .find(|(_, line)| line.contains("amx-subagent.ts"))
        .unwrap_or_else(|| panic!("doctor said nothing about the tool:\n{printed}"));
    assert!(!named.0, "{}", named.1);
    assert!(
        printed.contains("amx setup pi --subagent"),
        "and the remedy names the flag: {printed}"
    );

    std::fs::remove_file(&tool).unwrap();
    let printed = String::from_utf8_lossy(&amx.amx(&["doctor"]).stdout).into_owned();
    assert!(
        !printed.contains("amx-subagent.ts"),
        "an opt-in file that was never asked for is nobody's fault: {printed}"
    );
    let reporting = pi_hooks_lines(&printed)
        .into_iter()
        .find(|(_, line)| line.contains("amx.ts"))
        .unwrap_or_else(|| panic!("doctor said nothing about pi's wire:\n{printed}"));
    assert!(
        reporting.0,
        "and the reporting wire is still judged: {}",
        reporting.1
    );
}
