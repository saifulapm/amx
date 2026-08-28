//! The wall itself: the agents on it, the groups they stand in, and what
//! looking at a row or acting on one does.
//!
//! Every one of these drives the view in a real tmux pane, because a view is
//! only a view when something is drawing it on a terminal: what it puts on the
//! screen, what a keypress does to that, and what it leaves behind when it
//! closes are all questions a pty answers and nothing else does.

mod common;

use common::{Harness, card_on};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// Epoch seconds, for the records a test writes as though they had just
/// happened.
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("a clock")
        .as_secs()
}

/// An agent whose command has ended: no pane, and the record is the whole
/// story. `ago` is how long since it ended, which is what orders them.
fn finished(amx: &Harness, id: &str, state: &str, ago: u64) {
    let at = now() - ago;
    amx.record(id, "%404");
    amx.set_state(
        id,
        json!({
            "state": state,
            "exit": 0,
            "since": at,
            "last_event": at,
            "result": "did what it was asked",
        }),
    );
}

/// What is on the view's screen now.
fn screen(amx: &Harness, pane: &str) -> String {
    amx.capture(pane)
}

/// The mark on an agent's row, as the view has it drawn now: past the gutter
/// its rows are indented by, which is where the unread mark goes.
fn mark(amx: &Harness, view: &str, id: &str) -> Option<char> {
    row_of(amx, view, id)?.chars().nth(2)
}

/// Whether the view is saying nobody has read this row, which is the first
/// column of the gutter.
fn unread(amx: &Harness, view: &str, id: &str) -> bool {
    row_of(amx, view, id).is_some_and(|row| row.starts_with('•'))
}

/// The line of the list an agent is drawn on.
fn row_of(amx: &Harness, view: &str, id: &str) -> Option<String> {
    screen(amx, view)
        .lines()
        .find(|line| line.contains(id))
        .map(str::to_string)
}

/// The same screen with the colours the view drew it in, as the escapes tmux
/// wrote them: what a bar is made of cannot be read off the text.
fn coloured(amx: &Harness, pane: &str) -> String {
    amx.tmux(&["capture-pane", "-p", "-e", "-J", "-t", pane])
}

/// The line of the list holding `text`, escapes and all. The last of them,
/// because the header at the top says what there is in the same words the
/// headings under it do, and the list is the part the cursor walks.
fn coloured_line(amx: &Harness, view: &str, text: &str) -> String {
    let drawn = coloured(amx, view);
    drawn
        .lines()
        .rfind(|line| line.contains(text))
        .unwrap_or_else(|| panic!("no line holding {text} in:\n{drawn}"))
        .to_string()
}

/// The SGR attributes in force where `word` starts on this captured line:
/// every escape before it walked, resets honoured, and the colour
/// introducers' arguments consumed — the `2` of `38;2;r;g;b` is a
/// colourspace, never the dim attribute.
fn sgr_at(line: &str, word: &str) -> Vec<u16> {
    let at = line
        .find(word)
        .unwrap_or_else(|| panic!("{word:?} is not on {line:?}"));
    let mut on: Vec<u16> = Vec::new();
    let mut rest = &line[..at];
    while let Some(start) = rest.find("\u{1b}[") {
        let after = &rest[start + 2..];
        let Some(end) = after.find('m') else { break };
        let params: Vec<u16> = after[..end]
            .split(';')
            .map(|param| param.parse().unwrap_or(0))
            .collect();
        let mut n = 0;
        while n < params.len() {
            match params[n] {
                0 => on.clear(),
                22 => on.retain(|param| *param != 1 && *param != 2),
                38 | 48 => {
                    n += match params.get(n + 1) {
                        Some(2) => 4,
                        Some(5) => 2,
                        _ => 0,
                    };
                }
                param => on.push(param),
            }
            n += 1;
        }
        rest = &after[end + 1..];
    }
    on
}

/// What the default theme paints a role in, out of the file that states it.
///
/// The escapes below are what tmux wrote for a colour, and a colour typed out
/// here as well would part company with the palette the day somebody edited
/// one. `assets/themes/default.toml` is held to the struct default by a test
/// of its own, so reading it here reaches both.
fn default_theme(role: &str) -> (u8, u8, u8) {
    let said = include_str!("../assets/themes/default.toml")
        .lines()
        .find_map(|line| line.strip_prefix(&format!("{role} = ")))
        .unwrap_or_else(|| panic!("the default theme names {role}"))
        .trim()
        .trim_matches('"');
    rgb(said)
}

/// A colour as a theme file spells it, in the three bytes tmux writes.
fn rgb(said: &str) -> (u8, u8, u8) {
    let hex = said
        .strip_prefix('#')
        .unwrap_or_else(|| panic!("a hex colour: {said}"));
    let byte = |at: usize| {
        u8::from_str_radix(&hex[at..at + 2], 16).unwrap_or_else(|_| panic!("a hex colour: {said}"))
    };
    (byte(0), byte(2), byte(4))
}

/// A colour as the escape tmux writes for text painted in it.
fn text_in((r, g, b): (u8, u8, u8)) -> String {
    format!("38;2;{r};{g};{b}")
}

/// A role of the default theme as the escape tmux writes for text in it.
fn foreground(role: &str) -> String {
    text_in(default_theme(role))
}

/// And as the escape for a line drawn on it.
fn background(role: &str) -> String {
    let (r, g, b) = default_theme(role);
    format!("48;2;{r};{g};{b}")
}

/// Write a theme where the config file's `theme` name reaches it, which is the
/// directory beside the config a person keeps their own in.
fn theme(amx: &Harness, name: &str, text: &str) {
    let dir = amx.home().join(".config/amx/themes");
    std::fs::create_dir_all(&dir).expect("the themes directory");
    std::fs::write(dir.join(format!("{name}.toml")), text).expect("writing the theme");
}

/// Move a record's directory, which is what decides its project.
fn running_in(amx: &Harness, id: &str, dir: &std::path::Path) {
    let path = amx.agent_dir(id).join("meta.json");
    let mut meta: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).expect("the record")).expect("the record");
    meta["dir"] = json!(dir);
    std::fs::write(&path, serde_json::to_vec(&meta).expect("the record")).expect("the record");
}

/// Every agent amx holds a record for.
fn agents(amx: &Harness) -> Vec<String> {
    let mut ids: Vec<String> = std::fs::read_dir(amx.state_root())
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    ids.sort();
    ids
}

/// Wait for a view with nothing in it, which is the one line amx has for a
/// wall nobody has put anything on.
fn until_empty(amx: &Harness, view: &str) {
    amx.until("the empty view", || {
        screen(amx, view).contains("nobody asking").then_some(())
    });
}

fn press(amx: &Harness, view: &str, key: &str) {
    amx.tmux(&["send-keys", "-t", view, key]);
}

/// The same key twice, close enough together that a view holding a window
/// open for the second press still has it open.
fn twice(amx: &Harness, view: &str, key: &str) {
    amx.tmux(&["send-keys", "-t", view, key, key]);
}

/// A mouse event injected at the view as the raw SGR bytes a terminal sends
/// once a program has asked for the mouse: button 0 is the left button, 64
/// and 65 the wheel, 35 motion with nothing held. Column and row are counted
/// from one, which is the terminal's own way, and `press` is the trailing
/// letter — `M` down, `m` up.
fn mouse(amx: &Harness, view: &str, code: u16, column: u16, row: u16, press: bool) {
    let end = if press { 'M' } else { 'm' };
    amx.tmux(&[
        "send-keys",
        "-t",
        view,
        "-l",
        &format!("\u{1b}[<{code};{column};{row}{end}"),
    ]);
}

