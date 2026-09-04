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
