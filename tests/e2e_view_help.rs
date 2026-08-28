//! The keys: the screen that lists them, the row that says which ones the
//! cursor is over, and what the view answers to.
//!
//! Driven in a real tmux pane like the rest of the view, because what a chord
//! does to the view, and what the view leaves behind when the last of them
//! closes it, are questions a pty answers and nothing else does.

mod common;

use common::Harness;

/// What is on the view's screen now.
fn screen(amx: &Harness, pane: &str) -> String {
    amx.capture(pane)
}

/// Wait for a view with nothing in it, which is the one line amx has for a
/// wall nobody has put anything on.
fn until_empty(amx: &Harness, view: &str) {
    amx.until("the empty view", || {
        screen(amx, view).contains("nobody asking").then_some(())
    });
}

/// Type a line at the view, as a person types one.
fn types(amx: &Harness, view: &str, text: &str) {
    amx.tmux(&["send-keys", "-t", view, "-l", text]);
}

fn press(amx: &Harness, view: &str, key: &str) {
    amx.tmux(&["send-keys", "-t", view, key]);
}

fn pane_field(amx: &Harness, pane: &str, format: &str) -> String {
    amx.tmux(&["display-message", "-p", "-t", pane, format])
}

/// Give the pane a terminal of this shape. A window with nobody attached to it
/// takes tmux's own default of eighty by twenty-four, and the overlay is drawn
/// for a screen wider than that.
fn resize(amx: &Harness, view: &str, width: u16, height: u16) {
    amx.tmux(&["set-option", "-w", "-t", view, "window-size", "manual"]);
    amx.tmux(&[
        "resize-window",
        "-t",
        view,
        "-x",
        &width.to_string(),
        "-y",
        &height.to_string(),
    ]);
}

#[test]
fn acts_the_first_quit_offers_the_status_line_and_no_quit_after_it_does() {
    let amx = Harness::new();

    // Keep each pane after its command ends, so what it ended with can be
    // read.
    let closed = |view: &str| {
        amx.tmux(&["set-option", "-w", "-t", view, "remain-on-exit", "on"]);
        press(&amx, view, "q");
        amx.until("the view to close", || {
            let dead = amx.tmux(&["display-message", "-p", "-t", view, "#{pane_dead}"]);
            (dead == "1").then_some(())
        });
        screen(&amx, view)
    };

    let view = amx.in_a_terminal(&[], &[]);
    until_empty(&amx, &view);
    let offered = closed(&view);
    assert!(
        offered.contains("set -g status-right '#(amx statusline)"),
        "the line is pasted, so it is the whole line tmux takes:\n{offered}"
    );

    let again = amx.in_a_terminal(&[], &[]);
    until_empty(&amx, &again);
    let quiet = closed(&again);
    assert!(
        !quiet.contains("amx statusline"),
        "an offer that comes back every time is an advertisement:\n{quiet}"
    );
}

#[test]
fn keymap_the_hint_row_says_what_the_line_under_the_cursor_answers_to() {
    let amx = Harness::new();
    amx.play("ask-a1b", "asks-a-question");
    amx.until_state("ask-a1b", "waiting");

    let view = amx.in_a_terminal(&[], &[]);
    // The row is the one holding the key that leads to all of them, which is
    // the last thing the row sheds and so the way to find it.
    let hints = |want: &str| {
        amx.until(want, || {
            screen(&amx, &view)
                .lines()
                .rfind(|line| line.contains("? keys"))
                .filter(|row| row.contains(want))
                .map(str::to_string)
        })
    };

    // The view opens on the agent's row, where those keys reach the agent.
    let row = hints("space card");
    assert!(row.contains("enter attach"), "{row}");
    assert!(row.contains("ctrl+x stop"), "{row}");

    // One line up is the heading over it, where the same two keys are about
    // the group rather than about any one agent.
    press(&amx, &view, "Up");
    let heading = hints("enter shuts it");
    assert!(heading.contains("ctrl+x clears the group"), "{heading}");
    assert!(
        !heading.contains("attach"),
        "a heading has no window to bring forward:\n{heading}"
    );
}