/// A left click where a person clicks: press and release on one spot.
fn click(amx: &Harness, view: &str, column: u16, row: u16) {
    mouse(amx, view, 0, column, row, true);
    mouse(amx, view, 0, column, row, false);
}

/// The 1-based screen row an agent's row is drawn on, for a pointer to land
/// on.
fn screen_row_of(amx: &Harness, view: &str, id: &str) -> u16 {
    let drawn = screen(amx, view);
    let at = drawn
        .lines()
        .position(|line| line.contains(id))
        .unwrap_or_else(|| panic!("no row for {id} in:\n{drawn}"));
    at as u16 + 1
}

/// A pane showing exactly these rows and nothing else, where a real agent's
/// pane would be: the fixture screens the chrome cut is measured against, put
/// somewhere the view has to read them the way it reads any other pane.
fn a_pane_showing(amx: &Harness, rows: &[&str]) -> String {
    let drawn: String = rows.iter().map(|row| format!("{row}\\n")).collect();
    amx.tmux(&[
        "new-session",
        "-d",
        "-x",
        "60",
        "-y",
        "24",
        "-P",
        "-F",
        "#{pane_id}",
        "--",
        "sh",
        "-c",
        &format!("printf '{drawn}'; while :; do sleep 0.05; done"),
    ])
}

/// The five rows claude draws at the bottom of every pane it has the room
/// for, in the vendor's own order: the composer's top border with its
/// right-anchored label, whatever is staged in the box, the composer's bottom
/// border, the statusline, and the mode footer. Transcribed from a live
/// 2.1.237 on 2026-08-21.
const CHROME: [&str; 5] = [
    "──────────────────────────── execute amx-v2 ─",
    "❯ ",
    "─────────────────────────────────────────────",
    "  Opus 5 │ amx-main (main) │ xhigh",
    "  ⏵⏵ accept edits on (shift+tab to cycle)",
];

fn pane_field(amx: &Harness, pane: &str, format: &str) -> String {
    amx.tmux(&["display-message", "-p", "-t", pane, format])
}

/// A terminal amx cannot tell is inside tmux, which is what tmux's own two
/// variables say and the only thing that says it.
fn outside_tmux(amx: &Harness) -> String {
    amx.in_a_terminal(&[("TMUX", ""), ("TMUX_PANE", "")], &[])
}

/// The sessions on this harness's server, by name.
fn sessions(amx: &Harness) -> Vec<String> {
    amx.tmux(&["list-sessions", "-F", "#{session_name}"])
        .lines()
        .map(str::to_string)
        .collect()
}

/// An agent as every agent is: a detached session of its own named for the id,
/// with a line on its screen to know it by.
fn an_agent_session(amx: &Harness, id: &str) -> String {
    let pane = amx.tmux(&[
        "new-session",
        "-d",
        "-s",
        &format!("amx-{id}"),
        "-P",
        "-F",
        "#{pane_id}",
        "--",
        "sh",
        "-c",
        "printf 'the agent at work\\n'; while :; do sleep 0.05; done",
    ]);
    amx.record(id, &pane);
    pane
}

/// A person looking at a session: a tmux client of their own, on a terminal of
/// its own, the way somebody who typed `tmux attach` has one.
///
/// tmux's two variables are cleared for it, because the pane the client is
/// started in is itself inside tmux and a client that knows that declines to
/// nest.
fn watching(amx: &Harness, session: &str) -> String {
    amx.tmux(&[
        "new-session",
        "-d",
        "-P",
        "-F",
        "#{pane_id}",
        "--",
        "env",
        "-u",
        "TMUX",
        "-u",
        "TMUX_PANE",
        "tmux",
        "-L",
        amx.socket(),
        "-f",
        "/dev/null",
        "attach-session",
        "-t",
        session,
    ])
}

/// The terminals of whoever is looking at a session, if anybody is.
fn clients_on(amx: &Harness, session: &str) -> String {
    amx.tmux(&["list-clients", "-t", session, "-F", "#{client_tty}"])
}

#[test]
fn bare_amx_draws_the_list_in_the_terminal_it_was_typed_in() {
    let amx = Harness::new();

    // The same command at two terminals: one inside tmux, one that as far as
    // amx can tell is not.
    let inside = amx.in_a_terminal(&[], &[]);
    let outside = outside_tmux(&amx);

    for view in [&inside, &outside] {
        until_empty(&amx, view);
        assert_eq!(
            pane_field(&amx, view, "#{pane_current_command}"),
            "amx",
            "the view is the terminal's own program, not a client attached to \
             one somewhere else"
        );
    }

    let named = sessions(&amx);
    assert!(
        !named.iter().any(|name| name == "amx"),
        "and nothing was built to draw it in: {named:?}"
    );
}

#[test]
fn the_view_gathers_the_agents_under_what_they_need() {
    let amx = Harness::new();
    amx.play("ask-a1b", "asks-a-question");
    amx.play("port-import-b2c", "works-with-a-spinner");
    amx.play("fix-login-c3d", "happy-turn");
    amx.until_state("ask-a1b", "waiting");
    amx.until_state("port-import-b2c", "working");
    amx.until_state("fix-login-c3d", "idle");
    finished(&amx, "old-job-d4e", "done", 60);

    let view = amx.in_a_terminal(&[], &[]);
    let drawn = amx.until("every group", || {
        let drawn = screen(&amx, &view);
        ["NEEDS INPUT", "WORKING", "IDLE", "COMPLETED"]
            .iter()
            .all(|group| drawn.contains(group))
            .then_some(drawn)
    });

    for id in ["ask-a1b", "port-import-b2c", "fix-login-c3d", "old-job-d4e"] {
        assert!(drawn.contains(id), "{id} is missing from:\n{drawn}");
    }
    // A row says what the agent is up to: what it is asking, else what it is
    // doing, else what it answered.
    assert!(drawn.contains("Claude needs your permission"), "{drawn}");
    assert!(drawn.contains("Running Bash"), "{drawn}");
    assert!(drawn.contains("did what it was asked"), "{drawn}");

    // Twice over, in two vocabularies and two cases: the heading says what the
    // group means, and the band at the top says the word the list can be
    // narrowed by.
    for group in ["NEEDS INPUT", "COMPLETED"] {
        assert_eq!(
            drawn.matches(group).count(),
            1,
            "{group} stands over its rows and nowhere else:\n{drawn}"
        );
    }
    assert!(
        drawn.contains("1 WAITING"),
        "the one group that wants a person is counted in the badge:\n{drawn}"
    );
    assert!(
        drawn.contains("1 done"),
        "and the rest are counted beside it:\n{drawn}"
    );
}

