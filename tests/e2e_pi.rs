//! Driving the suite against a pi that is not pi.
//!
//! pi is the first entry in the table to declare a start flag, so it is the
//! first vendor amx hands a session id of its own choosing instead of waiting
//! to be told one. That id is what everything here is about: that it reaches
//! the argv a real pane runs, that the record holds it from the moment the
//! pane exists, that a resume comes back onto the same one, and that a branch
//! asks for it beside a new one in the same argv.
//!
//! The vendor is `tests/mock_pi/pi`, reached through the PATH. amx keys its
//! table by the program an agent command runs, so `--agent pi` with that
//! directory in front of the PATH is the whole of what makes these agents pi's
//! — and the only way to drive its entry on a machine with no pi on it.
//!
//! pi reports nothing through hooks, which is the other half of why this file
//! exists. There is no payload to assert on and no `meta.transcript` to read:
//! what the vendor was asked for is on its pane and nowhere else, so that is
//! where these read it.
//!
//! Which is why the stand-in paints a screen in one write, and why no test
//! below waits for two halves of one to arrive. Half a repaint is a pane pi
//! never drew, and a test that polled until the other half landed would be
//! agreeing with the screen it wanted instead of asserting the screen there
//! is: every wait here settles on one anchor, and the rest of the screen is
//! read off that same capture.

mod common;

use common::Harness;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// The task every agent here is started on.
const TASK: &str = "fix the login bug";

/// Where the stand-in and its scenarios live.
fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/mock_pi")
}

fn scenario(name: &str) -> PathBuf {
    fixtures()
        .join("scenarios")
        .join(format!("{name}.scenario"))
}

/// A PATH with the stand-in's directory in front of it, which is what makes
/// `pi` a program this machine has at all.
fn path_to_pi() -> String {
    let ours = fixtures().to_string_lossy().into_owned();
    match std::env::var("PATH") {
        Ok(rest) => format!("{ours}:{rest}"),
        Err(_) => ours,
    }
}

/// Run amx with pi on its PATH and the stand-in ready to play `scenario`.
///
/// Both ride the environment rather than the command line because that is how
/// they reach the pane: a spawn snapshots the environment it was run with, and
/// the pane is started from that snapshot.
fn amx_with_pi(amx: &Harness, scenario_name: &str, args: &[&str]) -> std::process::Output {
    amx.amx_command(args)
        .env("PATH", path_to_pi())
        .env("MOCK_PI_SCENARIO", scenario(scenario_name))
        .output()
        .expect("running amx")
}

/// Start an agent the way a person starts one, on the vendor amx knows as pi.
fn start(amx: &Harness, id: &str, scenario: &str) {
    let out = amx_with_pi(
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

/// The conversations the two vendors name in the terminal an adoption is
/// typed in: the claude somebody is working in, and the pi they started from
/// inside it.
const A_CLAUDE: &str = "4c1e8b73-2f60-4a15-9d38-7e2b6c0f9a54";
const THEIR_PI: &str = "9f3c1d20-5a44-4e7b-8c19-6d0a2b5f7e31";

/// A pi somebody started themselves, in a pane amx never opened, stopped on
/// the dialog it raises. Answers with that pane.
///
/// Started under the name that makes it pi: tmux answers for a pane with the
/// program its process was started as, and a script's is the shell named on
/// its shebang line, so the shell that reads the stand-in is reached through a
/// link called `pi`. Nothing here goes through the PATH the way `amx new
/// --agent pi` has to, because what is in a pane amx did not open is whatever
/// somebody ran.
fn a_pi_started_by_hand(amx: &Harness) -> String {
    let named_pi = amx.home().join("pi");
    std::os::unix::fs::symlink("/bin/sh", &named_pi).expect("a shell called pi");
    let scenario = format!("MOCK_PI_SCENARIO={}", scenario("asks-a-question").display());
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
        &scenario,
        &named_pi,
        &stand_in,
    ]);

    // The hint row pi draws under every dialog, which is the anchor its own
    // document reads that screen by. A capture taken before it is painted is a
    // different screen, and adoption reads the pane once.
    amx.until("pi's dialog on the pane", || {
        amx.capture(&pane).contains("↑↓ navigate").then_some(())
    });
    pane
}

/// `amx adopt`, typed in that pane by a command the vendors have told what
/// they told it.
///
/// The suite is run from inside somebody's own agent often enough that a
/// vendor's session variable is already in this process's environment, so both
/// of them are cleared and only what a caller names is put back.
fn adopt(amx: &Harness, id: &str, pane: &str, named: &[(&str, &str)]) -> std::process::Output {
    amx.amx_command(&["adopt", "--name", id, "--task", TASK])
        .env_remove("CLAUDE_CODE_SESSION_ID")
        .env_remove("PI_SESSION_ID")
        .env("TMUX_PANE", pane)
        .envs(named.iter().copied())
        .output()
        .expect("running amx adopt")
}