#[test]
fn keymap_a_chord_the_view_never_bound_leaves_it_holding_the_screen() {
    let amx = Harness::new();
    let view = amx.in_a_terminal(&[], &[]);
    until_empty(&amx, &view);

    // alt+q is somebody arranging their windows, and q on its own closes the
    // view: a list whose keys answered to every chord that carried them would
    // shut on the first of those.
    press(&amx, &view, "M-q");

    // There is nothing to wait for in a key that does nothing, so wait for
    // something only a view still holding the screen could draw.
    press(&amx, &view, "?");
    amx.until("the keys", || {
        screen(&amx, &view)
            .contains("walk the agents")
            .then_some(())
    });
    assert_eq!(
        pane_field(&amx, &view, "#{pane_dead}"),
        "0",
        "and the pane the view was running in is still its own"
    );
}

#[test]
fn the_keys_are_on_the_screen_for_the_asking() {
    let amx = Harness::new();
    let view = amx.in_a_terminal(&[], &[]);
    until_empty(&amx, &view);
    // The screen the overlay is drawn for: wide enough for two columns and
    // deep enough for the longer of them.
    resize(&amx, &view, 120, 40);

    types(&amx, &view, "?");
    // Waited for by the last row of the deeper column, so a screen caught
    // halfway through being written is not read as a key that is missing.
    let keys = amx.until("the keys", || {
        let drawn = screen(&amx, &view);
        drawn.contains("write the line in $EDITOR").then_some(drawn)
    });
    for does in [
        "start an agent",
        "reply",
        "what it has changed",
        "stop it",
        "call it something else",
        "ctrl+x",
    ] {
        assert!(keys.contains(does), "{does} is not among the keys:\n{keys}");
    }

    // Two columns, each headed the way the wall heads a group of agents: the
    // second stands beside the first rather than under it, and a screen this
    // wide says what every key does in full.
    let heading = keys
        .lines()
        .find(|line| line.contains("WALK"))
        .unwrap_or_default();
    assert!(
        heading.contains("ARRANGE"),
        "the columns stand side by side:\n{keys}"
    );
    assert!(heading.contains('─'), "and each is ruled: {heading:?}");
    assert!(
        !keys.contains('…'),
        "a screen this wide cuts nothing short:\n{keys}"
    );

    // And back to the agents, which is what the view is for.
    press(&amx, &view, "Escape");
    until_empty(&amx, &view);
}

#[test]
fn the_keys_a_short_screen_cannot_hold_are_a_page_away() {
    let amx = Harness::new();
    let view = amx.in_a_terminal(&[], &[]);
    until_empty(&amx, &view);
    // The screen a terminal opens at, which is half the rows the keys take.
    resize(&amx, &view, 80, 24);

    types(&amx, &view, "?");
    let first = amx.until("the keys", || {
        let drawn = screen(&amx, &view);
        drawn.contains("walk the agents").then_some(drawn)
    });
    assert!(
        first.contains("page 1 of"),
        "a screen this short says there is another page:\n{first}"
    );
    assert!(
        !first.contains("which vendor runs it, for one spawn"),
        "and the last of the keys is not on this one:\n{first}"
    );

    // The page turns under a real terminal's key, and the overlay is still up
    // when it has: paging is not the press that puts the agents back.
    press(&amx, &view, "PageDown");
    let second = amx.until("the rest of the keys", || {
        let drawn = screen(&amx, &view);
        drawn
            .contains("which vendor runs it, for one spawn")
            .then_some(drawn)
    });
    assert!(
        second.contains("page 2 of"),
        "which page it is on:\n{second}"
    );
    assert!(
        second.contains("any key goes back"),
        "and the overlay still has the screen:\n{second}"
    );

    press(&amx, &view, "Escape");
    until_empty(&amx, &view);
}

#[test]
fn q_closes_the_view_and_gives_the_screen_back() {
    let amx = Harness::new();
    let view = amx.in_a_terminal(&[], &[]);
    until_empty(&amx, &view);

    // Keep the pane after its command ends, so what it ended with can be read.
    amx.tmux(&["set-option", "-w", "-t", &view, "remain-on-exit", "on"]);
    amx.tmux(&["send-keys", "-t", &view, "q"]);

    amx.until("the view to close", || {
        let dead = amx.tmux(&["display-message", "-p", "-t", &view, "#{pane_dead}"]);
        (dead == "1").then_some(())
    });
    assert_eq!(
        amx.tmux(&["display-message", "-p", "-t", &view, "#{pane_dead_status}"]),
        "0",
        "closing a view is not a failure"
    );
    assert!(
        !screen(&amx, &view).contains("nobody asking"),
        "the screen the view borrowed is handed back"
    );
}