#[test]
fn glyphs_say_the_states_apart_and_the_working_one_breathes() {
    let amx = Harness::new();
    amx.play("ask-a1b", "asks-a-question");
    amx.play("port-import-b2c", "works-with-a-spinner");
    amx.play("fix-login-c3d", "happy-turn");
    amx.until_state("ask-a1b", "waiting");
    amx.until_state("port-import-b2c", "working");
    amx.until_state("fix-login-c3d", "idle");
    finished(&amx, "old-job-d4e", "done", 60);

    let view = amx.in_a_terminal(&[], &[]);
    for (id, want) in [
        ("ask-a1b", '?'),
        ("fix-login-c3d", '○'),
        ("old-job-d4e", '●'),
    ] {
        amx.until(&format!("{id} to be marked {want}"), || {
            (mark(&amx, &view, id) == Some(want)).then_some(())
        });
    }

    // The working row is drawn a frame at a time, so watching it for a moment
    // shows more than one of them — and every one is the vendor's own, from the
    // set tmux hands its panes rather than the one ghostty asks for.
    let mut frames = std::collections::BTreeSet::new();
    amx.until("the working row to breathe", || {
        frames.extend(mark(&amx, &view, "port-import-b2c"));
        (frames.len() > 1).then_some(())
    });
    for frame in &frames {
        assert!(
            "·✢*✶✻✽".contains(*frame),
            "{frame} is not a frame of the pulse: {frames:?}"
        );
    }
}

#[test]
fn a_working_row_says_what_the_line_over_the_composer_says() {
    let amx = Harness::new();
    let mut pane_rows = vec![
        "● Read(src/importer.rs)",
        "  ⎿  Read 210 lines",
        "",
        "✽ Nesting… (15s · ↓ 1.3k tokens)",
    ];
    pane_rows.extend_from_slice(&CHROME);
    let pane = a_pane_showing(&amx, &pane_rows);
    amx.record("port-cli-b2c", &pane);
    // The hooks have gone quiet inside the turn, which is the state a reader
    // is at the pane for. What the record has to say about the turn is the
    // tool call it last saw, and that call may have ended ten minutes ago.
    let quiet_since = now() - 600;
    amx.set_state(
        "port-cli-b2c",
        json!({
            "state": "working",
            "summary": "Running Read",
            "since": quiet_since,
            "last_event": quiet_since,
        }),
    );

    let view = amx.in_a_terminal(&[], &[]);
    let row = amx.until("the row to say what the pane says", || {
        row_of(&amx, &view, "port-cli-b2c").filter(|row| row.contains("Nesting"))
    });
    assert!(
        row.contains("Nesting… (15s · ↓ 1.3k tokens)"),
        "the vendor's line whole, less the glyph it pulses in front of it:\n{row}"
    );
    assert!(
        !row.contains("Running Read"),
        "the record's account of the same turn is the older one:\n{row}"
    );

    // Read and not written down: the line is about the second it was read in,
    // and a record carrying it would have every later reader repeat it as news.
    assert_eq!(
        amx.state("port-cli-b2c")["summary"],
        "Running Read",
        "the record says what the hooks said"
    );
}

/// A turn that ended with five paragraphs, which is the shape a row cannot say
/// anything useful about on its own: the answer opens with the work rather
/// than with a line about the work.
fn ended_with_an_answer(amx: &Harness, id: &str) {
    let at = now() - 60;
    amx.record(id, "%404");
    amx.set_state(
        id,
        json!({
            "state": "done",
            "exit": 0,
            "since": at,
            "last_event": at,
            "result": "ported the importer\n\nthe fixtures moved with it, and the suite is green.",
        }),
    );
}

#[test]
fn summary_command_writes_the_line_a_finished_row_shows() {
    let amx = Harness::new();
    // Whatever somebody configures here is routinely a model call. This one
    // answers the same way every time, which is the only difference that
    // matters to the reader that runs it.
    amx.config("summary_command = \"tr a-z A-Z\"\n");
    ended_with_an_answer(&amx, "port-cli-b2c");

    let view = amx.in_a_terminal(&[], &[]);
    let row = amx.until("the row to say what the command made of the answer", || {
        row_of(&amx, &view, "port-cli-b2c").filter(|row| row.contains("PORTED THE IMPORTER"))
    });
    assert!(
        !row.contains("ported the importer"),
        "the line stands where the answer's first line stood:\n{row}"
    );

    // On the record, so every reader after this one has the line without
    // running the command again, and the ask is marked as having come back.
    assert_eq!(
        amx.state("port-cli-b2c")["summary"],
        "PORTED THE IMPORTER",
        "the whole answer went in, and the first line it printed came out"
    );
    let asked: Value = serde_json::from_slice(
        &std::fs::read(amx.agent_dir("port-cli-b2c").join("summary.asked")).expect("the ask"),
    )
    .expect("the ask");
    assert_eq!(asked["over"], true, "one ask per turn, whatever came back");
}

#[test]
fn a_finished_row_without_a_summary_command_costs_nothing_and_keeps_the_answer() {
    let amx = Harness::new();
    // No config at all, which is the state every amx nobody has configured is
    // in. Nothing is run and nothing is spent.
    ended_with_an_answer(&amx, "port-cli-b2c");

    let view = amx.in_a_terminal(&[], &[]);
    let row = amx.until("the row", || {
        row_of(&amx, &view, "port-cli-b2c").filter(|row| row.contains("ported the importer"))
    });
    assert!(
        !row.contains("the fixtures moved with it"),
        "the answer's first line, which is what a row has room for:\n{row}"
    );

    // Nothing was asked, so nothing was written down about an ask, and the
    // record is where the turn left it.
    assert!(
        !amx.agent_dir("port-cli-b2c").join("summary.asked").exists(),
        "no command is no question"
    );
    assert_eq!(amx.state("port-cli-b2c")["summary"], Value::Null);
}

#[test]
fn ctrl_s_turns_the_axis_onto_the_project_each_agent_runs_in() {
    let amx = Harness::new();
    let repo = amx.a_repo();
    amx.play("ask-a1b", "asks-a-question");
    amx.play("fix-login-b2c", "happy-turn");
    amx.until_state("ask-a1b", "waiting");
    amx.until_state("fix-login-b2c", "idle");
    // One in the repository itself and one in a subdirectory of it, which is
    // the case the walk up the ancestors exists for. The third stays where the
    // harness put it, outside any repository at all.
    running_in(&amx, "ask-a1b", &repo);
    running_in(&amx, "fix-login-b2c", &repo.join("src"));
    finished(&amx, "old-job-c3d", "done", 60);

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("the agents", || {
        screen(&amx, &view).contains("NEEDS INPUT").then_some(())
    });

    press(&amx, &view, "C-s");
    let drawn = amx.until("the project headings", || {
        let drawn = screen(&amx, &view);
        drawn.contains("~/repo").then_some(drawn)
    });

    let row = |id: &str| {
        drawn
            .lines()
            .find(|line| line.contains(id))
            .unwrap_or_else(|| panic!("no row for {id} in:\n{drawn}"))
            .to_string()
    };
    let at = |text: &str| {
        drawn
            .lines()
            .position(|line| line.starts_with(&format!(" {text} ")))
            .unwrap_or_else(|| panic!("no {text} heading in:\n{drawn}"))
    };
    assert!(
        at("~/repo") < at("~"),
        "the project with the question in it comes first:\n{drawn}"
    );
    assert!(
        drawn
            .lines()
            .filter(|line| line.starts_with(" ~/repo "))
            .count()
            == 1,
        "one heading for the repository, subdirectory and all:\n{drawn}"
    );
    // The heading no longer says the state, so every row carries it.
    assert!(row("ask-a1b").contains("waiting"), "{drawn}");
    assert!(row("fix-login-b2c").contains("idle"), "{drawn}");
    assert!(row("old-job-c3d").contains("done"), "{drawn}");
    assert!(
        !drawn.contains("NEEDS INPUT"),
        "and the state headings are gone with the axis:\n{drawn}"
    );

    // And back, on the same key.
    press(&amx, &view, "C-s");
    amx.until("what they need again", || {
        screen(&amx, &view).contains("NEEDS INPUT").then_some(())
    });
}

