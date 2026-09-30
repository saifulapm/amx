//! End-to-end tests for the view's keys: the `?` help screen, the hint row,
//! chords the view does not bind, and what quitting with q leaves behind.

mod common;

use common::{Harness, pane_field, press, resize, types, until_empty};

#[test]
fn the_first_quit_offers_the_status_line_and_no_quit_after_it_does() {
    let amx = Harness::new();

    // Keep the pane after the view exits so its last screen can be captured.
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
    // Find the hint row by `? keys`, the last hint it drops for width.
    let hints = |want: &str| {
        amx.until(want, || {
            amx.capture(&view)
                .lines()
                .rfind(|line| line.contains("? keys"))
                .filter(|row| row.contains(want))
                .map(str::to_string)
        })
    };

    // The cursor starts on the agent's row.
    let row = hints("space card");
    assert!(row.contains("enter attach"), "{row}");
    assert!(row.contains("ctrl+x stop"), "{row}");
    // The row is in the group amx chose for it, so ctrl+t offers to pin it.
    assert!(row.contains("ctrl+t pin"), "{row}");

    // On the group heading, enter and ctrl+x act on the group.
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

    // alt+q is a common window manager chord. The view must not read it as q,
    // which quits.
    press(&amx, &view, "M-q");

    // An ignored key draws nothing, so open the help screen to show the view
    // is still running.
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
    // Tall enough for the whole help document: every key, five group headings
    // with a blank row above each, the position row and the blank under it,
    // and the view's chrome. A key added to the table needs a row added here.
    resize(&amx, &view, 100, 72);

    types(&amx, &view, "?");
    // Wait for the last key of the last group. The view has no synchronized
    // output, so a capture can land mid-frame.
    let keys = amx.until("the keys", || {
        let drawn = amx.capture(&view);
        drawn
            .contains("which vendor runs it, for one spawn")
            .then_some(drawn)
    });
    for does in [
        "start an agent",
        "fork the agent onto a task",
        "the agent you were last in",
        "to answer or send a message",
        "which vendor, model and effort each one runs",
        "edit the line in $EDITOR",
        "what it has changed",
        "open its pull request in the browser",
        "stop it",
        "mark the finished",
        "rename it",
        "pin it to the top",
        "ctrl+x",
        "a base ref · a pull request to start on",
        "an existing branch to start on",
        "bring along the uncommitted changes",
        "how hard the next agent thinks",
        "effort, for one spawn",
        "earlier lines · ↑ ↓ too on a task line",
    ] {
        assert!(keys.contains(does), "{does} is not among the keys:\n{keys}");
    }
    assert!(
        !keys.contains('…'),
        "a screen this tall cuts nothing short:\n{keys}"
    );

    // One column: each heading is on its own row, ruled like a wall group
    // heading, with its key count at the right edge.
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

    // A heading's count includes every key listed under it, `v` among them.
    let look = heading("LOOK");
    assert!(
        look.trim_end().ends_with("12"),
        "the LOOK count includes v: {look:?}"
    );

    // With the whole document on screen, the position row shows the total.
    assert!(
        keys.lines().any(|line| line.trim_end().ends_with("keys")),
        "the screen says how many keys there are:\n{keys}"
    );

    press(&amx, &view, "Escape");
    until_empty(&amx, &view);
}

#[test]
fn the_keys_a_short_screen_cannot_hold_are_a_scroll_away() {
    let amx = Harness::new();
    let view = amx.in_a_terminal(&[], &[]);
    until_empty(&amx, &view);
    // The default terminal size, about a third of the rows the help needs.
    resize(&amx, &view, 80, 24);

    types(&amx, &view, "?");
    // Wait for the keys, the position row and the scroll hint together. The
    // view has no synchronized output, so a capture can land mid-frame with
    // the list's own hint row still at the foot.
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

    // G jumps to the end. Scroll keys do not close the help screen.
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

    // The first Esc clears the search; the second closes the help screen.
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

    // Keep the pane after the view exits so its exit status can be read.
    amx.tmux(&["set-option", "-w", "-t", &view, "remain-on-exit", "on"]);
    amx.tmux(&["send-keys", "-t", &view, "q"]);

    // Wait on the status itself: tmux marks a pane dead before it records its
    // exit status.
    let status = amx.until("the view to close with its status recorded", || {
        let status = amx.tmux(&["display-message", "-p", "-t", &view, "#{pane_dead_status}"]);
        (!status.is_empty()).then_some(status)
    });
    assert_eq!(status, "0", "closing a view is not a failure");
    assert!(
        !amx.capture(&view).contains("no agents yet"),
        "the screen the view borrowed is handed back"
    );
}