/// Run the stand-in itself, with the home a pi would keep its sessions under
/// pinned to this harness.
///
/// For the one argv amx will not build. `--no-session` is on pi's conflicts
/// list, so amx never mints an id beside it, and what pi does when somebody
/// else writes both is still the fixture's to get right.
fn stand_in(amx: &Harness, args: &[&str]) -> std::process::Output {
    std::process::Command::new(fixtures().join("pi"))
        .args(args)
        .env("HOME", amx.home())
        .env("MOCK_PI_SCENARIO", scenario("one-screen"))
        .output()
        .expect("running the stand-in")
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

/// The same agent as a listing has it, which is one look taken by a process
/// that prints its table and exits.
fn listed(amx: &Harness, id: &str) -> Value {
    let out = amx.amx(&["ls", "--json"]);
    assert!(
        out.status.success(),
        "amx ls: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let rows: Vec<Value> = serde_json::from_slice(&out.stdout).expect("the listing is json");
    rows.into_iter()
        .find(|row| row["id"] == id)
        .unwrap_or_else(|| panic!("a row for {id}"))
}

/// How long a screen must hold still before a quiescent rule may end a turn
/// that is on the record as running: `rules::SETTLED_LOOKS` seconds, which is
/// what that many looks at a look a second always meant.
const SETTLED: u64 = 30;

/// The clock every stamp on a record is kept in.
fn epoch() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("a clock set after 1970")
        .as_secs()
}

/// Everything the stand-in has said in this pane, including what has scrolled
/// off it.
///
/// pi repaints its pane rather than appending to it, and so does the stand-in,
/// so how it was called is in the history rather than on the screen. This is
/// the harness's own capture with the history asked for as well.
fn said_in(amx: &Harness, pane: &str) -> String {
    amx.tmux(&["capture-pane", "-p", "-J", "-S", "-", "-t", pane])
}

/// Wait for the line the stand-in opens with, whichever of the two it is.
fn until_said(amx: &Harness, id: &str, opening: &str) -> String {
    let pane = amx.pane_of(id);
    amx.until(&format!("the vendor to say {opening}"), || {
        said_in(amx, &pane)
            .lines()
            .find(|line| line.starts_with(opening))
            .map(str::to_string)
    })
}

/// How the vendor was called, once it has said so.
fn argv_of(amx: &Harness, id: &str) -> String {
    until_said(amx, id, "argv:")
}

/// What the vendor did with the session it was handed, once it has said so.
fn session_of(amx: &Harness, id: &str) -> String {
    until_said(amx, id, "session:")
}

/// The file the stand-in keeps a session in, where pi keeps one under the
/// person's home.
fn session_file(amx: &Harness, session: &str) -> PathBuf {
    amx.home()
        .join(".pi/sessions")
        .join(format!("{session}.jsonl"))
}

/// What is on the pane now, with the blank rows a screen is padded out with
/// taken off the bottom.
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

/// The rows of a text amx printed, with the trailing spaces a capture keeps
/// taken off each of them, so it can be held against what [`drawn`] read.
fn rows_of(text: &str) -> Vec<String> {
    text.lines().map(|row| row.trim_end().to_string()).collect()
}

/// The rows pi's composer border is drawn on, topmost first.
///
/// A row of nothing but the glyph the box is drawn with, twenty columns of it
/// or more, which is the anchor every rule in `assets/screen-rules-pi.toml`
/// stands on.
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

/// Where a row carrying `text` is, when one is.
fn row_of(rows: &[String], text: &str) -> Option<usize> {
    rows.iter().position(|row| row.contains(text))
}

/// Whether a row opens with one of the frames pi spins a status line with.
///
/// Ten glyphs out of Unicode's braille block, cycled at eight a second, and
/// the one thing every status line pi draws carries whatever its message says
/// — which is why `assets/screen-rules-pi.toml` anchors its spinner rule on
/// them rather than on the word one of the four happens to use.
fn spins(row: &str) -> bool {
    row.trim_start()
        .starts_with(|glyph: char| ('\u{2800}'..='\u{28ff}').contains(&glyph))
}

/// The three status lines pi draws that do not say `Working...`, and the
/// scenario that puts each on a pane.
const OTHER_STATUS_LINES: [(&str, &str, &str); 3] = [
    (
        "a compacting turn",
        "compacts-the-context",
        "Compacting context...",
    ),
    ("a retrying turn", "retries-a-turn", "Retrying (1/3)"),
    (
        "a turn under an extension's own working message",
        "renames-the-working-line",
        "Reviewing the diff",
    ),
];

/// The two panes one of pi's own selectors makes, and how far the topmost
/// border a rule can see is from the stats line on each.
///
/// The widget is the same widget and its box is the same three rows. What
/// differs is what else is on the screen: a transcript above it leaves the
/// bottom border of `!cmd`'s own box on the pane, and a rule reading the
/// topmost border it can find starts from that one instead.
const SELECTORS: [(&str, &str, usize); 2] = [
    ("a selector with nothing above it", "opens-a-selector", 5),
    (
        "the same selector under a transcript",
        "opens-a-selector-under-a-transcript",
        7,
    ),
];

/// Doctor's line about one check: whether it passed, and what it said.
fn check_line(printed: &str, name: &str) -> (bool, String) {
    printed
        .lines()
        .find_map(|line| {
            let mut fields = line.split_whitespace();
            let verdict = fields.next()?;
            (fields.next()? == name).then(|| (verdict == "ok", line.to_string()))
        })
        .unwrap_or_else(|| panic!("doctor said nothing about the {name}:\n{printed}"))
}

/// `amx doctor --fix`, with a yes ready for the repair it asks about.
///
/// The yes is the point of the test that types it: a vendor with no hooks is
/// never asked, so the answer is never read and the file is never written.
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

/// Every path under `dir`, so a test can say that a command wrote nothing
/// anywhere rather than nothing to the one file it thought to name.
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

/// Every rule `assets/screen-rules-pi.toml` declares, in the order it declares
/// them.
///
/// Read off the file rather than out of the binary: the document is one rule
/// per `[[rule]]` table and each opens on its own name, so the names are a
/// line-scan away and the test needs no parser of its own.
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

/// Every rule the inventory says it measured a screen reading as.
///
/// The Reads column is the last cell of every table row in
/// `docs/pi-screens.md`, and a rule that claimed a screen is written in bold
/// there — `**`dialog`**` — which is what tells a verdict from the other
/// backticked words in the same cell.
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

#[test]
fn the_inventory_measured_a_screen_for_every_rule_pi_has() {
    // `docs/pi-screens.md` is the coverage half of pi's document: the whole
    // list of screens the vendor can put on a pane, and what each one reads as.
    // What it is for is knowing which screens the rules cover and which they
    // walk past, and that only holds while the two are read together — a rule
    // landing with no row against it is a rule whose coverage nobody measured,
    // and a row naming a rule the document dropped is a verdict nobody can get
    // any more.
    //
    // Which rules those are, and in which order they sit, is asserted in
    // `src/rules.rs` and only there. This is the other question: that every one
    // of them was measured against a screen somebody drove.
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
    // The flag was built against a table where no entry declared one, so
    // nothing could say the flag and the id reach a pane rather than only the
    // argv a unit test builds. pi declares one, and this is that proof: the
    // words are read off the vendor that really ran.
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
fn spawn_asks_pi_to_create_the_session_when_there_is_no_file_under_that_id() {
    // `--session-id` is mint-or-open, and this is the mint: nothing on disk
    // answers to the id amx has just minted, so the vendor makes it.
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
    // `--no-session` is the one flag on pi's conflicts list that is not a
    // refusal. The vendor takes it, runs the turn and keeps the conversation
    // in memory, so what amx owes a person who typed it is silence: no minted
    // id beside it, and no record offering back a conversation that was never
    // written down.
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
    // pi's folder-trust screen is answered with a word on the argv of the pane
    // rather than an entry in a file, so the proof is read off the vendor that
    // really ran. Into a repository rather than a bare directory, because that
    // is what cuts a worktree and so reaches the other half of the same config
    // key: the store that half writes is claude's own file, and a pi agent is
    // not something to write it for.
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
    // The key is the whole of the consent. Without it the flag is one nobody
    // asked for, and what pi loads out of the repository it was pointed at is
    // still pi's own question to put to whoever is at the keyboard.
    let amx = Harness::new();
    let id = "fix-login-a1b";
    start(&amx, id, "takes-a-turn");

    let called = argv_of(&amx, id);
    assert!(!called.contains("--approve"), "{called}");
}

#[test]
fn the_stand_in_parts_the_flags_pi_refuses_from_the_one_it_throws_away() {
    // The stand-in is asked directly here, because amx will not build this
    // argv: `--no-session` is on the conflicts list, so no id is ever minted
    // beside it. What the fixture has to get right is the difference between
    // the six. Five are an exit. The sixth takes the id and drops it, and a
    // fixture that exited on it too would turn the one failure this flag
    // causes on the real vendor — a conversation amx records that was never on
    // disk — into a loud one no test could ever reach.
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
    // The other half of mint-or-open, and the whole reason a vendor with no
    // hooks can be resumed at all: the same flag carries the same id, and the
    // vendor opens what is already there rather than starting a second
    // conversation nobody asked for.
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
    // pi branches by naming the session to copy on a flag of its own, which
    // leaves the start flag free to carry the copy's own minted id. Both in
    // one argv is what makes a forked pi an agent amx can name afterwards.
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
fn the_stand_in_spins_pis_line_two_rows_above_pis_own_box() {
    // The screen `assets/screen-rules-pi.toml` names `spinner`, drawn the way
    // it was measured: the line, one blank row, and the top of the composer
    // box. Two rows is the whole of what separates the word half the build
    // tools print from the word on the row above this vendor's own chrome.
    let amx = Harness::new();
    let id = "watch-log-c3d";
    start(&amx, id, "works-without-end");
    let pane = amx.pane_of(id);

    // One anchor is waited for and the rest of the screen is read off the same
    // capture. The stand-in paints a screen in one write, so a capture with
    // the spinner on it is a capture with the whole screen on it, and the box
    // below is an assertion rather than a second thing to wait for. Waiting
    // for both would be waiting for a pane that was never drawn to turn into
    // one that was, which is the reading this whole file exists to rule out.
    let rows = amx.until("the turn to be under way", || {
        let rows = drawn(&amx, &pane);
        row_of(&rows, "Working...").is_some().then_some(rows)
    });
    let spinner = row_of(&rows, "Working...").expect("the line pi spins");
    let top = *borders(&rows)
        .first()
        .unwrap_or_else(|| panic!("pi's composer box: {rows:?}"));
    assert_eq!(
        top - spinner,
        2,
        "the line, one blank row, and the top of the box: {rows:?}"
    );
}

#[test]
fn the_stand_in_spins_the_status_lines_that_do_not_say_working() {
    // pi has one status line and swaps out which of its four kinds is on it.
    // Compaction and a retry each take the working indicator down and put
    // their own where it was, so `Working...` is off the pane for the whole of
    // either, and `ctx.ui.setWorkingMessage` rewrites the message on the kind
    // that is left. What all three keep is the frame and the row: two above
    // the box's top border with a blank row between, exactly where the working
    // line sits, which is the shape the spinner rule was measured against.
    for (what, scenario, message) in OTHER_STATUS_LINES {
        let amx = Harness::new();
        let id = "watch-log-c3d";
        start(&amx, id, scenario);
        let pane = amx.pane_of(id);

        let rows = amx.until("the status line to be drawn", || {
            let rows = drawn(&amx, &pane);
            row_of(&rows, message).is_some().then_some(rows)
        });

        let line = row_of(&rows, message).expect("the status line");
        let top = *borders(&rows)
            .first()
            .unwrap_or_else(|| panic!("pi's composer box: {rows:?}"));
        assert_eq!(
            top - line,
            2,
            "{what}: the line, one blank row, and the top of the box: {rows:?}"
        );
        assert!(
            spins(&rows[line]),
            "{what}: the row opens with the frame pi spins: {rows:?}"
        );
        assert!(
            !rows.iter().any(|row| row.contains("Working...")),
            "{what}: the word the spinner rule used to stand on is nowhere on \
             the pane: {rows:?}"
        );
        assert_eq!(
            rows.len() - borders(&rows)[1],
            3,
            "{what}: the working directory and the stats line under the box, \
             same as any other screen: {rows:?}"
        );
    }
}

#[test]
fn a_pi_whose_status_line_stopped_saying_working_is_still_working() {
    // Three ways a turn can be under way with the word `Working...` nowhere on
    // the pane, and all three read `unknown` under a rule that stands on that
    // word: a compacting pi is doing work nobody can interrupt usefully, a
    // retrying one is between two provider calls, and a turn under an
    // extension's own message is an ordinary turn with the message rewritten.
    // The frame is what says a turn is running on all three.
    for (what, scenario, message) in OTHER_STATUS_LINES {
        let amx = Harness::new();
        let id = "watch-log-c3d";
        start(&amx, id, scenario);
        let pane = amx.pane_of(id);

        amx.until("the status line to be drawn", || {
            row_of(&drawn(&amx, &pane), message).is_some().then_some(())
        });

        // Aged the way `a_quiet_pi` ages one: nothing heard for an hour, with
        // nothing outstanding, which is where the screen is the only witness
        // there is on this vendor.
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
    // The chrome the other two rules stand on and `src/furniture.rs` walks up
    // over: the box's two borders, and under them the working directory and
    // the stats line, always both and never anything between them and the box.
    let amx = Harness::new();
    let id = "fix-login-a1b";
    start(&amx, id, "takes-a-turn");
    let pane = amx.pane_of(id);

    // The row a finished turn leaves and no other screen carries, waited for
    // on its own: which screen is up is what a wait is for, and how much of it
    // has been painted is not a question this fixture leaves open.
    let rows = amx.until("the turn to be over", || {
        let rows = drawn(&amx, &pane);
        row_of(&rows, "Took").is_some().then_some(rows)
    });

    assert!(
        row_of(&rows, "Working...").is_none(),
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
    // The screen `assets/screen-rules-pi.toml` names `dialog`, and it is drawn
    // the way it was measured: inside the composer box rather than above it,
    // with the same footer under it as every other screen carries, and with
    // the turn that raised it still running over the top. The rule's own
    // anchor is the hint row this stops on, `↑↓ navigate …`, so this is what
    // proves the stand-in draws the shape the rule was measured against rather
    // than only the words.
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
    // And the spinner is still up above the box, because pi raises this dialog
    // from a tool call while the turn is running. Both documented anchors are
    // on this one pane — the hint row the dialog rule stands on and the
    // spinner two rows over the border the spinner rule stands on — which is
    // why the document's order, and not its anchors, is what keeps the spinner
    // rule off a screen that is blocked.
    let spinner =
        row_of(&rows, "Working...").unwrap_or_else(|| panic!("the line pi spins: {rows:?}"));
    assert_eq!(
        top - spinner,
        2,
        "the line, one blank row, and the top of the box: {rows:?}"
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
    // The screens `assets/screen-rules-pi.toml` names `input` and `editor`,
    // drawn the way they were measured: the caller's title two rows under the
    // top of the composer box, what pi is waiting to be typed into below it,
    // and a hint row opening on `enter submit` where the dialog's opens on
    // `↑↓ navigate`. The editor draws a second box for the block it wants,
    // which is the shape that told the two rules apart.
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

        // The title this scenario asks for and no earlier screen in it
        // carries, waited for on its own, with the rest of the screen read off
        // that same capture.
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
        // And nothing is running over it. A caller raises either of these from
        // a turn or between two of them; the fixture draws them the way they
        // were measured, which was with no turn under way.
        assert!(
            row_of(&rows, "Working...").is_none(),
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
    // An extension stops pi three ways and two of them read `unknown`, so a
    // permission gate written with `ctx.ui.input` was a pane amx said nothing
    // about while somebody waited to be typed at. All three block, all three
    // put the caller's own sentence at the top of pi's box, and the row is
    // where a person reads it: the vendor reports through no hooks, so there
    // is no payload the question could arrive in.
    for (what, scenario, rule, question) in [
        ("a choice", "asks-a-question", "dialog", "Run echo hi?"),
        (
            "a line",
            "asks-for-a-line",
            "input",
            "Which branch should I push to?",
        ),
        (
            "a block",
            "asks-for-a-block",
            "editor",
            "Write the commit message",
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

        // Aged the way `a_quiet_pi` ages one: nothing heard for an hour, with
        // nothing outstanding, which is where the screen is the only witness
        // there is on this vendor.
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
            json!([]),
            "pi numbers none of its choices, and two of these have none to \
             number: {agent}"
        );
    }
}

#[test]
fn a_pi_on_the_folder_trust_question_reads_trust_and_not_a_tool_call() {
    // pi draws this in the same box a gated tool call's dialog is drawn in and
    // ends it in the same `↑↓ navigate` hint row, so the dialog rule claimed
    // it and the record said a tool call was waiting on an answer. What kind
    // of thing is being asked is what decides what may be sent back, and this
    // one takes a decision about the tree amx cut rather than a choice off a
    // caller's own menu.
    let amx = Harness::new();
    let id = "fix-login-a1b";
    start(&amx, id, "stops-on-trust");
    let pane = amx.pane_of(id);

    // The title pi draws on this screen and on no other, waited for on its
    // own, with the rest of the screen read off that same capture.
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
    // The hint row this screen ends in, which is the one the dialog rule
    // stands on and the one this rule must not: it says `enter save` where a
    // tool call's says `enter select`, and it is inside the box exactly where
    // the other one is.
    assert!(
        row_of(&rows, "enter save").is_some_and(|hint| top < hint && hint < bottom),
        "the hint row sits inside the box, where the editor usually is: {rows:?}"
    );
    // And nothing is running over it. A person raises this screen before a
    // turn rather than a tool call raising it inside one, so the line pi spins
    // is not on the pane the way it is over a dialog.
    assert!(
        row_of(&rows, "Working...").is_none(),
        "no turn is under way behind this question: {rows:?}"
    );
    assert_eq!(
        rows.len() - bottom,
        3,
        "the working directory and the stats line under it, same as any other \
         screen: {rows:?}"
    );

    // Aged the way `a_quiet_pi` ages one: nothing heard for an hour, with
    // nothing outstanding, which is where the screen is the only witness there
    // is on this vendor.
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
fn the_stand_in_draws_the_gate_pi_puts_in_front_of_a_first_run() {
    // The screen `assets/screen-rules-pi.toml` names `first_time_setup`, and
    // it is the one screen in that document with none of pi's chrome under it:
    // the vendor asks for a theme before it will draw a pane at all, so what
    // is on this one is a box, the vendor's own banner inside it, and nothing
    // else whatsoever.
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
    // The screen `assets/screen-rules-pi.toml` names `login`, drawn the way it
    // was measured: the box where the composer was, the footer under it as on
    // any other screen, and the vendor's title on the row directly under the
    // top border rather than a blank row below it, which is the one way this
    // box is drawn differently from the dialogs a caller raises.
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
    // Neither of these is a turn and neither is a prompt, and both were read
    // as one or the other: the setup gate carries the dialog rule's own hint
    // row, so it was reported as a tool call waiting on an answer, and the
    // login dialog is short enough that the box and the stats line under it
    // added up to `prompt` — a card saying idle over a pi that cannot take a
    // turn until somebody types a key into it.
    for (what, scenario, drawn_row, rule, question) in [
        (
            "the gate a first run stops at",
            "stops-at-setup",
            "Welcome to pi,",
            "first_time_setup",
            "Pick a theme. Detected system appearance: dark",
        ),
        (
            "a pi waiting for a provider's key",
            "stops-on-login",
            "Login to",
            "login",
            "Enter Cerebras API key",
        ),
    ] {
        let amx = Harness::new();
        let id = "fix-login-a1b";
        start(&amx, id, scenario);
        let pane = amx.pane_of(id);

        // The row the vendor draws on this screen and on no other, waited for
        // on its own, with the rest of the screen read off that same capture.
        amx.until("the screen to be drawn", || {
            row_of(&drawn(&amx, &pane), drawn_row)
                .is_some()
                .then_some(())
        });

        // Aged the way `a_quiet_pi` ages one: nothing heard for an hour, with
        // nothing outstanding, which is where the screen is the only witness
        // there is on this vendor.
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
            json!([]),
            "pi numbers none of its choices: {agent}"
        );
    }
}

#[test]
fn the_stand_in_draws_a_selector_in_the_slot_pis_composer_had() {
    // The screen no rule in `assets/screen-rules-pi.toml` is named for, and
    // `docs/pi-screens.md` counts fourteen of them: a widget a person opened,
    // drawn between the composer's own two borders with the working directory
    // and the stats line under them. There is no hint row on it that any rule
    // knows and no title, which is the whole reason it reaches the last rule in
    // the document at all.
    //
    // The distance is what this fixture is for. Five rows separate the topmost
    // border a rule can see from the stats line with nothing above the box, and
    // seven with a transcript above it, because `!cmd` leaves the bottom border
    // of its own box on the pane. The widget did not move.
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
        // And nothing on it that a rule above the last one would stop at: pi
        // spells this widget's keys nowhere on the pane, so the screen falls
        // past every rule that reads a hint row.
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
    // The last rule in pi's document was counted off a composer: an empty box,
    // the working directory and the stats line, four rows. A selector is the
    // same two borders with somebody's list between them, and at five rows and
    // at seven it fell inside a window of eight — so a card said idle over a pi
    // that would take the next keystroke as a menu choice, and `send` would
    // have typed into the widget.
    //
    // The second half is that it said it differently on the same widget. What
    // is above the box is not a fact about the box, and a verdict that turns on
    // how much output has scrolled by is not a reading of the screen at all.
    let mut verdicts = Vec::new();
    for (what, scenario, _) in SELECTORS {
        let amx = Harness::new();
        let id = "fix-login-a1b";
        start(&amx, id, scenario);
        let pane = amx.pane_of(id);

        // The row this widget draws and no earlier screen in the scenario
        // carries, waited for on its own, with the rest of the screen read off
        // that same capture.
        amx.until("the selector to be drawn", || {
            row_of(&drawn(&amx, &pane), "Show images inline")
                .is_some()
                .then_some(())
        });

        // Aged the way `a_quiet_pi` ages one: nothing heard for an hour, with
        // nothing outstanding, which is where the screen is the only witness
        // there is on this vendor.
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
    // Every reader held one document against whatever pane it was handed, and
    // that document was claude's. pi draws not one of claude's anchors, so a pi
    // that had gone quiet read `unknown` with its own prompt plainly on the
    // screen. What picks the document is the command the record kept at the
    // spawn.
    let amx = Harness::new();
    let id = "fix-login-a1b";
    start(&amx, id, "takes-a-turn");
    assert_eq!(amx.meta(id)["agent"], "pi", "the record says which vendor");
    let pane = amx.pane_of(id);

    // The prompt a finished turn leaves, stopped on the row no earlier screen
    // in this scenario carries.
    amx.until("the turn to be over", || {
        row_of(&drawn(&amx, &pane), "Took").is_some().then_some(())
    });

    // Aged the way `e2e_reader` ages one: nothing heard for an hour, with
    // nothing outstanding, which is the state a quiescent rule decides from at
    // once.
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
fn a_pi_that_has_held_still_settles_for_whichever_process_looks_next() {
    // How long a screen had held still was a run of consecutive looks counted
    // in one process's memory, and every verb but the view is a process that
    // prints a line and exits. So `amx ls` and `amx status` counted one look,
    // every time, and pi's quiescent `prompt` rule could never end a turn for
    // either of them: a pi whose turn ended, on a vendor that sends no hook to
    // say so, read `working` for as long as anybody cared to ask.
    //
    // What a look found and when it first found it goes on the record now, so
    // the stillness one process watched is there for the next one to read.
    let amx = Harness::new();
    let id = "fix-login-a1b";
    start(&amx, id, "takes-a-turn");
    let pane = amx.pane_of(id);

    // The prompt a finished turn leaves, stopped on the row no earlier screen
    // in this scenario carries.
    amx.until("the turn to be over", || {
        row_of(&drawn(&amx, &pane), "Took").is_some().then_some(())
    });

    // A turn on the record as running, with nothing heard for an hour: the
    // state the `prompt` rule may not decide from until the screen has held
    // still.
    amx.set_state(
        id,
        json!({ "state": "working", "since": 1, "last_event": 1 }),
    );

    let looked = epoch();
    let out = amx.amx(&["result", id, "--timeout", "1"]);
    assert_eq!(
        out.status.code(),
        Some(3),
        "a screen amx has only just laid eyes on ends no turn: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // What that wait wrote down: the screen it was reading, and when it first
    // saw it.
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

    // The same screen, first seen `SETTLED` seconds ago — aged on the record
    // the way everything about a clock is aged here, rather than waited for.
    let mut aged = state.clone();
    aged["still"]["since"] = json!(since - SETTLED);
    amx.set_state(id, aged);

    // One look, in a process of its own, and it is the look that ends the
    // turn: what the wait before it watched is on the record to be read.
    let row = listed(&amx, id);
    assert_eq!(row["state"], "idle", "{row}");
    assert_eq!(row["evidence"], "screen", "{row}");
    assert_eq!(
        row["rule"], "prompt",
        "pi's own rule, out of pi's own document: {row}"
    );

    // And a screen that changes starts the clock again. The record remembers a
    // screen that is not the one on the pane, so the one on the pane has been
    // there no time at all whatever the stamp beside it says.
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
    // The finding at the end of `docs/vendors.md`: `adopt` read the
    // environment in table order, so a pi started from a terminal that already
    // had claude's session id in it was adopted as claude — claude's id on a
    // record no pi will ever report under, and claude's document reading a
    // pane pi drew.
    let amx = Harness::new();
    let pane = a_pi_started_by_hand(&amx);

    // Claude's variable alone first, which is the terminal's own and says
    // nothing about what is running in this pane.
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

    // Both variables, which is what a pi started from that terminal really
    // carries: its own session id, and the one it inherited.
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
        amx.state(id)["question"],
        "Run echo hi?",
        "the sentence the caller passed, off the pane it is drawn on"
    );
}

#[test]
fn a_turn_that_ends_on_a_pi_leaves_what_the_pane_said_on_the_record() {
    // pi reports through no hooks and keeps no conversation amx can read back,
    // so nothing was ever going to write down what one of its turns answered:
    // `amx result` said it had captured none and every pi row on the wall
    // carried a blank column, while the answer sat on the pane in front of
    // everybody. The reader that has just read that screen as a finished turn
    // is the only thing that will ever be looking at it, so what it read goes
    // on the record — the rows the agent earned, with the vendor's own
    // furniture cut off the bottom the way `amx logs` and the card cut it.
    let amx = Harness::new();
    let id = "fix-login-a1b";
    start(&amx, id, "takes-a-turn");
    let pane = amx.pane_of(id);

    // The row a finished turn leaves and no earlier screen in this scenario
    // carries, waited for on its own, with the rest of the screen read off that
    // same capture.
    let rows = amx.until("the turn to be over", || {
        let rows = drawn(&amx, &pane);
        row_of(&rows, "Took").is_some().then_some(rows)
    });

    // Everything above pi's own box, which is the whole of what the agent
    // earned on this screen and the whole of what a reading of it is worth.
    let top = *borders(&rows)
        .first()
        .unwrap_or_else(|| panic!("pi's composer box: {rows:?}"));
    let mut work: Vec<String> = rows[..top].to_vec();
    while work.last().is_some_and(String::is_empty) {
        work.pop();
    }

    // Aged the way `a_quiet_pi` ages one: nothing heard for an hour, with
    // nothing outstanding, which is where the screen is the only witness there
    // is on this vendor.
    amx.set_state(
        id,
        json!({ "state": "starting", "since": 1, "last_event": 1 }),
    );

    let agent = status(&amx, id);
    assert_eq!(agent["state"], "idle", "{agent}");
    let said = agent["result"]
        .as_str()
        .unwrap_or_else(|| panic!("the answer the turn left: {agent}"));
    assert_eq!(
        rows_of(said),
        work,
        "the rows the agent earned, and none of the box, working directory or \
         stats line pi drew under them"
    );
    assert_eq!(
        agent["source"], "screen",
        "amx's reading of a picture, and the record says which: {agent}"
    );
    assert_eq!(
        amx.state(id)["result"],
        agent["result"],
        "written down, rather than worked out again by whoever asks next"
    );

    // Which is what writing it down is for: the verb a caller waits at hands
    // back the answer instead of saying it captured none.
    let out = amx.amx(&["result", id, "--timeout", "30"]);
    assert!(
        out.status.success(),
        "amx result: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(rows_of(&String::from_utf8_lossy(&out.stdout)), work);

    // And the row on the wall carries the last of those rows, where a pi row
    // carried nothing at all. The first of them is the prompt somebody typed
    // and the rows between are the tool call it ran: what a turn leaves for
    // somebody to read is at the bottom of a transcript, not the top.
    let out = amx.amx(&["ls"]);
    let printed = String::from_utf8_lossy(&out.stdout);
    let row = printed
        .lines()
        .find(|row| row.contains(id))
        .unwrap_or_else(|| panic!("a row for {id}: {printed}"));
    let last = work.last().expect("a turn that left something");
    assert!(
        row.contains(last.trim()),
        "the last thing said on the screen: {row}"
    );
    assert!(
        !row.contains(work[0].trim()),
        "and not the first row of the transcript over it: {row}"
    );
}

#[test]
fn a_message_leaves_result_waiting_beside_the_answer_it_will_not_serve() {
    // What the pane keeps is an answer to the turn amx watched end, and the
    // turn `result` waits for after a message is the one after it. Only a Stop
    // event says a turn ended, pi sends none, so the wait runs to its deadline
    // with the earlier answer on the record beside it. That is the hooks gap
    // costing a verb more than an empty answer, which is the one thing
    // docs/vendors.md says a partial entry has to write down.
    let amx = Harness::new();
    let id = "fix-login-c3d";
    start(&amx, id, "takes-a-turn");
    let pane = amx.pane_of(id);

    amx.until("the turn to be over", || {
        row_of(&drawn(&amx, &pane), "Took").is_some().then_some(())
    });
    amx.set_state(
        id,
        json!({ "state": "starting", "since": 1, "last_event": 1 }),
    );

    // Before the message, the reading is the answer and the verb hands it back.
    let said = status(&amx, id)["result"].clone();
    assert!(said.is_string(), "the answer the turn left: {said}");
    let out = amx.amx(&["result", id, "--timeout", "30"]);
    assert!(
        out.status.success(),
        "amx result: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // The message goes in front of the agent and is recorded before it is
    // typed. pi takes it and says nothing, which is the send this vendor
    // always gets.
    amx.amx(&["send", id, "and the tests?"]);

    // Now the wait has a turn in front of it that nothing will ever report the
    // end of.
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

    // While the answer the reading wrote is still where it was put. The verb
    // will not serve it as this turn's, which is the whole of why it is
    // refusing: an answer from before the message belongs to the turn before
    // it.
    assert_eq!(amx.state(id)["result"], said);
    assert_eq!(amx.state(id)["source"], "screen");
}

#[test]
fn logs_cut_the_furniture_pi_drew_and_print_the_work_above_it() {
    // The walk that takes a vendor's chrome off a reading held claude's
    // anchors against whatever pane it was handed, and pi's box, working
    // directory and stats line carry not one of them: `amx logs` printed the
    // vendor's own furniture back at somebody asking what the agent had been up
    // to. Which anchors the walk holds is the record's to say, the same way the
    // rules are.
    let amx = Harness::new();
    let id = "fix-login-a1b";
    start(&amx, id, "takes-a-turn");
    let pane = amx.pane_of(id);

    // The row a finished turn leaves and no earlier screen in this scenario
    // carries, waited for on its own, with the rest of the screen read off that
    // same capture.
    let rows = amx.until("the turn to be over", || {
        let rows = drawn(&amx, &pane);
        row_of(&rows, "Took").is_some().then_some(rows)
    });

    // Everything above pi's own box, which is the whole of what the agent
    // earned on this screen.
    let top = *borders(&rows)
        .first()
        .unwrap_or_else(|| panic!("pi's composer box: {rows:?}"));
    let mut work: Vec<String> = rows[..top].to_vec();
    while work.last().is_some_and(String::is_empty) {
        work.pop();
    }

    // Asked for exactly those rows: pi repaints its pane rather than appending
    // to it, so a longer reading is the screens before this one, which tmux
    // keeps in the pane's history.
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
    // The row pi spins is the vendor's, and which of its four messages is on
    // it is not the agent's business either. The walk held the one message,
    // so it cut that row while `Working...` was on it and printed it back at
    // somebody the other three times: `amx logs` on a compacting turn opened
    // with the vendor telling them it was compacting.
    for (what, scenario, message) in OTHER_STATUS_LINES {
        let amx = Harness::new();
        let id = "fix-login-a1b";
        start(&amx, id, scenario);
        let pane = amx.pane_of(id);

        let rows = amx.until("the status line to be drawn", || {
            let rows = drawn(&amx, &pane);
            row_of(&rows, message).is_some().then_some(rows)
        });

        // Everything above the row pi spins, which is the whole of what the
        // agent earned on this screen.
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
    // The other half of the same key, from the other side: an agent already
    // sitting on the screen. doctor asks whether amx could have answered the
    // gate at all, which is now true of pi, so the remedy names the key that
    // makes it never happen again rather than leaving it at attach and look.
    let amx = Harness::new();
    let id = "fix-login-a1b";
    start(&amx, id, "stops-on-trust");
    let pane = amx.pane_of(id);

    amx.until("the trust question to be drawn", || {
        row_of(&drawn(&amx, &pane), "Project trust")
    });
    // Nothing heard for an hour and nothing outstanding, which is where the
    // screen is the only witness there is on a vendor that reports nothing.
    amx.set_state(
        id,
        json!({ "state": "starting", "since": 1, "last_event": 1 }),
    );

    let printed = doctor_fix(&amx, "\n");
    let (ok, line) = check_line(&printed, "setup");
    assert!(!ok, "the agent is stopped in front of its work: {line}");
    assert!(line.contains(id), "{line}");
    assert!(
        printed.contains("trust = true"),
        "the key amx would have answered it with: {printed}"
    );
}

#[test]
fn install_writes_nothing_anywhere_for_a_vendor_that_reports_nothing() {
    // The one repair `--fix` asks about is wiring amx's hooks into the
    // vendor's settings, and pi has no hooks to wire: there are no entries to
    // write, nothing missing, and nothing for a person to agree to. A check
    // that asked would send somebody looking for a fault in their own machine,
    // and a write would leave amx's hooks in a file no pi will ever read.
    let amx = Harness::new();
    amx.config("agent = \"pi\"\n");
    let before = everything_under(amx.home());

    let printed = doctor_fix(&amx, "y\n");

    let (ok, line) = check_line(&printed, "hooks");
    assert!(ok, "there is nothing missing to report: {line}");
    assert!(
        line.contains("pi"),
        "and it says whose pane amx reads: {line}"
    );
    assert!(
        !printed.contains("go ahead?"),
        "nobody was asked to agree to anything: {printed}"
    );
    assert_eq!(
        everything_under(amx.home()),
        before,
        "and no settings file was made anywhere under the person's home"
    );
}