#[test]
fn a_wall_with_nothing_on_it_says_so_in_one_line_of_amxs_own() {
    let amx = Harness::new();
    let view = amx.in_a_terminal(&[], &[]);
    until_empty(&amx, &view);

    let drawn = screen(&amx, &view);
    // No heading, because a heading stands over rows and there are none, and
    // amx's own line where the rows would be. How many rows the empty wall
    // comes to is the empty wall's own business.
    for group in ["NEEDS INPUT", "WORKING", "IDLE", "COMPLETED"] {
        assert!(
            !drawn.contains(group),
            "{group} is a heading over rows, and there are none:\n{drawn}"
        );
    }
    assert!(
        drawn.contains("nothing running, nothing broken, nobody asking"),
        "and the wall says what it is in its own words:\n{drawn}"
    );

    // And it goes the moment there is anything to read off a row.
    amx.play("ask-a1b", "asks-a-question");
    amx.until("the agent's own row", || {
        let drawn = screen(&amx, &view);
        (drawn.contains("ask-a1b") && !drawn.contains("nobody asking")).then_some(())
    });
}

#[test]
fn a_blank_line_stands_the_list_off_from_the_header() {
    let amx = Harness::new();
    amx.play("ask-a1b", "asks-a-question");
    amx.until_state("ask-a1b", "waiting");

    let view = amx.in_a_terminal(&[], &[]);
    let drawn = amx.until("the first heading", || {
        let drawn = screen(&amx, &view);
        drawn.contains("NEEDS INPUT").then_some(drawn)
    });

    let lines: Vec<&str> = drawn.lines().map(str::trim_end).collect();
    let at = lines
        .iter()
        .position(|line| line.starts_with(" NEEDS INPUT "))
        .unwrap_or_else(|| panic!("no NEEDS INPUT heading in:\n{drawn}"));
    assert!(
        lines[at - 1].is_empty(),
        "the first heading is stood off from the header the way the next one \
         is stood off from it:\n{drawn}"
    );
    assert!(
        lines[at - 2].starts_with("└ next") && lines[at - 3].contains("running"),
        "and what the space is under is the header, both rows of it:\n{drawn}"
    );
}

#[test]
fn completed_agents_fold_into_a_count_when_the_screen_runs_out_of_rows() {
    let amx = Harness::new();
    for (n, id) in ["one-a1b", "two-b2c", "three-c3d", "four-d4e", "five-e5f"]
        .iter()
        .enumerate()
    {
        finished(&amx, id, "done", n as u64 * 60);
    }

    // A full-size terminal has a row for every one of them, so nothing
    // folds however many have finished.
    let view = amx.in_a_terminal(&[], &[]);
    let whole = amx.until("every row", || {
        let drawn = screen(&amx, &view);
        (drawn.contains("one-a1b") && drawn.contains("five-e5f")).then_some(drawn)
    });
    assert!(!whole.contains("more"), "nothing is held back:\n{whole}");

    // Shrunk to seven rows the list has five, and the fold takes exactly
    // what stopped fitting: the two oldest, behind the count on the last
    // row.
    amx.tmux(&["resize-window", "-t", &view, "-x", "80", "-y", "7"]);
    let folded = amx.until("the fold", || {
        let drawn = screen(&amx, &view);
        (drawn.contains("one-a1b") && drawn.contains("2 more")).then_some(drawn)
    });
    assert!(
        !folded.contains("five-e5f"),
        "the oldest are behind the count:\n{folded}"
    );

    // Down onto the fold — three agents are shown, so it is the fourth row
    // — and open it, then one more row down to walk history onto the
    // screen.
    amx.tmux(&[
        "send-keys",
        "-t",
        &view,
        "Down",
        "Down",
        "Down",
        "Enter",
        "Down",
    ]);
    amx.until("the rest of them", || {
        screen(&amx, &view).contains("five-e5f").then_some(())
    });
}

#[test]
fn a_row_keeps_the_weight_for_what_is_asking_and_dims_what_it_said() {
    let amx = Harness::new();
    amx.play("ask-a1b", "asks-a-question");
    amx.until_state("ask-a1b", "waiting");
    finished(&amx, "fix-login-b2c", "done", 60);

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("both rows", || {
        let drawn = screen(&amx, &view);
        (drawn.contains("ask-a1b") && drawn.contains("fix-login-b2c")).then_some(())
    });

    // A row that wants nobody: its name at the terminal's own strength, what
    // it said dim under the name of the next one, and no weight anywhere.
    let quiet = coloured_line(&amx, &view, "fix-login-b2c");
    let name = sgr_at(&quiet, "fix-login-b2c");
    assert!(
        !name.contains(&1) && !name.contains(&2),
        "the name is the terminal's own, neither dim nor bold:\n{quiet:?}"
    );
    assert!(
        sgr_at(&quiet, "did what it was asked").contains(&2),
        "and what it said is the quieter of the two:\n{quiet:?}"
    );
    assert!(
        quiet.contains(&foreground("done")),
        "the glyph alone carries the state's colour:\n{quiet:?}"
    );

    // A row that is asking is the one that stands out, wherever the cursor
    // happens to be: the name bold and in the colour of a thing waiting on a
    // person, and the question at full strength because it is the sentence
    // somebody came to read.
    let asking = coloured_line(&amx, &view, "ask-a1b");
    assert!(
        sgr_at(&asking, "ask-a1b").contains(&1),
        "the waiting name is the bold one:\n{asking:?}"
    );
    assert!(
        asking.contains(&foreground("waiting")),
        "and it is painted for what it wants:\n{asking:?}"
    );
    assert!(
        !sgr_at(&asking, "Claude needs your permission").contains(&2),
        "the question is not dimmed the way a finished row's line is:\n{asking:?}"
    );
}

#[test]
fn a_row_lands_its_name_summary_and_age_in_the_columns_the_grid_fixes() {
    let amx = Harness::new();
    finished(&amx, "fix-login-a1b", "done", 60);

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("the row", || {
        screen(&amx, &view).contains("fix-login-a1b").then_some(())
    });
    let row = row_of(&amx, &view, "fix-login-a1b").expect("a row");
    let cells: Vec<char> = row.chars().collect();
    assert_eq!(cells.len(), 80, "a row is drawn to the edge:\n{row:?}");

    // Two cells of gutter for the marks, the state glyph and the space after
    // it, and then the name column: sixteen cells of it below a hundred.
    let column = |from: usize, to: usize| cells[from..to].iter().collect::<String>();
    assert_eq!(column(4, 20), "fix-login-a1b   ", "{row:?}");
    assert_eq!(column(20, 22), "  ", "two cells stand the columns apart");

    // Then the summary, which takes whatever is left of the screen.
    assert_eq!(
        column(22, 74),
        format!("{:<52}", "did what it was asked"),
        "{row:?}"
    );

    // And the age, right-aligned in the last four cells.
    assert_eq!(column(74, 76), "  ", "{row:?}");
    let age = column(76, 80);
    assert!(
        !age.trim().is_empty() && !age.ends_with(' '),
        "the age is right-aligned in its own column:\n{row:?}"
    );
}

