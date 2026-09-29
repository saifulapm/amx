//! The keys: the screen that lists them, the row that says which ones the
//! cursor is over, and what the view answers to.
//!
//! Driven in a real tmux pane like the rest of the view, because what a chord
//! does to the view, and what the view leaves behind when the last of them
//! closes it, are questions a pty answers and nothing else does.

mod common;

use common::{Harness, pane_field, press, resize, types, until_empty};

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
        amx.capture(view)
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
            amx.capture(&view)
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
    // And the key that puts a row over the wall says what it would do to this
    // one: the row is in a group amx put it in, so the press pins it.
    assert!(row.contains("ctrl+t pin"), "{row}");

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
        amx.capture(&view).contains("walk the agents").then_some(())
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
    // A screen tall enough for the whole document at once: every key, a
    // heading over each of the five groups, the row that stands each group off
    // from the one above it, the row over them that says where in the document
    // this is and the air under it, and the view's own chrome. It grows with
    // the table, so a key added to it is a row added here.
    resize(&amx, &view, 100, 72);

    types(&amx, &view, "?");
    // Waited for by the last key of the last group, so a screen caught halfway
    // through being written is not read as a key that is missing.
    let keys = amx.until("the keys", || {
        let drawn = amx.capture(&view);
        drawn
            .contains("which vendor runs it, for one spawn")
            .then_some(drawn)
    });
    for does in [
        "start an agent",
        "start a copy of it on a task",
        "the agent you were last in",
        "an answer or a message on it",
        "which vendor, model and effort each one runs",
        "write the line in $EDITOR",
        "what it has changed",
        "open its pull request in the browser",
        "stop it",
        "clear the finished",
        "call it something else",
        "pin it over the wall",
        "ctrl+x",
        "a ref to cut from · a request to start on",
        "a branch that already exists, to start on",
        "take the uncommitted work",
        "how hard the next agent thinks",
        "effort, for one spawn",
        "lines sent before · ↑ ↓ too on a task line",
    ] {
        assert!(keys.contains(does), "{does} is not among the keys:\n{keys}");
    }
    assert!(
        !keys.contains('…'),
        "a screen this tall cuts nothing short:\n{keys}"
    );

    // One column: every heading is on a row of its own, headed the way the
    // wall heads a group of agents, with the count of what is under it at the
    // column's own right edge.
    let heading = |label: &str| {
        keys.lines()
            .find(|line| line.trim_start().starts_with(label))
            .unwrap_or_else(|| panic!("no {label} heading:\n{keys}"))
            .to_string()
    };
    let walk = heading("WALK");
    assert!(walk.contains('┈'), "each is ruled: {walk:?}");
    assert!(
        !walk.contains("ARRANGE"),
        "and stands on a row of its own:\n{keys}"
    );

    // The count at the end of a heading is the keys under it, `v` among them:
    // a key on the screen the heading over it does not count would be a number
    // somebody has to check by hand.
    let look = heading("LOOK");
    assert!(
        look.trim_end().ends_with("12"),
        "the LOOK count includes v: {look:?}"
    );

    // And the row over them says how much of the document is on the screen,
    // which on a screen holding all of it is how many keys there are.
    assert!(
        keys.lines().any(|line| line.trim_end().ends_with("keys")),
        "the screen says how many keys there are:\n{keys}"
    );

    // And back to the agents, which is what the view is for.
    press(&amx, &view, "Escape");
    until_empty(&amx, &view);
}

#[test]
fn the_keys_a_short_screen_cannot_hold_are_a_scroll_away() {
    let amx = Harness::new();
    let view = amx.in_a_terminal(&[], &[]);
    until_empty(&amx, &view);
    // The screen a terminal opens at, which is a third of the rows the keys
    // take.
    resize(&amx, &view, 80, 24);

    types(&amx, &view, "?");
    // Waited for by the marker as well as the keys, so a screen caught before
    // the marker catches up is not read as one that says nothing.
    // The whole frame: the keys, the row that says how many there are and the
    // row at the foot that says how to reach the rest. A screen caught halfway
    // through being written still carries the list's own foot under the keys.
    let first = amx.until("the keys, the count and the foot", || {
        let drawn = amx.capture(&view);
        (drawn.contains("walk the agents")
            && drawn.contains(" of 55")
            && drawn.contains("j k scroll"))
        .then_some(drawn)
    });
    assert!(
        !first.contains("which vendor runs it, for one spawn"),
        "the last of the keys is not on the first screenful:\n{first}"
    );

    // G is the foot of the document, one press, and the overlay still has the
    // screen when it lands: scrolling is not the press that puts the agents
    // back.
    press(&amx, &view, "G");
    let last = amx.until("the foot of the keys", || {
        let drawn = amx.capture(&view);
        drawn
            .contains("which vendor runs it, for one spawn")
            .then_some(drawn)
    });
    assert!(
        last.contains(&format!(" of {}", 55)),
        "which says where in the document it is:\n{last}"
    );
    assert!(
        last.contains("any key goes back"),
        "and the overlay still has the screen:\n{last}"
    );

    // And `/` narrows them as it is typed, which is the other way to reach a
    // key below the fold.
    press(&amx, &view, "/");
    types(&amx, &view, "worktree");
    let found = amx.until("the keys that answer to it", || {
        let drawn = amx.capture(&view);
        drawn.contains("find worktree").then_some(drawn)
    });
    assert!(
        found.contains("whether it gets a worktree of its own"),
        "{found}"
    );
    assert!(
        !found.contains("walk the agents"),
        "and nothing that does not answer to it:\n{found}"
    );

    // Esc drops the search rather than the screen, and the one after it is the
    // way out.
    press(&amx, &view, "Escape");
    amx.until("every key back", || {
        amx.capture(&view).contains("walk the agents").then_some(())
    });
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

    // Waited for by the status the assertion reads, not merely the death:
    // tmux marks a pane dead a beat before it records what it died with, and
    // a look landing between the two reads an empty status.
    let status = amx.until("the view to close with its status recorded", || {
        let status = amx.tmux(&["display-message", "-p", "-t", &view, "#{pane_dead_status}"]);
        (!status.is_empty()).then_some(status)
    });
    assert_eq!(status, "0", "closing a view is not a failure");
    assert!(
        !amx.capture(&view).contains("nobody asking"),
        "the screen the view borrowed is handed back"
    );
}
