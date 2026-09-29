//! The keys somebody binds themselves: the command one runs, where it runs,
//! the group the keys screen gives them, and what a spelling nobody can press
//! is answered with.
//!
//! Driven in a real tmux pane, because the whole of a bound key is the view
//! handing the terminal to somebody else's command and taking it back again,
//! and a terminal is the only thing that can be asked whether that happened.

mod common;

use common::{Harness, press, resize, until_empty};
use serde_json::json;

/// The foot of the keys screen, which is where a group somebody bound is
/// drawn: after the last of amx's own, because the keys the view binds are the
/// ones on every machine.
///
/// Waited for by the last key amx itself binds, so a screen caught before the
/// foot of the document is not read as a group that is missing.
fn keys_screen(amx: &Harness, view: &str) -> String {
    press(amx, view, "?");
    press(amx, view, "G");
    amx.until("the foot of the keys", || {
        let drawn = amx.capture(view);
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
        amx.capture(&view).contains("fix-login-a1b").then_some(())
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
        amx.capture(&view).contains("walk the agents").then_some(())
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
        let drawn = amx.capture(&view);
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
        let drawn = amx.capture(&view);
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

#[test]
fn a_killed_view_gives_the_terminal_back() {
    for signal in ["TERM", "HUP"] {
        let amx = Harness::new();
        let view = amx.in_a_terminal(&[], &[]);
        until_empty(&amx, &view);

        // Keep the pane after the view ends, so the modes it left behind can
        // be read off it.
        amx.tmux(&["set-option", "-w", "-t", &view, "remain-on-exit", "on"]);
        let pid = amx.tmux(&["display-message", "-p", "-t", &view, "#{pane_pid}"]);
        let killed = std::process::Command::new("kill")
            .args([&format!("-{signal}"), &pid])
            .status()
            .expect("kill");
        assert!(killed.success(), "SIG{signal} reached the view");

        // Read in the same look as the end is recorded in, whether it ended
        // by its own hand or the signal's, so the modes are what the view left
        // and not what it was still drawing.
        let left = amx.until("the view to end", || {
            let read = amx.tmux(&[
                "display-message",
                "-p",
                "-t",
                &view,
                "#{pane_dead_status}#{pane_dead_signal}|#{alternate_on} #{mouse_any_flag} \
                 #{mouse_button_flag} #{mouse_standard_flag} #{bracket_paste_flag}|#{pane_title}|",
            ]);
            let (status, modes) = read.split_once('|')?;
            (!status.is_empty()).then(|| modes.to_string())
        });
        assert_eq!(
            left, "0 0 0 0 0||",
            "after SIG{signal}: off the alternate screen, no mouse, no bracketed \
             paste, and no title of the view's left on the window"
        );
    }

    // And a view whose terminal goes with its pane ends too.
    let amx = Harness::new();
    let view = amx.in_a_terminal(&[], &[]);
    until_empty(&amx, &view);
    let pid = amx.tmux(&["display-message", "-p", "-t", &view, "#{pane_pid}"]);

    // The pane going takes the terminal with it: every read after that is an
    // end of file or an error, and the view must stop reading, not spin.
    amx.tmux(&["kill-pane", "-t", &view]);
    let there = || std::path::Path::new(&format!("/proc/{pid}")).exists();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while there() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let lingered = there();
    if lingered {
        // Not left behind to spin after the test has failed.
        let _ = std::process::Command::new("kill")
            .args(["-KILL", &pid])
            .status();
    }
    assert!(!lingered, "the view outlived its terminal");
}