#[test]
fn a_group_heading_is_uppercase_over_a_rule_that_ends_in_its_count() {
    let amx = Harness::new();
    amx.play("ask-a1b", "asks-a-question");
    amx.until_state("ask-a1b", "waiting");
    finished(&amx, "one-b2c", "done", 60);
    finished(&amx, "two-c3d", "done", 120);

    let view = amx.in_a_terminal(&[], &[]);
    let drawn = amx.until("the headings", || {
        let drawn = screen(&amx, &view);
        (drawn.contains("NEEDS INPUT") && drawn.contains("COMPLETED")).then_some(drawn)
    });

    for (label, members) in [("NEEDS INPUT", 1), ("COMPLETED", 2)] {
        let line = drawn
            .lines()
            .find(|line| line.starts_with(&format!(" {label} ")))
            .unwrap_or_else(|| panic!("no {label} heading in:\n{drawn}"))
            .to_string();
        let cells: Vec<char> = line.chars().collect();
        assert_eq!(cells.len(), 80, "a heading is drawn to the edge:\n{line:?}");
        assert_eq!(
            cells[76..].iter().collect::<String>(),
            format!("{members:>4}"),
            "the count is right-aligned in the column the ages are:\n{line:?}"
        );
        let rule: String = cells[label.chars().count() + 2..74].iter().collect();
        assert!(
            !rule.is_empty() && rule.chars().all(|cell| cell == '─'),
            "and a rule runs from the label out to it:\n{line:?}"
        );
    }

    // The label carries the weight and the rule carries none of it, which is
    // what makes a heading without a second type size.
    let painted = coloured_line(&amx, &view, "COMPLETED");
    assert!(
        sgr_at(&painted, "COMPLETED").contains(&1),
        "the label is bold:\n{painted:?}"
    );
    assert!(
        sgr_at(&painted, "───").contains(&2),
        "the rule is dim:\n{painted:?}"
    );
    let waiting = coloured_line(&amx, &view, "NEEDS INPUT");
    assert!(
        waiting.contains(&foreground("waiting")),
        "and the group that wants a person is painted for it:\n{waiting:?}"
    );
}

#[test]
fn a_path_heading_keeps_its_case_and_carries_the_same_rule_and_count() {
    let amx = Harness::new();
    let repo = amx.a_repo();
    amx.play("ask-a1b", "asks-a-question");
    amx.until_state("ask-a1b", "waiting");
    running_in(&amx, "ask-a1b", &repo);
    finished(&amx, "old-job-c3d", "done", 60);

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("the agents", || {
        screen(&amx, &view).contains("NEEDS INPUT").then_some(())
    });
    press(&amx, &view, "C-s");
    let drawn = amx.until("the project headings", || {
        let drawn = screen(&amx, &view);
        drawn.contains("~/repo").then_some(drawn)
    });

    let line = drawn
        .lines()
        .find(|line| line.starts_with(" ~/repo "))
        .unwrap_or_else(|| panic!("no heading over the repository in:\n{drawn}"))
        .to_string();
    let cells: Vec<char> = line.chars().collect();
    assert_eq!(cells.len(), 80, "a heading is drawn to the edge:\n{line:?}");
    assert_eq!(
        cells[76..].iter().collect::<String>(),
        "   1",
        "the count is right-aligned in the column the ages are:\n{line:?}"
    );
    let rule: String = cells[8..74].iter().collect();
    assert!(
        rule.chars().all(|cell| cell == '─'),
        "and a rule runs from the path out to it:\n{line:?}"
    );
    assert!(
        !drawn.contains("~/REPO"),
        "a path is not a word, and a word is what uppercases:\n{drawn}"
    );

    // The weight goes on the segment that says which directory this is, with
    // the parents it hangs off dim behind it. Found by the segment alone,
    // because the escape that changes the weight stands between the two.
    let painted = coloured_line(&amx, &view, "repo");
    let last = sgr_at(&painted, "repo");
    assert!(
        last.contains(&1) && !last.contains(&2),
        "the last segment is the bold one:\n{painted:?}"
    );
    assert!(
        sgr_at(&painted, "~/").contains(&2),
        "and the parents in front of it are dim:\n{painted:?}"
    );
    assert!(
        sgr_at(&painted, "───").contains(&2),
        "the rule is dim, the way it is over a group:\n{painted:?}"
    );
}

#[test]
fn a_path_too_long_for_its_heading_loses_its_middle_and_not_its_end() {
    let amx = Harness::new();
    let deep =
        PathBuf::from("/srv/monorepo/services/ingest/packages/importer-worker/vendor/legacy-shim");
    amx.play("ask-a1b", "asks-a-question");
    amx.until_state("ask-a1b", "waiting");
    running_in(&amx, "ask-a1b", &deep);

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("the agent", || {
        screen(&amx, &view).contains("NEEDS INPUT").then_some(())
    });
    press(&amx, &view, "C-s");
    let line = amx.until("the heading over the deep path", || {
        screen(&amx, &view)
            .lines()
            .find(|line| line.starts_with(" /srv/"))
            .map(str::to_string)
    });

    let cells: Vec<char> = line.chars().collect();
    assert_eq!(
        cells.len(),
        80,
        "a path this long does not push the heading off the edge:\n{line:?}"
    );
    assert_eq!(
        cells[76..].iter().collect::<String>(),
        "   1",
        "the count is in the column it is in over a short path:\n{line:?}"
    );
    assert!(
        cells.iter().filter(|cell| **cell == '─').count() >= 8,
        "and enough rule is left to read the line as a heading:\n{line:?}"
    );

    let path = line.split_whitespace().next().expect("the path");
    assert!(
        path.starts_with("/srv/…/") && path.ends_with("/vendor/legacy-shim"),
        "what goes is the middle: the end is the segment that says \
         which worktree of a project this is:\n{line:?}"
    );
}

#[test]
fn a_row_under_a_path_grows_a_state_word_and_moves_no_other_column() {
    let amx = Harness::new();
    amx.play("port-import-b2c", "works-with-a-spinner");
    amx.until_state("port-import-b2c", "working");
    finished(&amx, "fix-login-a1b", "done", 60);

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("both rows", || {
        let drawn = screen(&amx, &view);
        (drawn.contains("fix-login-a1b") && drawn.contains("port-import-b2c")).then_some(())
    });
    let before = row_of(&amx, &view, "fix-login-a1b").expect("a row under its state");

    press(&amx, &view, "C-s");
    let row = amx.until("the state word on the row", || {
        row_of(&amx, &view, "fix-login-a1b").filter(|row| row.contains("done"))
    });
    let cells: Vec<char> = row.chars().collect();
    let column = |from: usize, to: usize| cells[from..to].iter().collect::<String>();
    assert_eq!(cells.len(), 80, "a row is drawn to the edge:\n{row:?}");
    assert_eq!(column(4, 20), "fix-login-a1b   ", "the name has not moved");
    assert_eq!(column(20, 22), "  ", "two cells stand the columns apart");

    // The heading over the row is a place now, so the row says the state:
    // eight cells of it, which is what the longest of the words needs.
    assert_eq!(column(22, 30), "done    ", "{row:?}");
    assert_eq!(column(30, 32), "  ", "{row:?}");

    // The summary pays for all ten of those cells and nothing else does.
    assert_eq!(
        column(32, 74),
        format!("{:<42}", "did what it was asked"),
        "{row:?}"
    );
    assert_eq!(column(74, 76), "  ", "{row:?}");
    let was: Vec<char> = before.chars().collect();
    assert_eq!(
        column(76, 80),
        was[76..80].iter().collect::<String>(),
        "the age is in the cells it was in under a state heading:\n{row:?}\n{before:?}"
    );

    // The word is quiet while there is nothing to say about how the work went,
    // and takes the phase's own colour once there is.
    let working = coloured_line(&amx, &view, "port-import-b2c");
    assert!(
        sgr_at(&working, "working").contains(&2),
        "a row still at it says so under its breath:\n{working:?}"
    );
    let done = coloured_line(&amx, &view, "fix-login-a1b");
    assert!(
        !sgr_at(&done, "done").contains(&2),
        "and a row that has ended says how it went:\n{done:?}"
    );
    assert!(
        done.contains(&foreground("done")),
        "in the colour that says so:\n{done:?}"
    );
}

