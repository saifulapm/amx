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
use std::path::{Path, PathBuf};
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

/// What the last look at the forge wrote down beside the record, which is what
/// a row's number is read from. Written as of now, so the view takes it at its
/// word and no forge is asked at all.
fn a_request(amx: &Harness, id: &str, number: u64, standing: &str) {
    let branch = format!("amx/{id}");
    amx.set_meta(id, json!({ "branch": branch }));
    let written = json!({
        "asked": now(),
        "branch": branch,
        "prs": [{ "number": number, "standing": standing }],
    });
    std::fs::write(
        amx.agent_dir(id).join("pr.json"),
        written.to_string().as_bytes(),
    )
    .expect("what the last look wrote");
}

/// What is on the view's screen now.
fn screen(amx: &Harness, pane: &str) -> String {
    amx.capture(pane)
}

/// Which line of a drawn screen holds a word, for the tests about what stands
/// over what.
fn line_of(drawn: &str, text: &str) -> usize {
    drawn
        .lines()
        .position(|line| line.contains(text))
        .unwrap_or_else(|| panic!("no line holding {text} in:\n{drawn}"))
}

/// The glyph on an agent's row, as the view has it drawn now: past the blank
/// cell its rows are indented by.
fn mark(amx: &Harness, view: &str, id: &str) -> Option<char> {
    row_of(amx, view, id)?.chars().nth(1)
}

/// Somebody having been to read what an agent is holding, written where a look
/// writes it. Nothing on the wall is painted for it: what it moves is where the
/// row sorts against the completed fold.
fn read(amx: &Harness, id: &str) {
    let mut state = amx.state(id);
    state["seen"] = json!(now());
    amx.set_state(id, state);
}

/// The line of the list an agent is drawn on.
fn row_of(amx: &Harness, view: &str, id: &str) -> Option<String> {
    screen(amx, view)
        .lines()
        .find(|line| line.contains(id))
        .map(str::to_string)
}

