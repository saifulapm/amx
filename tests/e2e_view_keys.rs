//! The keys somebody binds themselves: the command one runs, where it runs,
//! the group the keys screen gives them, and what a spelling nobody can press
//! is answered with.
//!
//! Driven in a real tmux pane, because the whole of a bound key is the view
//! handing the terminal to somebody else's command and taking it back again,
//! and a terminal is the only thing that can be asked whether that happened.

mod common;

use common::Harness;
use serde_json::json;

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

fn press(amx: &Harness, view: &str, key: &str) {
    amx.tmux(&["send-keys", "-t", view, key]);
}

/// Give the pane a terminal wide enough for both columns of the keys screen
/// and deep enough to hold them without a page turn, so what is on it is all
/// of it.
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

/// The keys screen, waited for by the last key of the second column: the group
/// somebody bound is drawn under that one, so a screen caught before it is not
/// read as a group that is missing.
fn keys_screen(amx: &Harness, view: &str) -> String {
    press(amx, view, "?");
    amx.until("the keys", || {
        let drawn = screen(amx, view);
        drawn
            .contains("which vendor runs it, for one spawn")
            .then_some(drawn)
    })
}

#[test]
fn a_bound_key_runs_its_command_where_the_agent_works_and_the_view_takes_the_screen_back() {
    let amx = Harness::new();
    let repo = amx.a_repo();

    // The command writes where it was started rather than the path it was
    // told: a file it makes in the tree is the one answer no string comparison
    // can be talked out of.
    amx.config("[keys]\n\"x\" = \"{ echo $AMX_ID; echo $AMX_WORKTREE; } > ran-here\"\n");

    amx.play("fix-login-a1b", "asks-a-question");
    amx.until_state("fix-login-a1b", "waiting");
    // An agent with a tree of its own, which is not the directory it was
    // started in: the command runs in the tree.
    amx.set_meta("fix-login-a1b", json!({ "worktree": repo }));

    let view = amx.in_a_terminal(&[], &[]);
    amx.until("the agent's row", || {
        screen(&amx, &view).contains("fix-login-a1b").then_some(())
    });

    press(&amx, &view, "x");

    // Waited for by both lines, so a file caught halfway through being written
    // is not read as a variable the command was never given.
    let wrote = repo.join("ran-here");
    let said = amx.until("the command to have run in the tree", || {
        std::fs::read_to_string(&wrote)
            .ok()
            .filter(|said| said.lines().count() == 2)
    });
    let tree = repo.to_string_lossy().into_owned();
    assert_eq!(
        said.lines().collect::<Vec<_>>(),
        ["fix-login-a1b", tree.as_str()],
        "the agent it was pressed on, and the tree it was run in"
    );

    // And the view has the terminal again. There is nothing on the screen a
    // command that drew nothing changed, so the view is asked a question only
    // one still holding the screen could answer.
    press(&amx, &view, "?");
    amx.until("the keys", || {
        screen(&amx, &view)
            .contains("walk the agents")
            .then_some(())
    });
    assert!(
        amx.pane_alive(&view),
        "and the pane it was drawing in is still its own"
    );
}

#[test]
fn the_keys_screen_gives_what_somebody_bound_a_group_of_their_own() {
    let amx = Harness::new();
    amx.config("[keys]\n\"alt+g\" = \"lazygit\"\n");

    let view = amx.in_a_terminal(&[], &[]);
    until_empty(&amx, &view);
    resize(&amx, &view, 120, 40);

    let keys = keys_screen(&amx, &view);
    assert!(
        keys.contains("YOURS"),
        "a heading over them, the way the five are headed:\n{keys}"
    );
    let row = keys
        .lines()
        .find(|line| line.contains("alt+g"))
        .unwrap_or_default();
    assert!(
        row.contains("lazygit"),
        "the key stands against the command it runs, which is its whole name: {row:?}"
    );
}

#[test]
fn a_spelling_the_view_cannot_read_is_said_once_and_binds_nothing() {
    let amx = Harness::new();
    amx.config("[keys]\n\"shift+z\" = \"never runs\"\n");

    let view = amx.in_a_terminal(&[], &[]);
    let said = amx.until("the view to say which spelling it could not read", || {
        let drawn = screen(&amx, &view);
        drawn.contains("shift+z").then_some(drawn)
    });
    assert!(
        said.contains("is no key the view can read"),
        "in words that say what is wrong with it:\n{said}"
    );

    resize(&amx, &view, 120, 40);
    let keys = keys_screen(&amx, &view);
    assert!(
        !keys.contains("YOURS"),
        "and nothing was bound, so there is no group of theirs:\n{keys}"
    );
    assert!(
        !keys.contains("never runs"),
        "least of all the command nobody can reach:\n{keys}"
    );
}

#[test]
fn a_spelling_amx_already_binds_is_refused_by_name_and_binds_nothing() {
    let amx = Harness::new();
    amx.config("[keys]\n\"ctrl+x\" = \"never runs\"\n");

    let view = amx.in_a_terminal(&[], &[]);
    let said = amx.until("the view to say the key is its own", || {
        let drawn = screen(&amx, &view);
        drawn.contains("ctrl+x").then_some(drawn)
    });
    assert!(
        said.contains("amx's own"),
        "in words that say whose key it is:\n{said}"
    );
    assert!(
        said.contains("stop it"),
        "and what amx does on it, so the person knows what they were taking:\n{said}"
    );

    resize(&amx, &view, 120, 40);
    let keys = keys_screen(&amx, &view);
    assert!(
        !keys.contains("YOURS"),
        "and nothing was bound, so there is no group of theirs:\n{keys}"
    );
    assert!(
        !keys.contains("never runs"),
        "least of all the command the key amx binds would never reach:\n{keys}"
    );
}