/// The background the cursor's bar is made of, as tmux writes the escape.
fn bar() -> String {
    background("cursor")
}

#[test]
fn the_view_paints_in_the_theme_the_config_names() {
    let amx = Harness::new();
    amx.config("theme = \"terminal\"\n");
    finished(&amx, "fix-login-a1b", "done", 60);

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("the row", || {
        screen(&amx, &view).contains("fix-login-a1b").then_some(())
    });

    // The terminal theme names its colours and measures none, so the row is
    // painted out of this terminal's own palette — by index, which is how
    // tmux writes a colour that was named — rather than in a value amx
    // measured.
    let row = coloured_line(&amx, &view, "fix-login-a1b");
    assert!(
        row.contains("38;5;"),
        "nothing on the row is painted in a colour the palette names:\n{row:?}"
    );

    // Which means the two the default theme would have put on this row — the
    // green that says it went the way it was meant to, and the bar under the
    // cursor — are not on it.
    assert!(
        !row.contains(&foreground("done")) && !row.contains(&bar()),
        "the default theme is what is being painted:\n{row:?}"
    );
}

#[test]
fn editing_the_theme_recolours_the_view_that_is_open_on_it() {
    let amx = Harness::new();
    amx.config("theme = \"mine\"\n");
    theme(&amx, "mine", "done = \"#ff00ff\"\n");
    finished(&amx, "fix-login-a1b", "done", 60);

    let view = amx.in_a_terminal(&[], &[]);
    let before = text_in(rgb("#ff00ff"));
    amx.until("the row in the colour the theme says", || {
        coloured(&amx, &view).contains(&before).then_some(())
    });

    // What a person picking a palette does: edit the file and look at the
    // screen. Nothing is restarted, nothing is pressed, and the view is the
    // thing that has to notice.
    theme(&amx, "mine", "done = \"#00ffff\"\n");

    let after = text_in(rgb("#00ffff"));
    let drawn = amx.until("the row in the colour the file now says", || {
        let drawn = coloured(&amx, &view);
        drawn.contains(&after).then_some(drawn)
    });
    assert!(
        !drawn.contains(&before),
        "the colour that was edited away is still on the screen:\n{drawn:?}"
    );
}

#[test]
fn the_list_takes_the_mouse_and_a_click_is_the_cursor() {
    let amx = Harness::new();
    finished(&amx, "fix-login-a1b", "done", 60);
    finished(&amx, "port-import-b2c", "done", 120);

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("both rows", || {
        let drawn = screen(&amx, &view);
        (drawn.contains("fix-login-a1b") && drawn.contains("port-import-b2c")).then_some(())
    });
    assert_eq!(
        pane_field(&amx, &view, "#{mouse_any_flag}"),
        "1",
        "the view asked the terminal for the mouse"
    );

    // A click on the older agent's row moves the bar to it. Its id is about
    // to be on the footer too, so the row is the line that also carries its
    // summary.
    click(
        &amx,
        &view,
        5,
        screen_row_of(&amx, &view, "port-import-b2c"),
    );
    amx.until("the bar under the clicked row", || {
        coloured(&amx, &view)
            .lines()
            .find(|line| line.contains("port-import-b2c") && line.contains("did what"))
            .filter(|line| line.contains(&bar()))
            .map(|_| ())
    });
    assert!(
        !coloured_line(&amx, &view, "fix-login-a1b").contains(&bar()),
        "one cursor, and the click is where it is"
    );

    // And the click went on to bring the window forward, the way enter
    // does: this agent has no session to carry back, and the refusal
    // naming that is how far it got.
    amx.until("the refusal", || {
        screen(&amx, &view)
            .contains("no session was ever recorded")
            .then_some(())
    });

    // A click on the heading shuts the group, and another opens it.
    let heading = screen(&amx, &view)
        .lines()
        .position(|line| line.starts_with(" COMPLETED "))
        .expect("the heading") as u16
        + 1;
    click(&amx, &view, 5, heading);
    amx.until("the group shut", || {
        let drawn = screen(&amx, &view);
        (drawn.contains("COMPLETED") && !drawn.contains("port-import-b2c")).then_some(())
    });
    click(&amx, &view, 5, heading);
    amx.until("the group open again", || {
        screen(&amx, &view)
            .contains("port-import-b2c")
            .then_some(())
    });

    // The mouse goes back with the screen when the view closes.
    amx.tmux(&["set-option", "-w", "-t", &view, "remain-on-exit", "on"]);
    press(&amx, &view, "q");
    amx.until("the view to close", || {
        let dead = amx.tmux(&["display-message", "-p", "-t", &view, "#{pane_dead}"]);
        (dead == "1").then_some(())
    });
    assert_eq!(
        pane_field(&amx, &view, "#{mouse_any_flag}"),
        "0",
        "the capture was released on the way out"
    );
}

#[test]
fn hovering_a_row_tints_its_name_and_moves_no_cursor() {
    let amx = Harness::new();
    finished(&amx, "fix-login-a1b", "done", 60);
    finished(&amx, "port-import-b2c", "done", 120);

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("both rows", || {
        let drawn = screen(&amx, &view);
        (drawn.contains("fix-login-a1b") && drawn.contains("port-import-b2c")).then_some(())
    });

    // The pointer comes to rest on the row the cursor is not on.
    let row = screen_row_of(&amx, &view, "port-import-b2c");
    mouse(&amx, &view, 35, 5, row, true);
    amx.until("the name to take the tint", || {
        sgr_at(
            &coloured_line(&amx, &view, "port-import-b2c"),
            "port-import-b2c",
        )
        .contains(&1)
        .then_some(())
    });
    assert!(
        coloured_line(&amx, &view, "fix-login-a1b").contains(&bar()),
        "the bar stayed where the keyboard's cursor is"
    );
    assert!(
        !coloured_line(&amx, &view, "port-import-b2c").contains(&bar()),
        "a hover is a tint, not a selection"
    );
}