/// How many rows of a drawn screen name an agent whose id begins this way,
/// which is how many of a group's rows the fold left standing.
fn rows_of(drawn: &str, prefix: &str) -> usize {
    drawn.lines().filter(|line| line.contains(prefix)).count()
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

/// The SGR attributes in force where `word` starts on this capture: every
/// escape before it walked, resets honoured, and the colour introducers'
/// arguments consumed — the `2` of `38;2;r;g;b` is a colourspace, never the
/// dim attribute.
///
/// A whole screen where the attribute being read was turned on further up it:
/// tmux writes an attribute where it changes and leaves it in force, so a
/// line lifted out on its own carries none of what the lines above it set.
fn sgr_at(drawn: &str, word: &str) -> Vec<u16> {
    let at = drawn
        .find(word)
        .unwrap_or_else(|| panic!("{word:?} is not on {drawn:?}"));
    let mut on: Vec<u16> = Vec::new();
    let mut rest = &drawn[..at];
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

/// What the default theme paints a role in, as the file that states it spells
/// it: a hex it measured, or the name of one of the terminal's own.
///
/// The escapes below are what tmux wrote for a colour, and a colour typed out
/// here as well would part company with the palette the day somebody edited
/// one. `assets/themes/default.toml` is held to the struct default by a test
/// of its own, so reading it here reaches both.
fn default_theme(role: &str) -> &'static str {
    include_str!("../assets/themes/default.toml")
        .lines()
        .find_map(|line| line.strip_prefix(&format!("{role} = ")))
        .unwrap_or_else(|| panic!("the default theme names {role}"))
        .trim()
        .trim_matches('"')
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

/// A colour the theme named rather than measured, as the same escape: the
/// terminal's own eight, which is what a name is written as and what tmux
/// keeps.
fn text_named(said: &str) -> String {
    let at = [
        "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
    ]
    .iter()
    .position(|name| *name == said)
    .unwrap_or_else(|| panic!("a colour of the terminal's own: {said}"));
    format!("38;5;{at}")
}

/// A role of the default theme as the escape tmux writes for text in it.
fn foreground(role: &str) -> String {
    let said = default_theme(role);
    match said.starts_with('#') {
        true => text_in(rgb(said)),
        false => text_named(said),
    }
}

/// And as the escape for a line drawn on it.
fn background(role: &str) -> String {
    let (r, g, b) = rgb(default_theme(role));
    format!("48;2;{r};{g};{b}")
}

/// Write a theme where the config file's `theme` name reaches it, which is the
/// directory beside the config a person keeps their own in.
fn theme(amx: &Harness, name: &str, text: &str) {
    let dir = amx.home().join(".config/amx/themes");
    std::fs::create_dir_all(&dir).expect("the themes directory");
    std::fs::write(dir.join(format!("{name}.toml")), text).expect("writing the theme");
}

/// The title claude gave the session, where the hook writes it: on the record,
/// at the end of a turn.
fn titled(amx: &Harness, id: &str, title: &str) {
    let mut state = amx.state(id);
    state["session_title"] = json!(title);
    amx.set_state(id, state);
}

/// And what somebody at the wall renamed the agent to.
fn renamed(amx: &Harness, id: &str, name: &str) {
    let mut state = amx.state(id);
    state["name"] = json!(name);
    amx.set_state(id, state);
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

/// Characters typed at a line the view has open, as against a key it acts on.
fn types(amx: &Harness, view: &str, text: &str) {
    amx.tmux(&["send-keys", "-t", view, "-l", text]);
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
        ["Needs input", "Working", "Completed"]
            .iter()
            .all(|group| drawn.contains(group))
            .then_some(drawn)
    });

    for id in ["ask-a1b", "port-import-b2c", "fix-login-c3d", "old-job-d4e"] {
        assert!(drawn.contains(id), "{id} is missing from:\n{drawn}");
    }
    // The agent sitting at its prompt is under the same heading as the one
    // whose command exited: both turns are over, and whether the process is
    // still there is the row's business rather than the group's.
    assert!(
        !drawn.contains("Idle"),
        "an ended turn is completed, so there is no group between:\n{drawn}"
    );
    assert!(
        line_of(&drawn, "Completed") < line_of(&drawn, "fix-login-c3d"),
        "the idle agent stands under Completed:\n{drawn}"
    );
    // A row says what the agent is up to: what it is asking, else what it is
    // doing, else what it answered.
    assert!(drawn.contains("Claude needs your permission"), "{drawn}");
    assert!(drawn.contains("Running Bash"), "{drawn}");
    assert!(drawn.contains("did what it was asked"), "{drawn}");

    // Twice over, in two vocabularies: the heading says what the group means,
    // and the band at the top says the word the list can be narrowed by.
    for group in ["Needs input", "Completed"] {
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
        drawn.contains("2 done"),
        "and the rest are counted beside it:\n{drawn}"
    );
}

#[test]
fn ctrl_t_pins_the_row_under_the_cursor_over_every_group_and_lets_it_go() {
    let amx = Harness::new();
    amx.play("ask-a1b", "asks-a-question");
    amx.play("port-import-b2c", "works-with-a-spinner");
    amx.until_state("ask-a1b", "waiting");
    amx.until_state("port-import-b2c", "working");

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("the two groups", || {
        let drawn = screen(&amx, &view);
        (drawn.contains("Needs input") && drawn.contains("Working")).then_some(())
    });

    // Onto the working agent: the view opens on the first row, and the walk
    // down takes the heading between them.
    press(&amx, &view, "Down");
    press(&amx, &view, "Down");
    press(&amx, &view, "C-t");
    let drawn = amx.until("the pinned group", || {
        let drawn = screen(&amx, &view);
        drawn.contains("Pinned").then_some(drawn)
    });

    assert!(
        line_of(&drawn, "Pinned") < line_of(&drawn, "Needs input"),
        "what somebody pinned stands over the agent that is asking:\n{drawn}"
    );
    assert_eq!(
        line_of(&drawn, "port-import-b2c"),
        line_of(&drawn, "Pinned") + 1,
        "and it is the row under the heading, whatever it is doing:\n{drawn}"
    );
    assert!(
        !drawn.contains("Working"),
        "the group it came out of was the last of it:\n{drawn}"
    );

    // And the same key lets it go, back under what it is doing.
    press(&amx, &view, "C-t");
    let back = amx.until("the working group again", || {
        let drawn = screen(&amx, &view);
        drawn.contains("Working").then_some(drawn)
    });
    assert!(!back.contains("Pinned"), "{back}");
    assert!(
        line_of(&back, "Working") < line_of(&back, "port-import-b2c"),
        "{back}"
    );
}

#[test]
fn z_puts_the_row_under_the_cursor_below_every_group_and_wakes_it_again() {
    let amx = Harness::new();
    amx.play("ask-a1b", "asks-a-question");
    amx.play("port-import-b2c", "works-with-a-spinner");
    amx.until_state("ask-a1b", "waiting");
    amx.until_state("port-import-b2c", "working");

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("the two groups", || {
        let drawn = screen(&amx, &view);
        (drawn.contains("Needs input") && drawn.contains("Working")).then_some(())
    });

    // The view opens on the agent that is asking, which is the one to put
    // away: whatever it is doing is where a sleeping row is not drawn.
    press(&amx, &view, "z");
    let drawn = amx.until("the sleeping group", || {
        let drawn = screen(&amx, &view);
        drawn.contains("Asleep").then_some(drawn)
    });

    assert!(
        line_of(&drawn, "Working") < line_of(&drawn, "Asleep"),
        "what somebody put away stands under the work that is running:\n{drawn}"
    );
    assert_eq!(
        line_of(&drawn, "ask-a1b"),
        line_of(&drawn, "Asleep") + 1,
        "and it is the row under the heading, though it is the one asking:\n{drawn}"
    );
    assert!(
        !drawn.contains("Needs input"),
        "the group it came out of was the last of it:\n{drawn}"
    );
    assert!(
        drawn.contains("1 WAITING"),
        "and the badge still counts it: where a row is drawn is no answer to \
         its question:\n{drawn}"
    );

    // The mark outlives the view that made it, so a terminal opened after it
    // draws the same wall.
    let again = amx.in_a_terminal(&[], &[]);
    let opened = amx.until("the second view", || {
        let drawn = screen(&amx, &again);
        (drawn.contains("Asleep") && drawn.contains("ask-a1b")).then_some(drawn)
    });
    assert!(
        line_of(&opened, "Asleep") < line_of(&opened, "ask-a1b"),
        "the next view opens on the wall the last one was left on:\n{opened}"
    );

    // And the word the header counts them by is the word that finds them
    // again on the find line.
    types(&amx, &view, "/");
    types(&amx, &view, "s:asleep");
    amx.until("the wall narrowed to the sleeping agent", || {
        let drawn = screen(&amx, &view);
        (drawn.contains("ask-a1b") && !drawn.contains("port-import-b2c")).then_some(())
    });
    press(&amx, &view, "Escape");
    amx.until("the whole fleet again", || {
        screen(&amx, &view)
            .contains("port-import-b2c")
            .then_some(())
    });

    // The same key wakes it, back under what it is doing. The foot of the
    // wall is where it was put, so that is where the cursor goes for it.
    press(&amx, &view, "G");
    press(&amx, &view, "z");
    let back = amx.until("the asking group again", || {
        let drawn = screen(&amx, &view);
        drawn.contains("Needs input").then_some(drawn)
    });
    assert!(!back.contains("Asleep"), "{back}");
    assert!(
        line_of(&back, "Needs input") < line_of(&back, "ask-a1b"),
        "{back}"
    );
}

#[test]
fn ready_for_review_takes_an_ended_agent_whose_request_is_still_open() {
    let amx = Harness::new();
    finished(&amx, "fix-login-a1b", "done", 60);
    a_request(&amx, "fix-login-a1b", 12, "open");
    finished(&amx, "port-import-b2c", "done", 30);
    a_request(&amx, "port-import-b2c", 9, "merged");
    finished(&amx, "old-job-c3d", "done", 90);

    let view = amx.in_a_terminal(&[], &[]);
    let drawn = amx.until("both groups", || {
        let drawn = screen(&amx, &view);
        (drawn.contains("Ready for review") && drawn.contains("Completed")).then_some(drawn)
    });

    assert!(
        line_of(&drawn, "Ready for review") < line_of(&drawn, "Completed"),
        "work waiting on a reviewer stands over the work that is over:\n{drawn}"
    );
    assert_eq!(
        line_of(&drawn, "fix-login-a1b"),
        line_of(&drawn, "Ready for review") + 1,
        "the agent whose request is still asking for something:\n{drawn}"
    );
    assert!(
        row_of(&amx, &view, "fix-login-a1b").is_some_and(|row| row.contains("#12")),
        "with the number the review is happening under:\n{drawn}"
    );
    for id in ["port-import-b2c", "old-job-c3d"] {
        assert!(
            line_of(&drawn, id) > line_of(&drawn, "Completed"),
            "a merged request and a branch nobody opened one for are both \
             over, so {id} is completed:\n{drawn}"
        );
    }
}

#[test]
fn glyphs_say_a_live_agent_from_an_ended_one_and_the_working_one_breathes() {
    let amx = Harness::new();
    amx.play("ask-a1b", "asks-a-question");
    amx.play("port-import-b2c", "works-with-a-spinner");
    amx.play("fix-login-c3d", "happy-turn");
    amx.until_state("ask-a1b", "waiting");
    amx.until_state("port-import-b2c", "working");
    amx.until_state("fix-login-c3d", "idle");
    finished(&amx, "old-job-d4e", "done", 60);

    // One shape while there is a process to go back to and another once there
    // is not, whatever the row is doing with it.
    let view = amx.in_a_terminal(&[], &[]);
    for (id, want) in [
        ("ask-a1b", '✻'),
        ("fix-login-c3d", '✻'),
        ("old-job-d4e", '∙'),
    ] {
        amx.until(&format!("{id} to be marked {want}"), || {
            (mark(&amx, &view, id) == Some(want)).then_some(())
        });
    }

    // What tells the two live ones apart is the colour on that one shape: the
    // row that wants a person is painted for it, and the row sitting at its
    // prompt is the quiet one.
    let asking = coloured_line(&amx, &view, "ask-a1b");
    assert!(
        asking.contains(&foreground("waiting")),
        "the waiting glyph is painted for what the row wants:\n{asking:?}"
    );
    let resting = coloured_line(&amx, &view, "fix-login-c3d");
    assert!(
        sgr_at(&resting, "✻").contains(&2),
        "and the idle one is dim:\n{resting:?}"
    );

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
fn glyphs_wear_a_dollar_on_the_rows_running_a_command() {
    let amx = Harness::new();
    // A shell row is a record with no agent on it, which is what `!cmd` and
    // `amx new --exec` write. One still running and one over, beside an agent
    // in each of the same two states, because what this is about is which of
    // the shapes belongs to which kind of row.
    let pane = a_pane_showing(&amx, &["cargo build"]);
    amx.record("build-a1b", &pane);
    amx.set_meta("build-a1b", json!({ "agent": null }));
    finished(&amx, "sweep-b2c", "done", 30);
    amx.set_meta("sweep-b2c", json!({ "agent": null }));
    amx.play("port-import-c3d", "works-with-a-spinner");
    amx.until_state("port-import-c3d", "working");
    finished(&amx, "old-job-d4e", "done", 60);

    let view = amx.in_a_terminal(&[], &[]);
    for id in ["build-a1b", "sweep-b2c"] {
        amx.until(&format!("{id} to be marked $"), || {
            (mark(&amx, &view, id) == Some('$')).then_some(())
        });
    }

    // The agent rows are what they were: the one whose turn is running is
    // drawn a frame at a time, and the one whose pane has gone rests on the
    // dot.
    amx.until("old-job-d4e to rest on the dot", || {
        (mark(&amx, &view, "old-job-d4e") == Some('∙')).then_some(())
    });
    let mut frames = std::collections::BTreeSet::new();
    amx.until("the working agent to breathe", || {
        frames.extend(mark(&amx, &view, "port-import-c3d"));
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
fn keys_v_says_which_vendor_model_and_effort_each_row_runs() {
    let amx = Harness::new();
    // The three kinds of row the column has anything to say about: a spawn
    // that turned both dials, one that turned neither, and a shell command,
    // which runs no vendor at all.
    finished(&amx, "fix-login-a1b", "done", 30);
    amx.set_meta(
        "fix-login-a1b",
        json!({ "model": "opus", "effort": "high" }),
    );
    finished(&amx, "port-b2c", "done", 60);
    let pane = a_pane_showing(&amx, &["cargo build"]);
    amx.record("build-c3d", &pane);
    amx.set_meta("build-c3d", json!({ "agent": null }));

    let ids = ["fix-login-a1b", "port-b2c", "build-c3d"];
    // The three rows or none of them: a capture can land mid-frame, and a
    // screen half written is one to look at again rather than one to fail on.
    let rows = |drawn: &str| -> Option<Vec<String>> {
        ids.iter()
            .map(|id| {
                drawn
                    .lines()
                    .find(|line| line.contains(id))
                    .map(str::to_string)
            })
            .collect()
    };
    let rows_of =
        |drawn: &str| rows(drawn).unwrap_or_else(|| panic!("the three rows in:\n{drawn}"));

    let view = amx.in_a_terminal(&[], &[]);
    let quiet = amx.until("the three rows", || {
        let drawn = screen(&amx, &view);
        rows(&drawn).map(|_| drawn)
    });
    for row in rows_of(&quiet) {
        assert!(
            !row.contains("claude"),
            "the column is not on the wall until somebody asks: {row:?}"
        );
    }

    // Waited for by all three rows, because all three are what is measured:
    // a capture that landed between the frame and the assertion would read as
    // a column that never came up.
    press(&amx, &view, "v");
    let loud = amx.until("what each row runs", || {
        let drawn = screen(&amx, &view);
        let said = rows(&drawn)?;
        (said[0].contains("claude opus high")
            && said[1].contains("claude")
            && said[2].contains("sh"))
        .then_some(drawn)
    });
    assert!(
        !rows_of(&loud)[1].contains("opus"),
        "a dial nobody turned is the vendor's own, and amx does not guess it: {:?}",
        rows_of(&loud)[1]
    );
    for (before, after) in rows_of(&quiet).iter().zip(rows_of(&loud)) {
        assert_eq!(
            before.find(char::is_alphanumeric),
            after.find(char::is_alphanumeric),
            "the name column does not move for it:\n{before:?}\n{after:?}"
        );
    }

    // The same key takes it away again.
    press(&amx, &view, "v");
    amx.until("the wall without it", || {
        let drawn = screen(&amx, &view);
        rows(&drawn)?
            .iter()
            .all(|row| !row.contains("claude"))
            .then_some(())
    });

    // And the choice outlives the view that made it: put back up, it is the
    // wall the next terminal opens on.
    press(&amx, &view, "v");
    amx.until("the column again", || {
        screen(&amx, &view)
            .contains("claude opus high")
            .then_some(())
    });
    let again = amx.in_a_terminal(&[], &[]);
    amx.until("the second view to open on the same wall", || {
        let drawn = screen(&amx, &again);
        let said = rows(&drawn)?;
        (said[0].contains("claude opus high") && said[2].contains("sh")).then_some(())
    });
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

#[test]
fn a_working_row_says_the_newest_line_of_its_transcript() {
    let amx = Harness::new();
    let mut pane_rows = vec!["✽ Nesting… (15s · ↓ 1.3k tokens)"];
    pane_rows.extend_from_slice(&CHROME);
    let pane = a_pane_showing(&amx, &pane_rows);
    amx.record("port-cli-b2c", &pane);

    // The transcript the session is writing, part way through a turn: the
    // agent has said a sentence and called nothing since.
    let transcript = amx.agent_dir("port-cli-b2c").join("session.jsonl");
    let said = "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\
                \"text\":\"The importer keeps its own clock.\"}]}}\n";
    std::fs::write(&transcript, said).expect("the transcript");
    amx.set_meta("port-cli-b2c", json!({ "transcript": transcript }));

    // The hooks went quiet ten minutes ago, on the call the record names.
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
    let row = amx.until("the row to say what the transcript says", || {
        row_of(&amx, &view, "port-cli-b2c").filter(|row| row.contains("The importer"))
    });
    assert!(
        row.contains("The importer keeps its own clock."),
        "the newest line of the conversation, between two calls:\n{row}"
    );
    assert!(
        !row.contains("Nesting"),
        "which is newer than the line the vendor spins:\n{row}"
    );

    // The next call lands: the row moves to the tool and the path it names.
    let call = "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"tool_use\",\
                \"name\":\"Read\",\"input\":{\"file_path\":\"src/importer.rs\"}}]}}\n";
    std::fs::write(&transcript, format!("{said}{call}")).expect("the transcript");

    let row = amx.until("the row to say the call the transcript names", || {
        row_of(&amx, &view, "port-cli-b2c").filter(|row| row.contains("Read src/importer.rs"))
    });
    assert!(
        !row.contains("Running Read"),
        "the tool and its path, not the record's word for the same call:\n{row}"
    );

    // Read and not written down, like the line over the composer: a transcript
    // is read again by whoever looks next.
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
        screen(&amx, &view).contains("Needs input").then_some(())
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
    // A heading is the whole of its line now, so the project it stands over is
    // what the line reads rather than what it starts with: ~ heads the agents
    // outside any repository, not the ones under ~/repo as well.
    let headings = |text: &str| {
        let heading = text.to_string();
        drawn
            .lines()
            .enumerate()
            .filter(move |(_, line)| line.trim_end() == heading)
    };
    let at = |text: &str| match headings(text).next() {
        Some((at, _)) => at,
        None => panic!("no {text} heading in:\n{drawn}"),
    };
    assert!(
        at("~/repo") < at("~"),
        "the project with the question in it comes first:\n{drawn}"
    );
    assert!(
        headings("~/repo").count() == 1,
        "one heading for the repository, subdirectory and all:\n{drawn}"
    );
    // The heading no longer says the state, so every row carries it.
    assert!(row("ask-a1b").contains("waiting"), "{drawn}");
    assert!(row("fix-login-b2c").contains("idle"), "{drawn}");
    assert!(row("old-job-c3d").contains("done"), "{drawn}");
    assert!(
        !drawn.contains("Needs input"),
        "and the state headings are gone with the axis:\n{drawn}"
    );

    // And on again: the repository each tree shares, then once more back to
    // what they need. `ctrl+s` walks three axes now.
    press(&amx, &view, "C-s");
    amx.until("the repository headings", || {
        let drawn = screen(&amx, &view);
        (drawn.contains("~/repo") && !drawn.contains("Needs input")).then_some(drawn)
    });
    press(&amx, &view, "C-s");
    amx.until("what they need again", || {
        screen(&amx, &view).contains("Needs input").then_some(())
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
    for group in [
        "Pinned",
        "Ready for review",
        "Needs input",
        "Working",
        "Completed",
    ] {
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
        drawn.contains("Needs input").then_some(drawn)
    });

    let lines: Vec<&str> = drawn.lines().map(str::trim_end).collect();
    let at = lines
        .iter()
        .position(|line| *line == "Needs input")
        .unwrap_or_else(|| panic!("no Needs input heading in:\n{drawn}"));
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
fn every_group_folds_past_thirty_rows_and_enter_opens_the_one_it_is_on() {
    let amx = Harness::new();
    // Sixty-four ended agents: thirty-two still standing in front of a
    // reviewer and thirty-two over, in a directory each. Both axes gather
    // them into two groups of thirty-two, so what folds is the same either
    // way.
    let reviews = amx.home().join("reviews");
    let shipped = amx.home().join("shipped");
    for at in [&reviews, &shipped] {
        std::fs::create_dir_all(at).expect("a place for them to have run");
    }
    for n in 0..32u64 {
        let id = format!("review-{n:02}");
        finished(&amx, &id, "done", (n + 1) * 60);
        a_request(&amx, &id, 100 + n, "open");
        running_in(&amx, &id, &reviews);
        let id = format!("over-{n:02}");
        finished(&amx, &id, "done", (n + 1) * 60);
        running_in(&amx, &id, &shipped);
    }

    // Room for every row a fold could give back, so nothing here is about
    // the height of the screen.
    let view = amx.in_a_terminal(&[], &[]);
    amx.tmux(&["resize-window", "-t", &view, "-x", "80", "-y", "80"]);
    let folded = amx.until("both groups folded", || {
        let drawn = screen(&amx, &view);
        (drawn.matches("2 more").count() == 2
            && drawn.contains("Ready for review")
            && drawn.contains("Completed"))
        .then_some(drawn)
    });
    assert_eq!(
        rows_of(&folded, "review-"),
        30,
        "thirty of thirty-two:\n{folded}"
    );
    assert_eq!(
        rows_of(&folded, "over-"),
        30,
        "and thirty of thirty-two:\n{folded}"
    );

    // Down the thirty rows onto the fold under them, and open it. That group
    // gives its rows back and the other one is holding its two still.
    for _ in 0..30 {
        press(&amx, &view, "Down");
    }
    press(&amx, &view, "Enter");
    let opened = amx.until("the group that was opened", || {
        let drawn = screen(&amx, &view);
        (rows_of(&drawn, "review-") == 32 && drawn.matches("2 more").count() == 1).then_some(drawn)
    });
    assert_eq!(rows_of(&opened, "over-"), 30, "{opened}");

    // The same fleet gathered by project folds the same way, and what was
    // opened under a group heading is not open under a path.
    press(&amx, &view, "C-s");
    let by_project = amx.until("both projects folded", || {
        let drawn = screen(&amx, &view);
        (drawn.contains("~/reviews") && drawn.matches("2 more").count() == 2).then_some(drawn)
    });
    assert_eq!(rows_of(&by_project, "review-"), 30, "{by_project}");
    assert_eq!(rows_of(&by_project, "over-"), 30, "{by_project}");
}

#[test]
fn a_row_brings_up_the_name_under_the_cursor_and_leaves_the_wall_quiet() {
    let amx = Harness::new();
    amx.play("ask-a1b", "asks-a-question");
    amx.until_state("ask-a1b", "waiting");
    finished(&amx, "fix-login-b2c", "done", 60);
    read(&amx, "fix-login-b2c");

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("both rows", || {
        let drawn = screen(&amx, &view);
        (drawn.contains("ask-a1b") && drawn.contains("fix-login-b2c")).then_some(())
    });

    // A row the cursor is not on: the name as quiet as what the agent said
    // beside it, and no weight anywhere on either.
    let quiet = coloured_line(&amx, &view, "fix-login-b2c");
    let name = sgr_at(&quiet, "fix-login-b2c");
    assert!(
        name.contains(&2) && !name.contains(&1),
        "the name is dim and carries no weight:\n{quiet:?}"
    );
    assert!(
        sgr_at(&quiet, "did what it was asked").contains(&2),
        "and what it said is drawn at the same strength:\n{quiet:?}"
    );
    assert!(
        quiet.contains(&foreground("done")),
        "the glyph alone carries the state's colour:\n{quiet:?}"
    );

    // The row the view opens on, which is the one asking: the name up at the
    // terminal's own strength in the colour of a thing waiting on a person, and
    // the question at full strength because it is the sentence somebody came to
    // read.
    let asking = coloured_line(&amx, &view, "ask-a1b");
    let name = sgr_at(&asking, "ask-a1b");
    assert!(
        !name.contains(&2) && !name.contains(&1),
        "the name under the cursor comes up without weight:\n{asking:?}"
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
    let row = amx.until("the row, drawn to the edge", || {
        row_of(&amx, &view, "fix-login-a1b").filter(|row| row.chars().count() == 80)
    });
    let cells: Vec<char> = row.chars().collect();
    assert_eq!(cells.len(), 80, "a row is drawn to the edge:\n{row:?}");

    // A blank cell of indent, the state glyph and the space after it, and
    // then the name column: sixteen cells of it below a hundred.
    let column = |from: usize, to: usize| cells[from..to].iter().collect::<String>();
    assert_eq!(column(3, 19), "fix-login-a1b   ", "{row:?}");
    assert_eq!(column(19, 21), "  ", "two cells stand the columns apart");

    // Then the summary, which takes whatever is left of the screen.
    assert_eq!(
        column(21, 74),
        format!("{:<53}", "did what it was asked"),
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
fn a_row_goes_under_the_title_its_session_was_given_until_somebody_renames_it() {
    let amx = Harness::new();
    finished(&amx, "port-import-b2c", "done", 60);
    titled(&amx, "port-import-b2c", "Importer clock");
    // A record carrying both, which is the one the order has to be read off.
    finished(&amx, "fix-login-a1b", "done", 120);
    titled(&amx, "fix-login-a1b", "Login timeout");
    renamed(&amx, "fix-login-a1b", "auth");

    let view = amx.in_a_terminal(&[], &[]);
    let row = amx.until("the row under the title claude gave the session", || {
        row_of(&amx, &view, "Importer clock")
    });
    assert!(
        !row.contains("port-import-b2c"),
        "the title stands where the id stood:\n{row}"
    );
    let both = row_of(&amx, &view, "auth").expect("the renamed row");
    assert!(
        !both.contains("Login timeout"),
        "and a name somebody typed here outranks the one the session goes \
         under:\n{both}"
    );

    // The find line reads the title the way it reads a name or an id: the row
    // is on the wall under a word of it, and the row it does not reach is off.
    types(&amx, &view, "/");
    types(&amx, &view, "clock");
    let drawn = amx.until("the wall narrowed to the title", || {
        let drawn = screen(&amx, &view);
        (drawn.contains("Importer clock") && !drawn.contains("auth")).then_some(drawn)
    });
    assert!(
        !drawn.contains("fix-login-a1b"),
        "the row the word misses is gone, id and all:\n{drawn}"
    );
}

#[test]
fn a_group_heading_is_the_groups_own_words_and_stops_there() {
    let amx = Harness::new();
    amx.play("ask-a1b", "asks-a-question");
    amx.until_state("ask-a1b", "waiting");
    finished(&amx, "one-b2c", "done", 60);
    finished(&amx, "two-c3d", "done", 120);

    let view = amx.in_a_terminal(&[], &[]);
    // Drawn whole, not merely drawn: the view puts out no synchronized-output
    // markers, so a capture can land partway through a line. Waiting for the
    // words the assertions below take is what makes them assertions rather
    // than a coin toss.
    let drawn = amx.until("both headings, whole", || {
        let drawn = screen(&amx, &view);
        let whole = ["Needs input", "Completed"]
            .iter()
            .all(|label| drawn.lines().any(|line| line.trim_end() == *label));
        whole.then_some(drawn)
    });

    // The words a person would say out loud, and the line ends on them: no
    // rule out to the edge, and no count while the rows the heading stands
    // over are on the screen to be counted.
    assert!(
        !drawn.contains('┈'),
        "nothing carries the eye out to the edge of the wall:\n{drawn}"
    );

    // Dim, the way the summary beside a row is, with the one exception the
    // wall makes up here: the group that wants a person says so in colour.
    // Read off the whole screen, because the dim the heading is drawn in was
    // turned on by the row above it and left in force.
    let painted = coloured(&amx, &view);
    let label = sgr_at(&painted, "Completed");
    assert!(
        label.contains(&2) && !label.contains(&1),
        "the label is dim and carries no weight:\n{painted:?}"
    );
    let waiting = coloured_line(&amx, &view, "Needs input");
    assert!(
        waiting.contains(&foreground("waiting")),
        "and the group that wants a person is painted for it:\n{waiting:?}"
    );
}

#[test]
fn a_path_heading_reads_the_way_a_group_heading_does() {
    let amx = Harness::new();
    let repo = amx.a_repo();
    amx.play("ask-a1b", "asks-a-question");
    amx.until_state("ask-a1b", "waiting");
    running_in(&amx, "ask-a1b", &repo);
    finished(&amx, "old-job-c3d", "done", 60);

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("the agents", || {
        screen(&amx, &view).contains("Needs input").then_some(())
    });
    press(&amx, &view, "C-s");
    let drawn = amx.until("the heading over the repository, whole", || {
        let drawn = screen(&amx, &view);
        let whole = drawn.lines().any(|line| line.trim_end() == "~/repo");
        whole.then_some(drawn)
    });

    // One document on either axis: the path and nothing after it, with the
    // count left to the rows under it the way a group's is.
    assert!(
        !drawn.contains('┈'),
        "no rule over a project either:\n{drawn}"
    );

    // Dim end to end, so the segment that says which directory this is reads
    // no louder than the parents it hangs off. Found by the segment alone,
    // because the two used to be told apart by an escape between them, and
    // read off the whole screen, because the dim carries down from the rows
    // above rather than being turned on again here.
    let painted = coloured(&amx, &view);
    for segment in ["~/", "repo"] {
        let cells = sgr_at(&painted, segment);
        assert!(
            cells.contains(&2) && !cells.contains(&1),
            "{segment} carries no more weight than the rest of the path:\n{painted:?}"
        );
    }
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
        screen(&amx, &view).contains("Needs input").then_some(())
    });
    press(&amx, &view, "C-s");
    let line = amx.until("the heading over the deep path, whole", || {
        screen(&amx, &view)
            .lines()
            .find(|line| line.starts_with("/srv/") && line.ends_with("legacy-shim"))
            .map(str::to_string)
    });

    assert!(
        line.chars().count() <= 80,
        "a path this long does not push the heading off the edge:\n{line:?}"
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
    let row = amx.until("the state word on the row, drawn to the edge", || {
        row_of(&amx, &view, "fix-login-a1b")
            .filter(|row| row.contains("done") && row.chars().count() == 80)
    });
    let cells: Vec<char> = row.chars().collect();
    let column = |from: usize, to: usize| cells[from..to].iter().collect::<String>();
    assert_eq!(cells.len(), 80, "a row is drawn to the edge:\n{row:?}");
    assert_eq!(column(3, 19), "fix-login-a1b   ", "the name has not moved");
    assert_eq!(column(19, 21), "  ", "two cells stand the columns apart");

    // The heading over the row is a place now, so the row says the state:
    // eight cells of it, which is what the longest of the words needs.
    assert_eq!(column(21, 29), "done    ", "{row:?}");
    assert_eq!(column(29, 31), "  ", "{row:?}");

    // The summary pays for all ten of those cells and nothing else does.
    assert_eq!(
        column(31, 74),
        format!("{:<43}", "did what it was asked"),
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
        .position(|line| line.trim_end() == "Completed")
        .expect("the heading") as u16
        + 1;
    click(&amx, &view, 5, heading);
    amx.until("the group shut", || {
        let drawn = screen(&amx, &view);
        (drawn.contains("Completed") && !drawn.contains("port-import-b2c")).then_some(())
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

/// The 1-based screen column a word starts at on an agent's row, for a drag
/// to be aimed at the cells it is drawn on.
/// Cells rather than bytes: the glyph a row opens with is three bytes of one
/// of them.
fn column_of(amx: &Harness, view: &str, id: &str, word: &str) -> u16 {
    let row = row_of(amx, view, id).unwrap_or_else(|| panic!("no row for {id}"));
    let at = row
        .find(word)
        .unwrap_or_else(|| panic!("no {word} on {row:?}"));
    row[..at].chars().count() as u16 + 1
}

#[test]
fn a_left_drag_reverses_the_cells_it_covers_and_copies_them_on_release() {
    let amx = Harness::new();
    finished(&amx, "fix-login-a1b", "done", 60);
    let view = amx.in_a_terminal(&[], &[]);
    // tmux only keeps a buffer of what a pane copies where it is asked to;
    // by default it hands the sequence to the outer terminal and keeps
    // nothing, and a buffer is the only way a test can read it back. Asked
    // of the server the terminal above started, since there is no server to
    // ask before it.
    amx.tmux(&["set-option", "-s", "set-clipboard", "on"]);
    amx.until("the row", || {
        screen(&amx, &view).contains("fix-login-a1b").then_some(())
    });

    // Press on the first cell of the id and drag to its last: the cells
    // between come up reversed while the button is held, which is what says
    // out loud what a release would copy.
    let row = screen_row_of(&amx, &view, "fix-login-a1b");
    let from = column_of(&amx, &view, "fix-login-a1b", "fix-login-a1b");
    let to = from + "fix-login-a1b".len() as u16 - 1;
    mouse(&amx, &view, 0, from, row, true);
    mouse(&amx, &view, 32, to, row, true);
    amx.until("the dragged cells reversed", || {
        let line = coloured_line(&amx, &view, "fix-login-a1b");
        sgr_at(&line, "fix-login-a1b").contains(&7).then_some(())
    });

    // And the release puts them on the clipboard by OSC 52, which this
    // server is holding as a buffer, and says so where the view says things.
    mouse(&amx, &view, 0, to, row, false);
    amx.until("the view to say it copied", || {
        screen(&amx, &view).contains("copied").then_some(())
    });
    assert_eq!(
        amx.tmux(&["show-buffer"]),
        "fix-login-a1b",
        "the id the drag covered and nothing either side of it"
    );
    assert!(
        !sgr_at(
            &coloured_line(&amx, &view, "fix-login-a1b"),
            "fix-login-a1b"
        )
        .contains(&7),
        "and the reverse went with the button"
    );
}

#[test]
fn hovering_a_row_tints_its_name_and_moves_no_cursor() {
    let amx = Harness::new();
    finished(&amx, "fix-login-a1b", "done", 60);
    finished(&amx, "port-import-b2c", "done", 120);
    // Both read, which the wall paints neither way, so what the pointer does to
    // a name is the whole of the difference between the two rows.
    read(&amx, "fix-login-a1b");
    read(&amx, "port-import-b2c");

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("both rows", || {
        let drawn = screen(&amx, &view);
        (drawn.contains("fix-login-a1b") && drawn.contains("port-import-b2c")).then_some(())
    });

    // The pointer comes to rest on the row the cursor is not on.
    let row = screen_row_of(&amx, &view, "port-import-b2c");
    mouse(&amx, &view, 35, 5, row, true);
    amx.until("the name to take the tint", || {
        let name = sgr_at(
            &coloured_line(&amx, &view, "port-import-b2c"),
            "port-import-b2c",
        );
        (!name.contains(&2) && !name.contains(&1)).then_some(())
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

/// The name on the card's rule, where a card is open: the one row a card
/// draws whatever it is a look at, and the only line of the screen that starts
/// at the column the list indents its rows past.
fn card_rule(drawn: &str) -> Option<String> {
    drawn
        .lines()
        .find(|line| line.contains('┈') && !line.starts_with(' '))
        .and_then(|line| line.split_whitespace().next())
        .map(str::to_string)
}

/// Whether the cursor's bar is on an agent's row of the list, told from the
/// card's rule below it by the summary only a row carries.
fn barred(amx: &Harness, view: &str, id: &str) -> bool {
    coloured(amx, view)
        .lines()
        .any(|line| line.contains(id) && line.contains("did what") && line.contains(&bar()))
}

#[test]
fn space_and_l_open_the_card_on_the_row_under_the_pointer() {
    let amx = Harness::new();
    finished(&amx, "first-a1b", "done", 60);
    finished(&amx, "second-b2c", "done", 120);
    finished(&amx, "third-c3d", "done", 180);

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("three rows with the bar on the first", || {
        let drawn = screen(&amx, &view);
        (drawn.contains("second-b2c")
            && drawn.contains("third-c3d")
            && barred(&amx, &view, "first-a1b"))
        .then_some(())
    });

    // The pointer comes to rest on the third row. Space is read where it is
    // resting, so the card is that row's and the cursor went with it.
    mouse(
        &amx,
        &view,
        35,
        5,
        screen_row_of(&amx, &view, "third-c3d"),
        true,
    );
    press(&amx, &view, "Space");
    amx.until("the third row's card", || {
        (card_rule(&screen(&amx, &view)).as_deref() == Some("third-c3d")).then_some(())
    });
    assert!(
        barred(&amx, &view, "third-c3d"),
        "the cursor landed where the pointer was"
    );

    // The same of `l`, with the pointer back on the first row.
    press(&amx, &view, "Escape");
    amx.until("the card away", || {
        card_rule(&screen(&amx, &view)).is_none().then_some(())
    });
    mouse(
        &amx,
        &view,
        35,
        5,
        screen_row_of(&amx, &view, "first-a1b"),
        true,
    );
    press(&amx, &view, "l");
    amx.until("the first row's card", || {
        (card_rule(&screen(&amx, &view)).as_deref() == Some("first-a1b")).then_some(())
    });
    assert!(
        barred(&amx, &view, "first-a1b"),
        "the cursor landed where the pointer was"
    );

    // And with the pointer off the list, space is the cursor's card: the
    // walk down moves it, and the row the pointer last rested on is not it.
    press(&amx, &view, "Escape");
    amx.until("the card away", || {
        card_rule(&screen(&amx, &view)).is_none().then_some(())
    });
    mouse(&amx, &view, 35, 5, 1, true);
    press(&amx, &view, "j");
    press(&amx, &view, "Space");
    amx.until("the second row's card", || {
        (card_rule(&screen(&amx, &view)).as_deref() == Some("second-b2c")).then_some(())
    });
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
        !coloured_line(&amx, &view, "Needs input").contains(&bar()),
        "and not under the heading the cursor is not on"
    );

    press(&amx, &view, "Up");
    amx.until("the bar to move up onto the heading", || {
        coloured_line(&amx, &view, "Needs input")
            .contains(&bar())
            .then_some(())
    });
    assert!(
        !coloured_line(&amx, &view, "ask-a1b").contains(&bar()),
        "one line at a time, whatever kind of line it is"
    );
}

#[test]
fn w_lands_the_bar_on_the_first_agent_that_needs_you() {
    let amx = Harness::new();
    amx.play("ask-a1b", "asks-a-question");
    amx.play("port-import-b2c", "works-with-a-spinner");
    amx.until_state("ask-a1b", "waiting");
    amx.until_state("port-import-b2c", "working");

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("the two groups", || {
        let drawn = screen(&amx, &view);
        (drawn.contains("Needs input") && drawn.contains("Working")).then_some(())
    });

    // The working agent pinned over the rest, which puts the one that is
    // asking below it: the key has to reach down the wall and not just to the
    // top of it.
    press(&amx, &view, "Down");
    press(&amx, &view, "Down");
    press(&amx, &view, "C-t");
    let drawn = amx.until("the pinned group", || {
        let drawn = screen(&amx, &view);
        drawn.contains("Pinned").then_some(drawn)
    });
    assert!(
        line_of(&drawn, "port-import-b2c") < line_of(&drawn, "ask-a1b"),
        "the agent that is asking stands under the working one:\n{drawn}"
    );

    // From the top of the wall, which is the heading over the pinned row: the
    // question is about the wall rather than about the line the cursor is on.
    twice(&amx, &view, "g");
    amx.until("the bar at the top", || {
        coloured_line(&amx, &view, "Pinned")
            .contains(&bar())
            .then_some(())
    });

    press(&amx, &view, "w");
    amx.until("the bar on the agent that is asking", || {
        coloured_line(&amx, &view, "ask-a1b")
            .contains(&bar())
            .then_some(())
    });
    let drawn = screen(&amx, &view);
    assert!(
        !coloured_line(&amx, &view, "Pinned").contains(&bar()),
        "and off the line it was pressed on:\n{drawn}"
    );
    assert!(
        !drawn
            .lines()
            .any(|line| line.starts_with("ask-a1b · claude ┈")),
        "the cursor is all the key moves, so nothing is opened over the wall:\n{drawn}"
    );
}

#[test]
fn the_vim_letters_walk_the_bar_and_go_in_and_out_of_the_card() {
    // Every card opens with a line at its foot, and every letter is text the
    // moment one is open: these letters are keys on the wall and characters
    // over a card, which is why esc is what comes back out.
    let amx = Harness::new();
    finished(&amx, "done-a1b", "done", 60);

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("the row", || {
        screen(&amx, &view).contains("done-a1b").then_some(())
    });
    amx.until("the bar under the cursor", || {
        coloured_line(&amx, &view, "done-a1b")
            .contains(&bar())
            .then_some(())
    });

    // k walks up onto the heading exactly as the arrow does, and j back down.
    press(&amx, &view, "k");
    amx.until("the bar to move up onto the heading", || {
        coloured_line(&amx, &view, "Completed")
            .contains(&bar())
            .then_some(())
    });
    press(&amx, &view, "j");
    amx.until("the bar back on the row", || {
        coloured_line(&amx, &view, "done-a1b")
            .contains(&bar())
            .then_some(())
    });

    // l goes in to the card and esc comes back out. The view still has the
    // terminal either way: an attach would have handed it to tmux, and the
    // wall would be gone rather than standing over a card.
    let carded = |drawn: &str| {
        drawn
            .lines()
            .any(|line| line.starts_with("done-a1b · claude ┈"))
    };
    press(&amx, &view, "l");
    amx.until("the card", || carded(&screen(&amx, &view)).then_some(()));

    // And h, which used to close it, is a character of the line the card
    // opened with: the card stands where it was with an h typed at it.
    press(&amx, &view, "h");
    let typed = amx.until("the letter on the line", || {
        let drawn = screen(&amx, &view);
        drawn.contains("❯ h").then_some(drawn)
    });
    assert!(
        carded(&typed),
        "with the card still open under it:\n{typed}"
    );

    press(&amx, &view, "Escape");
    amx.until("the card put away", || {
        let drawn = screen(&amx, &view);
        (!carded(&drawn) && drawn.contains("done-a1b")).then_some(())
    });
}

#[test]
fn gg_and_g_reach_the_two_ends_of_the_list_and_one_g_waits() {
    let amx = Harness::new();
    finished(&amx, "one-a1b", "done", 60);
    finished(&amx, "two-b2c", "done", 120);

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("both agents", || {
        let drawn = screen(&amx, &view);
        (drawn.contains("one-a1b") && drawn.contains("two-b2c")).then_some(())
    });

    // G is one press and lands on the last row there is.
    press(&amx, &view, "G");
    amx.until("the bar at the foot", || {
        coloured_line(&amx, &view, "two-b2c")
            .contains(&bar())
            .then_some(())
    });

    // One g moves nothing and says on the keys row that it is waiting.
    press(&amx, &view, "g");
    let waiting = amx.until("the row to say a g is waiting", || {
        let drawn = screen(&amx, &view);
        drawn.contains("g again").then_some(drawn)
    });
    assert!(
        coloured_line(&amx, &view, "two-b2c").contains(&bar()),
        "and the cursor has not moved for it:\n{waiting}"
    );

    // The second one goes to the top, which is the heading over the group.
    press(&amx, &view, "g");
    amx.until("the bar at the top", || {
        let drawn = screen(&amx, &view);
        (coloured_line(&amx, &view, "Completed").contains(&bar()) && !drawn.contains("g again"))
            .then_some(())
    });
}

#[test]
fn enter_shuts_the_group_its_headings_stand_over_and_opens_it_again() {
    let amx = Harness::new();
    finished(&amx, "one-a1b", "done", 60);
    finished(&amx, "two-b2c", "failed", 120);

    let view = amx.in_a_terminal(&[], &[]);
    // The heading whole, not merely started: a capture can land partway
    // through the line it is drawn on.
    let drawn = amx.until("both agents under a heading that says so", || {
        let drawn = screen(&amx, &view);
        (drawn.contains("one-a1b")
            && drawn.contains("two-b2c")
            && drawn.contains("Completed · 1 failed"))
        .then_some(drawn)
    });
    let heading = |drawn: &str| {
        drawn
            .lines()
            .find(|line| line.trim_end().starts_with("Completed"))
            .unwrap_or_else(|| panic!("no Completed heading in:\n{drawn}"))
            .trim_end()
            .to_string()
    };
    assert_eq!(
        heading(&drawn),
        "Completed · 1 failed",
        "a heading says how many failed under it, and leaves the counting of \
         the rows to the rows:\n{drawn}"
    );

    // Up off the first agent onto the heading over it, and shut the group.
    press(&amx, &view, "Up");
    press(&amx, &view, "Enter");
    let shut = amx.until("the group to be put away", || {
        let drawn = screen(&amx, &view);
        (!drawn.contains("one-a1b") && drawn.contains("Completed 2 · 1 failed")).then_some(drawn)
    });
    assert!(!shut.contains("two-b2c"), "the rows are away:\n{shut}");
    assert_eq!(
        heading(&shut),
        "Completed 2 · 1 failed",
        "the count comes up to stand for the rows that have gone, and the \
         failures keep their place after it:\n{shut}"
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
fn backspace_puts_the_cursor_on_the_agent_the_terminal_was_last_in() {
    let amx = Harness::new();
    let view = amx.in_a_terminal(&[], &[]);
    let holding = pane_field(&amx, &view, "#{session_name}");
    until_empty(&amx, &view);

    // A client on the view, because the trail is written where somebody went
    // and a press that moved nobody is nowhere anybody has been.
    let terminal = watching(&amx, &holding);
    let tty = amx.until("a client on the view", || {
        let clients = clients_on(&amx, &holding);
        (!clients.is_empty()).then_some(clients)
    });

    an_agent_session(&amx, "fix-login-a1b");
    an_agent_session(&amx, "port-import-b2c");
    let drawn = amx.until("both rows", || {
        let drawn = screen(&amx, &view);
        (drawn.contains("fix-login-a1b") && drawn.contains("port-import-b2c")).then_some(drawn)
    });
    // Whichever of them the wall drew first, which is where the view opens the
    // cursor: the order of the two rows is the list's business, and the walk
    // down is the other one.
    let (first, second) =
        match line_of(&drawn, "fix-login-a1b") < line_of(&drawn, "port-import-b2c") {
            true => ("fix-login-a1b", "port-import-b2c"),
            false => ("port-import-b2c", "fix-login-a1b"),
        };

    // Into the agent under the cursor and back out the way somebody comes
    // back: the client moves to it, and the view goes on drawing behind.
    let go_in_and_out = |id: &str| {
        press(&amx, &view, "Enter");
        amx.until("the client on the agent", || {
            (clients_on(&amx, &format!("amx-{id}")) == tty).then_some(())
        });
        amx.tmux(&["switch-client", "-c", &tty, "-t", &holding]);
        amx.until("the list again", || {
            screen(&amx, &terminal).contains("? keys").then_some(())
        });
    };

    go_in_and_out(first);
    press(&amx, &view, "Down");
    amx.until("the cursor on the other row", || {
        coloured_line(&amx, &view, second)
            .contains(&bar())
            .then_some(())
    });
    go_in_and_out(second);

    // One press back along the trail: the row the cursor is standing on is
    // the agent somebody is in, so going back is the one before it.
    press(&amx, &view, "BSpace");
    amx.until("the cursor on the agent before it", || {
        coloured_line(&amx, &view, first)
            .contains(&bar())
            .then_some(())
    });
    assert!(
        !coloured_line(&amx, &view, second).contains(&bar()),
        "and off the row it was pressed on:\n{}",
        screen(&amx, &view)
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
fn ctrl_z_in_the_session_gives_the_view_its_terminal_back() {
    let amx = Harness::new();
    let view = outside_tmux(&amx);
    until_empty(&amx, &view);

    an_agent_session(&amx, "fix-login-a1b");
    amx.until("the row", || row_of(&amx, &view, "fix-login-a1b").map(drop));

    press(&amx, &view, "Enter");
    amx.until("the agent on the screen", || {
        screen(&amx, &view)
            .contains("the agent at work")
            .then_some(())
    });

    // The key is pressed at the terminal the view lent out, which is where
    // somebody looking at the agent is. It is the client tmux put there that
    // reads it, not the view, and detaching is what hands the terminal back.
    press(&amx, &view, "C-z");
    amx.until("the list again", || {
        screen(&amx, &view).contains("? keys").then_some(())
    });
    assert!(
        row_of(&amx, &view, "fix-login-a1b").is_some(),
        "with the agent still on it"
    );
}

#[test]
fn ctrl_z_in_the_session_moves_the_client_back_to_the_view_inside_tmux() {
    let amx = Harness::new();
    let view = amx.in_a_terminal(&[], &[]);
    let holding = pane_field(&amx, &view, "#{session_name}");
    until_empty(&amx, &view);

    let terminal = watching(&amx, &holding);
    let tty = amx.until("a client on the view", || {
        let clients = clients_on(&amx, &holding);
        (!clients.is_empty()).then_some(clients)
    });

    an_agent_session(&amx, "fix-login-a1b");
    amx.until("the row", || row_of(&amx, &view, "fix-login-a1b").map(drop));

    press(&amx, &view, "Enter");
    amx.until("the client on the agent", || {
        (clients_on(&amx, "amx-fix-login-a1b") == tty).then_some(())
    });

    // The key is pressed at the client's own terminal, which is the pane it
    // runs in, and the client goes back to the session the view never left.
    press(&amx, &terminal, "C-z");
    amx.until("the client on the view again", || {
        (clients_on(&amx, &holding) == tty).then_some(())
    });
    assert!(
        row_of(&amx, &view, "fix-login-a1b").is_some(),
        "with the agent still on it"
    );
}

#[test]
fn the_row_the_terminal_came_back_from_is_the_one_the_accent_marks() {
    // Two agents sitting at their prompts, which is a wall of rows that look
    // alike: the same glyph, the same weight, and nothing to say which of them
    // somebody has just been inside.
    let amx = Harness::new();
    amx.play("fix-login-a1b", "happy-turn");
    amx.play("port-import-b2c", "happy-turn");
    amx.until_state("fix-login-a1b", "idle");
    amx.until_state("port-import-b2c", "idle");

    let view = outside_tmux(&amx);
    let drawn = amx.until("both rows", || {
        let drawn = screen(&amx, &view);
        (drawn.contains("fix-login-a1b") && drawn.contains("port-import-b2c")).then_some(drawn)
    });
    // The cursor opens on the first row, and which of the two that is is
    // whichever of them ended last, so it is read off the screen.
    let (went_into, stayed) =
        match line_of(&drawn, "fix-login-a1b") < line_of(&drawn, "port-import-b2c") {
            true => ("fix-login-a1b", "port-import-b2c"),
            false => ("port-import-b2c", "fix-login-a1b"),
        };

    // In, the way enter takes somebody in outside tmux: the terminal is the
    // view's to lend, and the agent's own screen comes up on it. The footer is
    // the last thing the scenario prints, so waiting on it waits for the whole
    // screen.
    press(&amx, &view, "Enter");
    amx.until("the agent's own screen", || {
        screen(&amx, &view)
            .contains("⏵⏵ auto mode on")
            .then_some(())
    });

    // And out the way somebody who is looking at a session gets out: the tmux
    // prefix and d, at the client the view handed the terminal to.
    press(&amx, &view, "C-b");
    press(&amx, &view, "d");
    amx.until("the wall again", || {
        screen(&amx, &view).contains("? keys").then_some(())
    });

    // The cursor onto the other row, so what is left on the first name is the
    // mark rather than the bar that follows the cursor about.
    press(&amx, &view, "Down");
    amx.until("the cursor on the row nobody went into", || {
        coloured_line(&amx, &view, stayed)
            .contains(&bar())
            .then_some(())
    });

    let marked = coloured_line(&amx, &view, went_into);
    assert!(
        marked.contains(&foreground("accent")),
        "the name of the agent the terminal came back from is in the \
         accent:\n{marked:?}"
    );
    let rest = coloured_line(&amx, &view, stayed);
    assert!(
        !rest.contains(&foreground("accent")),
        "and the row nobody went into is the terminal's own:\n{rest:?}"
    );
}

#[test]
fn the_row_the_client_came_back_from_is_marked_inside_tmux_too() {
    // The view's other way in, and the one most people are on: inside tmux
    // the terminal is not the view's to lend, so enter moves the client to the
    // agent's session and the view keeps drawing behind it. Coming back is the
    // client moving again, and the wall has to say where it has been the same
    // as when it lent the terminal out.
    let amx = Harness::new();
    amx.play("fix-login-a1b", "happy-turn");
    amx.play("port-import-b2c", "happy-turn");
    amx.until_state("fix-login-a1b", "idle");
    amx.until_state("port-import-b2c", "idle");

    let view = amx.in_a_terminal(&[], &[]);
    let holding = pane_field(&amx, &view, "#{session_name}");
    // Without a client there is nothing for enter to move.
    watching(&amx, &holding);
    let tty = amx.until("a client on the view", || {
        let clients = clients_on(&amx, &holding);
        (!clients.is_empty()).then_some(clients)
    });

    let drawn = amx.until("both rows", || {
        let drawn = screen(&amx, &view);
        (drawn.contains("fix-login-a1b") && drawn.contains("port-import-b2c")).then_some(drawn)
    });
    let (went_into, stayed) =
        match line_of(&drawn, "fix-login-a1b") < line_of(&drawn, "port-import-b2c") {
            true => ("fix-login-a1b", "port-import-b2c"),
            false => ("port-import-b2c", "fix-login-a1b"),
        };

    // The session the agent's pane is in, by name: a harness pane is not one
    // `new` named, so the pane is asked rather than the id spelled.
    let into = pane_field(&amx, &amx.pane_of(went_into), "#{session_name}");
    press(&amx, &view, "Enter");
    amx.until("the client on the agent", || {
        (clients_on(&amx, &into) == tty).then_some(())
    });

    // Back to the view the way tmux brings somebody back: the same client,
    // moved to the session it left.
    amx.tmux(&["switch-client", "-c", &tty, "-t", &holding]);
    amx.until("the client on the view again", || {
        (clients_on(&amx, &holding) == tty).then_some(())
    });

    press(&amx, &view, "Down");
    amx.until("the cursor on the row nobody went into", || {
        coloured_line(&amx, &view, stayed)
            .contains(&bar())
            .then_some(())
    });

    let marked = coloured_line(&amx, &view, went_into);
    assert!(
        marked.contains(&foreground("accent")),
        "the name of the agent the client came back from is in the \
         accent:\n{marked:?}"
    );
    let rest = coloured_line(&amx, &view, stayed);
    assert!(
        !rest.contains(&foreground("accent")),
        "and the row nobody went into is the terminal's own:\n{rest:?}"
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
fn i_cuts_short_the_turn_the_row_under_the_cursor_is_on() {
    let amx = Harness::new();
    let pane = amx.play("port-import-c3d", "interrupted");
    amx.until_state("port-import-c3d", "working");

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("the row", || row_of(&amx, &view, "port-import-c3d"));
    // Gathered by the project, where a row carries its state as a word of its
    // own rather than standing under a heading that says it.
    press(&amx, &view, "C-s");
    amx.until("the working row", || {
        row_of(&amx, &view, "port-import-c3d").filter(|row| row.contains("working"))
    });

    // The view opens on the first row, which is the one agent there is.
    press(&amx, &view, "i");
    let said = amx.until("what the view says it did", || {
        screen(&amx, &view)
            .lines()
            .rfind(|line| line.contains("interrupted"))
            .map(str::to_string)
    });
    assert!(said.contains("interrupted port-import-c3d"), "{said}");

    // The key reached the vendor and not only the record: this scenario sits
    // on its stdin until Escape arrives, and draws its prompt only then.
    amx.until("the vendor to go back to its prompt", || {
        amx.capture(&pane).contains("⏵⏵").then_some(())
    });
    let row = amx.until("the row off the turn it was on", || {
        row_of(&amx, &view, "port-import-c3d").filter(|row| row.contains("idle"))
    });
    assert!(!row.contains("working"), "{row}");

    // What the press wrote down is the verb's own, so a `result` waiting on
    // this turn is told there is no answer coming.
    let kinds = amx.event_kinds("port-import-c3d");
    assert!(
        kinds.iter().any(|kind| kind == "interrupt"),
        "the turn the view cut short is on the log: {kinds:?}"
    );
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
        drawn
            .contains("ctrl+x again stops and forgets")
            .then_some(drawn)
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
fn acts_ctrl_x_on_a_heading_arms_rows_in_every_state_before_it_stops_any() {
    let amx = Harness::new();
    // A live agent sitting at its prompt and a finished one, both in the
    // harness's home: one project heading stands over both states at once.
    let pane = amx.play("fix-login-a1b", "happy-turn");
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
            .any(|line| line.starts_with("~ "))
            .then_some(())
    });

    // Up from the row the view opens on is the heading. One press arms every
    // row under it, whatever its state — no group is refused any more — and
    // stops nothing.
    press(&amx, &view, "Up");
    press(&amx, &view, "C-x");
    let armed = amx.until("both rows to be armed", || {
        let drawn = screen(&amx, &view);
        (drawn.matches("ctrl+x again stops and forgets").count() == 2).then_some(drawn)
    });
    assert!(
        !armed.contains("has finished"),
        "the refusal went with the rule:\n{armed}"
    );
    assert_eq!(agents(&amx).len(), 2, "and arming forgets nothing");
    assert_eq!(
        amx.state("fix-login-a1b")["state"],
        "idle",
        "the press that armed the group stopped nothing in it"
    );
    assert!(
        amx.pane_alive(&pane),
        "the live agent is sitting at its prompt with its pane"
    );

    // Two presses whatever the clock did to the first window: if it is still
    // open the first of these stops the live agent and forgets both, and if it
    // lapsed the first re-arms and the second does it.
    twice(&amx, &view, "C-x");
    amx.until("the group to be stopped and forgotten", || {
        agents(&amx).is_empty().then_some(())
    });
    amx.until("the live agent's pane to go with it", || {
        (!amx.pane_alive(&pane)).then_some(())
    });
    // The project axis has no welcome line: a list of places with nothing to
    // arrange says so plainly.
    amx.until("the empty wall", || {
        screen(&amx, &view).contains("no agents").then_some(())
    });
}

/// git as these tests run it: none of the developer's own configuration, and
/// an identity of its own for the commits and merges they make.
fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_AUTHOR_NAME", "amx tests")
        .env("GIT_AUTHOR_EMAIL", "tests@example.invalid")
        .env("GIT_COMMITTER_NAME", "amx tests")
        .env("GIT_COMMITTER_EMAIL", "tests@example.invalid")
        .output()
        .unwrap_or_else(|e| panic!("git {args:?}: {e}"));
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// An agent with a tree of its own in `repo`, played to the end the ordinary
/// way: it answered and stopped. The tree it left is where its work is.
fn an_ended_agent(amx: &Harness, id: &str, repo: &Path) -> String {
    let out = amx
        .amx_command(&[
            "new",
            "--name",
            id,
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
    amx.until_state(id, "done");
    amx.meta(id)["worktree"]
        .as_str()
        .expect("a worktree")
        .to_string()
}

/// A commit of the agent's own, which is what puts its branch somewhere main
/// is not.
fn work_on_the_branch(tree: &str, name: &str) {
    let tree = Path::new(tree);
    std::fs::write(tree.join(name), "fn login() {}\n").expect("a file to commit");
    git(tree, &["add", name]);
    git(tree, &["commit", "-m", "fix the login bug"]);
}

/// What a look at the forge would have written down beside the record: the
/// request on this agent's branch, and that it went in.
fn a_merged_request(amx: &Harness, id: &str, number: u64) {
    std::fs::write(
        amx.agent_dir(id).join("pr.json"),
        json!({
            "asked": now(),
            "branch": format!("amx/{id}"),
            "prs": [{ "number": number, "standing": "merged" }],
        })
        .to_string(),
    )
    .expect("writing pr.json");
}

/// The other way work lands: somebody merged the branch themselves.
fn merged_by_hand(repo: &Path, id: &str) {
    git(
        repo,
        &["merge", "--no-ff", "-m", "merge", &format!("amx/{id}")],
    );
}

/// The wall the sweep has something to say about: two ended agents with work
/// on branches of their own, one whose request the forge says went in and one
/// whose branch somebody merged themselves. Their trees, in that order.
fn two_agents_whose_work_landed(amx: &Harness, repo: &Path) -> (String, String) {
    let landed = an_ended_agent(amx, "fix-login-a1b", repo);
    work_on_the_branch(&landed, "login.rs");
    a_merged_request(amx, "fix-login-a1b", 12);

    let merged = an_ended_agent(amx, "tidy-b2c", repo);
    work_on_the_branch(&merged, "search.rs");
    merged_by_hand(repo, "tidy-b2c");

    (landed, merged)
}

/// Wait for the rows the first `c` marked, each saying its own reason.
fn until_armed(amx: &Harness, view: &str) -> String {
    amx.until("both rows to say why their work has landed", || {
        let drawn = screen(amx, view);
        (drawn.contains("c again clears · #12 merged")
            && drawn.contains("c again clears · amx/tidy-b2c merged into main"))
        .then_some(drawn)
    })
}

#[test]
fn c_clears_the_agents_whose_work_has_landed_and_says_why_on_each_row() {
    let amx = Harness::new();
    let repo = amx.a_repo();
    let (landed, merged) = two_agents_whose_work_landed(&amx, &repo);

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("both rows", || {
        let drawn = screen(&amx, &view);
        (drawn.contains("fix-login-a1b") && drawn.contains("tidy-b2c")).then_some(())
    });

    // One press marks every row the sweep found, wherever the cursor is
    // standing, and each row says the reason it was found by where its summary
    // was. Nothing is taken for it.
    press(&amx, &view, "c");
    let armed = until_armed(&amx, &view);
    assert!(
        !armed.contains("ctrl+x again"),
        "the rows name the key that armed them and not the other one:\n{armed}"
    );
    assert_eq!(
        agents(&amx),
        ["fix-login-a1b", "tidy-b2c"],
        "and one press clears nothing:\n{armed}"
    );

    // The press inside the window takes both: the record, the tree and the
    // branch, and the line at the foot says how many went.
    press(&amx, &view, "c");
    amx.until("the records to go", || {
        agents(&amx).is_empty().then_some(())
    });
    amx.until("the count", || {
        screen(&amx, &view).contains("cleared 2").then_some(())
    });
    for tree in [&landed, &merged] {
        assert!(!Path::new(tree).exists(), "{tree} went with its record");
    }
    let left = git(&repo, &["branch", "--list"]);
    assert!(!left.contains("amx/"), "and so did their branches: {left}");
}

#[test]
fn c_whose_window_lapses_keeps_every_agent_it_marked() {
    let amx = Harness::new();
    let repo = amx.a_repo();
    let (landed, merged) = two_agents_whose_work_landed(&amx, &repo);

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("both rows", || {
        let drawn = screen(&amx, &view);
        (drawn.contains("fix-login-a1b") && drawn.contains("tidy-b2c")).then_some(())
    });
    press(&amx, &view, "c");
    until_armed(&amx, &view);

    // Nobody answers, so the window closes on its own and the rows go back to
    // saying what their agents did.
    let back = amx.until("the summaries to come back", || {
        let drawn = screen(&amx, &view);
        (!drawn.contains("c again clears")).then_some(drawn)
    });
    assert!(
        back.contains("fix-login-a1b") && back.contains("tidy-b2c"),
        "both agents are still on the wall:\n{back}"
    );
    assert_eq!(agents(&amx), ["fix-login-a1b", "tidy-b2c"]);
    for tree in [&landed, &merged] {
        assert!(Path::new(tree).exists(), "{tree} stands");
    }
    let left = git(&repo, &["branch", "--list"]);
    for id in ["fix-login-a1b", "tidy-b2c"] {
        assert!(
            left.contains(&format!("amx/{id}")),
            "and its branch: {left}"
        );
    }
}

#[test]
fn c_keeps_back_the_landed_agent_whose_tree_holds_work_and_counts_it() {
    let amx = Harness::new();
    let repo = amx.a_repo();
    let (clean, holding) = two_agents_whose_work_landed(&amx, &repo);

    // Both branches are in the main line, but somebody left a file in the
    // second tree that no commit has. The sweep keeps such a tree and the
    // record that names it, so the view has it to say before the press that
    // would otherwise clear the row.
    std::fs::write(Path::new(&holding).join("notes.md"), "half an idea\n")
        .expect("a file nobody committed");

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("both rows", || {
        let drawn = screen(&amx, &view);
        (drawn.contains("fix-login-a1b") && drawn.contains("tidy-b2c")).then_some(())
    });

    // The first press asks git of every tree it found, and the row behind the
    // one holding work says so where the others say what the next press does.
    press(&amx, &view, "c");
    let armed = amx.until("the row that will be kept to say why", || {
        let drawn = screen(&amx, &view);
        drawn.contains("holds work no commit has").then_some(drawn)
    });
    let held_row = row_of(&amx, &view, "tidy-b2c").expect("the row still on the wall");
    assert!(
        held_row.contains("holds work no commit has") && !held_row.contains("c again clears"),
        "the held row says why rather than promising a press that passes it \
         by:\n{armed}"
    );
    assert!(
        row_of(&amx, &view, "fix-login-a1b")
            .is_some_and(|row| row.contains("c again clears · #12 merged")),
        "and the row with nothing uncommitted in it says what it said:\n{armed}"
    );

    // The press inside the window takes the clean row and goes past the held
    // one, and the line at the foot says how many stayed and why.
    press(&amx, &view, "c");
    amx.until("the count of what went and what was kept", || {
        screen(&amx, &view)
            .contains("cleared 1 · kept 1 holding work no commit has")
            .then_some(())
    });
    assert_eq!(
        agents(&amx),
        ["tidy-b2c"],
        "the kept agent keeps its record, since the tree is where its work is"
    );
    assert!(!Path::new(&clean).exists(), "{clean} went with its record");
    assert!(
        Path::new(&holding).join("notes.md").exists(),
        "and the uncommitted file is where somebody left it"
    );
    let after = screen(&amx, &view);
    assert!(
        after.contains("tidy-b2c"),
        "the kept agent is still a row on the wall:\n{after}"
    );
}

/// An origin for `repo` to push to and be pruned against, bare and in a
/// directory of its own, with `main` already on it.
fn an_origin(amx: &Harness, repo: &Path) -> PathBuf {
    let bare = amx.home().join("origin.git");
    std::fs::create_dir_all(&bare).expect("the origin");
    git(&bare, &["init", "--bare", "-b", "main"]);
    git(repo, &["remote", "add", "origin", &bare.to_string_lossy()]);
    git(repo, &["push", "-q", "origin", "main"]);
    bare
}

/// What git in `repo` records about where this agent's branch stands on the
/// origin: nothing at all until somebody fetches, and `[gone]` afterwards.
fn upstream_track(repo: &Path, id: &str) -> String {
    git(
        repo,
        &[
            "for-each-ref",
            "--format=%(upstream:track)",
            &format!("refs/heads/amx/{id}"),
        ],
    )
    .trim()
    .to_string()
}

#[test]
fn c_reads_a_branch_the_forge_deleted_because_the_view_fetched_for_it() {
    let amx = Harness::new();
    let repo = amx.a_repo();
    let origin = an_origin(&amx, &repo);
    let tree = an_ended_agent(&amx, "tidy-b2c", &repo);
    work_on_the_branch(&tree, "search.rs");
    git(
        Path::new(&tree),
        &["push", "-q", "-u", "origin", "amx/tidy-b2c"],
    );

    // What a squash merge leaves behind: the forge took the commits under a sha
    // this branch does not hold, so nothing reads as merged, and then it
    // deleted the branch. Nobody has run a sweep here, so the view's own fetch
    // is the only thing that can make the delete a fact git will say out loud.
    git(&origin, &["branch", "-D", "amx/tidy-b2c"]);
    assert_eq!(
        upstream_track(&repo, "tidy-b2c"),
        "",
        "until something fetches, git has the origin holding the branch"
    );

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("the row", || {
        screen(&amx, &view).contains("tidy-b2c").then_some(())
    });
    amx.until("the view's own fetch to record the delete", || {
        (upstream_track(&repo, "tidy-b2c") == "[gone]").then_some(())
    });

    // And the press that waits on nothing has something current to read.
    press(&amx, &view, "c");
    let armed = amx.until("the row to say why its work has landed", || {
        let drawn = screen(&amx, &view);
        drawn
            .contains("c again clears · amx/tidy-b2c gone from origin")
            .then_some(drawn)
    });
    assert_eq!(
        agents(&amx),
        ["tidy-b2c"],
        "and one press clears nothing:\n{armed}"
    );
}

#[test]
fn acts_space_writes_the_look_on_the_record_and_leaves_the_rows_alone() {
    let amx = Harness::new();
    finished(&amx, "fix-login-a1b", "done", 60);
    finished(&amx, "port-import-b2c", "done", 120);

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("both rows", || {
        let drawn = screen(&amx, &view);
        (drawn.contains("fix-login-a1b") && drawn.contains("port-import-b2c")).then_some(())
    });

    // The cursor opens on the newest ending, which is the row the card opens
    // on. Nothing on the wall is painted for whether a row has been read, so
    // what the look is worth is on the record rather than on the screen.
    press(&amx, &view, "Space");
    amx.until("the look to reach the record", || {
        (amx.state("fix-login-a1b")["seen"].as_u64().unwrap_or(0) > 0).then_some(())
    });
    let carded = |drawn: &str| {
        drawn
            .lines()
            .any(|line| line.starts_with("fix-login-a1b · claude ┈"))
    };
    amx.until("the card", || carded(&screen(&amx, &view)).then_some(()));

    // With the card up the wall behind it is a wall behind a modal, and the
    // whole of it goes quiet the way it does under a task line — the cursor's
    // row with the rest, and nothing up there in weight. Read off the whole
    // screen, because the dim is set once at the top of it and left in force;
    // the first place either id stands is its row, above the card's rule.
    let whole = coloured(&amx, &view);
    for id in ["fix-login-a1b", "port-import-b2c"] {
        let on = sgr_at(&whole, id);
        assert!(
            on.contains(&2) && !on.contains(&1),
            "{id} is dim under the card and carries no weight:\n{}",
            screen(&amx, &view)
        );
    }

    // And with the card away, the rows read as they did before the press: the
    // cursor's row up out of the dim, the other as quiet as it always was.
    // Which row has been looked at is on the record and nowhere on the wall.
    press(&amx, &view, "Escape");
    amx.until("the card put away", || {
        (!carded(&screen(&amx, &view))).then_some(())
    });
    let whole = coloured(&amx, &view);
    let opened = sgr_at(&whole, "fix-login-a1b");
    assert!(
        !opened.contains(&1) && !opened.contains(&2),
        "the row the card was on is the row the cursor is on, and it reads as \
         it did before the press:\n{}",
        screen(&amx, &view)
    );
    let untouched = sgr_at(&whole, "port-import-b2c");
    assert!(
        untouched.contains(&2) && !untouched.contains(&1),
        "and the row nobody opened is as quiet as it always was:\n{}",
        screen(&amx, &view)
    );
}