#[test]
fn the_wheel_walks_the_list_and_pages_the_card_under_the_pointer() {
    let amx = Harness::new();
    amx.record("tall-b2c", "%404");
    let at = now() - 100;
    amx.set_state(
        "tall-b2c",
        json!({
            "state": "done",
            "exit": 0,
            "since": at,
            "last_event": at,
            "result": (0..40).map(|n| format!("said {n}\n")).collect::<String>(),
        }),
    );
    finished(&amx, "short-a1b", "done", 200);

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("both rows", || {
        let drawn = screen(&amx, &view);
        (drawn.contains("tall-b2c") && drawn.contains("short-a1b")).then_some(())
    });

    // Wheel-down over the list walks the selection down, and wheel-up back.
    let over_rows = screen_row_of(&amx, &view, "short-a1b");
    mouse(&amx, &view, 65, 5, over_rows, true);
    amx.until("the bar to walk down", || {
        coloured_line(&amx, &view, "short-a1b")
            .contains(&bar())
            .then_some(())
    });
    mouse(&amx, &view, 64, 5, over_rows, true);
    amx.until("and back up", || {
        coloured_line(&amx, &view, "tall-b2c")
            .contains(&bar())
            .then_some(())
    });

    // With the card open, the wheel pages it where the pointer is over it:
    // the recorded answer opens on its first words and wheel-down reads on.
    // The row's own summary is the answer's first line too, so the card's
    // copy is told from it by counting.
    let carded = card_on(&amx, &view, "tall-b2c");
    assert_eq!(
        carded.matches("said 0").count(),
        2,
        "the top of the answer, over the row saying the same:\n{carded}"
    );
    let inside = carded
        .lines()
        .position(|line| line.contains("said 2"))
        .expect("a row of the card's body") as u16
        + 1;
    mouse(&amx, &view, 65, 5, inside, true);
    let paged = amx.until("the paged card", || {
        let drawn = screen(&amx, &view);
        drawn.contains("more").then_some(drawn)
    });
    assert_eq!(
        paged.matches("said 0").count(),
        1,
        "the top is behind, and only the row still says it:\n{paged}"
    );
    mouse(&amx, &view, 64, 5, inside, true);
    amx.until("the edge again", || {
        (screen(&amx, &view).matches("said 0").count() == 2).then_some(())
    });
}

#[test]
fn the_cursor_is_a_bar_over_rows_and_headings_alike() {
    let amx = Harness::new();
    amx.play("ask-a1b", "asks-a-question");
    amx.until_state("ask-a1b", "waiting");

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("the row", || {
        screen(&amx, &view).contains("ask-a1b").then_some(())
    });

    // The bar is a background colour, so it is in the escapes rather than in
    // the text: the theme's cursor colour, which is the vendor's own for a
    // selected line.
    amx.until("the bar under the cursor", || {
        coloured_line(&amx, &view, "ask-a1b")
            .contains(&bar())
            .then_some(())
    });
    assert!(
        !coloured_line(&amx, &view, "NEEDS INPUT").contains(&bar()),
        "and not under the heading the cursor is not on"
    );

    press(&amx, &view, "Up");
    amx.until("the bar to move up onto the heading", || {
        coloured_line(&amx, &view, "NEEDS INPUT")
            .contains(&bar())
            .then_some(())
    });
    assert!(
        !coloured_line(&amx, &view, "ask-a1b").contains(&bar()),
        "one line at a time, whatever kind of line it is"
    );
}

#[test]
fn enter_shuts_the_group_its_headings_stand_over_and_opens_it_again() {
    let amx = Harness::new();
    finished(&amx, "one-a1b", "done", 60);
    finished(&amx, "two-b2c", "failed", 120);

    let view = amx.in_a_terminal(&[], &[]);
    let drawn = amx.until("both agents", || {
        let drawn = screen(&amx, &view);
        (drawn.contains("one-a1b") && drawn.contains("two-b2c")).then_some(drawn)
    });
    assert!(
        drawn.contains("COMPLETED · 1 failed"),
        "a heading says how many failed in front of its rule:\n{drawn}"
    );
    let heading = |drawn: &str| {
        drawn
            .lines()
            .find(|line| line.starts_with(" COMPLETED "))
            .unwrap_or_else(|| panic!("no COMPLETED heading in:\n{drawn}"))
            .to_string()
    };
    assert!(
        heading(&drawn).ends_with("   2"),
        "and how many there are, open or shut:\n{drawn}"
    );

    // Up off the first agent onto the heading over it, and shut the group.
    press(&amx, &view, "Up");
    press(&amx, &view, "Enter");
    let shut = amx.until("the group to be put away", || {
        let drawn = screen(&amx, &view);
        (!drawn.contains("one-a1b")).then_some(drawn)
    });
    assert!(!shut.contains("two-b2c"), "the rows are away:\n{shut}");
    assert!(
        heading(&shut).ends_with("   2") && shut.contains("COMPLETED · 1 failed"),
        "the count stands for them, unmoved by their going:\n{shut}"
    );

    press(&amx, &view, "Enter");
    amx.until("the agents back", || {
        screen(&amx, &view).contains("one-a1b").then_some(())
    });
}

#[test]
fn enter_puts_the_agent_in_front_of_the_terminal() {
    let amx = Harness::new();
    let view = amx.in_a_terminal(&[], &[]);
    let holding = pane_field(&amx, &view, "#{session_name}");
    until_empty(&amx, &view);

    // Somebody looking at the view, on a terminal of their own. Without a
    // client there is nothing for enter to move, and a view nobody has
    // attached to is not a view anybody is reading.
    let terminal = watching(&amx, &holding);
    let tty = amx.until("a client on the view", || {
        let clients = clients_on(&amx, &holding);
        (!clients.is_empty()).then_some(clients)
    });

    an_agent_session(&amx, "fix-login-a1b");
    // A window the agent opened for itself and left in front of its own, which
    // is not the one somebody pressing enter is asking after.
    amx.tmux(&[
        "new-window",
        "-t",
        "amx-fix-login-a1b",
        "--",
        "sh",
        "-c",
        "while :; do sleep 0.05; done",
    ]);
    amx.until("the row", || row_of(&amx, &view, "fix-login-a1b").map(drop));

    press(&amx, &view, "Enter");
    amx.until("the agent on their screen", || {
        screen(&amx, &terminal)
            .contains("the agent at work")
            .then_some(())
    });
    assert_eq!(
        clients_on(&amx, "amx-fix-login-a1b"),
        tty,
        "the client that was on the view is the one that moved"
    );

    // And back the way they came, because the view never left the session it
    // was drawing in.
    amx.tmux(&["switch-client", "-c", &tty, "-t", &holding]);
    amx.until("the list again", || {
        screen(&amx, &terminal).contains("? keys").then_some(())
    });
    assert!(
        row_of(&amx, &terminal, "fix-login-a1b").is_some(),
        "with the agent still on it"
    );
}

#[test]
fn enter_lends_the_terminal_to_a_view_that_has_it_to_itself() {
    let amx = Harness::new();
    let view = outside_tmux(&amx);
    until_empty(&amx, &view);

    an_agent_session(&amx, "fix-login-a1b");
    amx.until("the row", || row_of(&amx, &view, "fix-login-a1b").map(drop));

    // Outside tmux there is no client to move, so what the view has to give is
    // the terminal itself.
    press(&amx, &view, "Enter");
    amx.until("the agent on the screen", || {
        screen(&amx, &view)
            .contains("the agent at work")
            .then_some(())
    });

    // Detaching is how somebody comes back, and what they come back to is the
    // list they left: the view waited rather than exiting.
    amx.tmux(&["detach-client", "-s", "amx-fix-login-a1b"]);
    amx.until("the list again", || {
        screen(&amx, &view).contains("? keys").then_some(())
    });
    assert!(
        row_of(&amx, &view, "fix-login-a1b").is_some(),
        "with the agent still on it"
    );
}

#[test]
fn ctrl_x_stops_the_agent_and_then_forgets_it() {
    let amx = Harness::new();
    let pane = amx.play("watch-log-e5f", "works-without-end");
    amx.until_state("watch-log-e5f", "working");

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("the row", || {
        screen(&amx, &view).contains("watch-log-e5f").then_some(())
    });

    press(&amx, &view, "C-x");
    amx.until("the agent to stop", || {
        (amx.state("watch-log-e5f")["state"] == "stopped").then_some(())
    });
    // The record says stopped before the signal is sent, so that the exit it
    // causes reads as a stop rather than a failure. The pane going is the
    // stopping itself, and it happens a moment after.
    amx.until("its pane to go with it", || {
        (!amx.pane_alive(&pane)).then_some(())
    });

    // Again on the same row: an agent that has already ended is forgotten,
    // and it takes the two presses forgetting takes anywhere.
    twice(&amx, &view, "C-x");
    amx.until("the record to go", || agents(&amx).is_empty().then_some(()));
    until_empty(&amx, &view);
}

#[test]
fn ctrl_x_arms_a_finished_row_and_says_so_where_its_summary_was() {
    let amx = Harness::new();
    finished(&amx, "fix-login-a1b", "done", 60);

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("the row and what it did", || {
        row_of(&amx, &view, "fix-login-a1b")
            .filter(|row| row.contains("did what it was asked"))
            .map(|_| ())
    });

    // One press says what a second one would do, on the row itself.
    press(&amx, &view, "C-x");
    let armed = amx.until("the warning where the summary was", || {
        coloured(&amx, &view)
            .lines()
            .find(|line| line.contains("ctrl+x again forgets"))
            .map(str::to_string)
    });
    assert!(
        armed.contains("fix-login-a1b"),
        "on the agent's own row rather than at the foot of the screen:\n{armed}"
    );
    assert!(
        !armed.contains("did what it was asked"),
        "in place of the summary rather than beside it:\n{armed}"
    );
    assert!(
        armed.contains(&foreground("waiting")),
        "in the colour of a thing waiting on a person:\n{armed}"
    );
    assert_eq!(
        agents(&amx),
        ["fix-login-a1b"],
        "and one press forgets nothing"
    );

    // The window closes on its own, and the row goes back to saying what the
    // agent did.
    amx.until("the summary to come back", || {
        row_of(&amx, &view, "fix-login-a1b")
            .filter(|row| row.contains("did what it was asked"))
            .map(|_| ())
    });
    assert_eq!(
        agents(&amx),
        ["fix-login-a1b"],
        "a window that closed forgets nothing either"
    );

    // Two presses inside the window is what forgets it.
    twice(&amx, &view, "C-x");
    amx.until("the record to go", || agents(&amx).is_empty().then_some(()));
    until_empty(&amx, &view);
}

#[test]
fn acts_ctrl_x_on_a_heading_forgets_the_finished_and_keeps_the_work() {
    let amx = Harness::new();
    let repo = amx.a_repo();

    // One agent that ran in a tree of its own and left work in it nothing has
    // committed, and one that had no tree at all.
    let out = amx
        .amx_command(&[
            "new",
            "--name",
            "keeps-work-a1b",
            "--dir",
            &repo.to_string_lossy(),
            "--agent",
            &amx.mock(),
            "fix the login bug",
        ])
        .env("MOCK_CLAUDE_SCENARIO", amx.scenario("finishes"))
        .output()
        .expect("running amx new");
    assert!(
        out.status.success(),
        "amx new: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let tree = PathBuf::from(
        amx.meta("keeps-work-a1b")["worktree"]
            .as_str()
            .expect("a worktree"),
    );
    amx.until_state("keeps-work-a1b", "done");
    std::fs::write(tree.join("login.rs"), "fn login() {}\n").expect("the work in the tree");
    finished(&amx, "port-import-b2c", "done", 120);

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("both rows", || {
        let drawn = screen(&amx, &view);
        (drawn.contains("keeps-work-a1b") && drawn.contains("port-import-b2c")).then_some(())
    });

    // Up from the row the view opens on is the heading the group is under.
    // One press arms every finished row under it, in the rows themselves.
    press(&amx, &view, "Up");
    press(&amx, &view, "C-x");
    let armed = amx.until("the armed rows", || {
        let drawn = screen(&amx, &view);
        drawn.contains("ctrl+x again forgets").then_some(drawn)
    });
    assert!(
        !armed.contains("forget 2 finished"),
        "the rows say it and the footer asks nothing:\n{armed}"
    );
    assert_eq!(agents(&amx).len(), 2, "and arming forgets nothing");

    // Two presses whatever the clock did to the first window: if it is still
    // open the first of these forgets, and if it lapsed the first re-arms and
    // the second forgets.
    twice(&amx, &view, "C-x");
    amx.until("the sweep", || {
        (agents(&amx) == ["keeps-work-a1b"]).then_some(())
    });
    assert!(
        tree.exists(),
        "the tree holding work nobody else has a copy of is still here"
    );
}

#[test]
fn acts_ctrl_x_on_a_heading_stops_the_live_and_arms_rows_in_every_state() {
    let amx = Harness::new();
    // A live agent sitting at its prompt and a finished one, both in the
    // harness's home: one project heading stands over both states at once.
    amx.play("fix-login-a1b", "happy-turn");
    amx.until_state("fix-login-a1b", "idle");
    finished(&amx, "old-job-d4e", "done", 60);

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("both rows", || {
        let drawn = screen(&amx, &view);
        (drawn.contains("fix-login-a1b") && drawn.contains("old-job-d4e")).then_some(())
    });
    press(&amx, &view, "C-s");
    amx.until("the project heading over both", || {
        screen(&amx, &view)
            .lines()
            .any(|line| line.starts_with(" ~ "))
            .then_some(())
    });

    // Up from the row the view opens on is the heading. One press stops the
    // live agent and arms every row under the heading, whatever its state —
    // no group is refused any more.
    press(&amx, &view, "Up");
    press(&amx, &view, "C-x");
    amx.until("the live agent to stop", || {
        (amx.state("fix-login-a1b")["state"] == "stopped").then_some(())
    });
    let armed = amx.until("both rows to be armed", || {
        let drawn = screen(&amx, &view);
        (drawn.matches("ctrl+x again forgets").count() == 2).then_some(drawn)
    });
    assert!(
        !armed.contains("has finished"),
        "the refusal went with the rule:\n{armed}"
    );
    assert_eq!(agents(&amx).len(), 2, "and arming forgets nothing");

    // Two presses whatever the clock did to the first window: if it is still
    // open the first of these forgets both, and if it lapsed the first
    // re-arms — everything under the heading is terminal by now — and the
    // second forgets.
    twice(&amx, &view, "C-x");
    amx.until("the group to be forgotten", || {
        agents(&amx).is_empty().then_some(())
    });
    // The project axis has no welcome line: a list of places with nothing to
    // arrange says so plainly.
    amx.until("the empty wall", || {
        screen(&amx, &view).contains("no agents").then_some(())
    });
}

#[test]
fn acts_space_takes_the_unread_mark_off_the_row_it_opened() {
    let amx = Harness::new();
    finished(&amx, "fix-login-a1b", "done", 60);
    finished(&amx, "port-import-b2c", "done", 120);

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("both rows to be marked unread", || {
        (unread(&amx, &view, "fix-login-a1b") && unread(&amx, &view, "port-import-b2c"))
            .then_some(())
    });

    // The cursor opens on the newest ending, which is the row the card opens
    // over.
    press(&amx, &view, "Space");
    amx.until("the mark to go with the look", || {
        (!unread(&amx, &view, "fix-login-a1b")).then_some(())
    });
    assert!(
        unread(&amx, &view, "port-import-b2c"),
        "and the row nobody opened keeps its mark:\n{}",
        screen(&amx, &view)
    );
    assert!(
        amx.state("fix-login-a1b")["seen"].as_u64().unwrap_or(0) > 0,
        "the look is on the record, so the next view opens knowing it"
    );
}
