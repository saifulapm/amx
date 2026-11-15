//! End-to-end tests for the claudes running outside amx on the wall: the view
//! draws them in an `OUTSIDE AMX` group at its foot, and enter on one moves it
//! into an amx pane.

mod common;

use common::{Harness, bar, coloured, press};
use std::path::PathBuf;

/// The session the loose stand-in is on.
const SESSION: &str = "0c9e1d2f-4a5b-4c6d-8e7f-9a0b1c2d3e4f";

/// A loose stand-in on [`SESSION`] in `<home>/app`, with mock-claude as the
/// configured agent so a move starts the stand-in again.
fn a_loose_claude(amx: &Harness) -> (common::Loose, PathBuf) {
    amx.config(&format!("agent = \"{}\"\n", amx.mock()));
    let dir = amx.home().join("app");
    std::fs::create_dir_all(&dir).unwrap();
    let claude = amx.loose("sits-loose", &dir, &[("MOCK_CLAUDE_SESSION", SESSION)]);
    (claude, dir)
}

/// The view in a pane, kept off the account dir of whoever runs the suite, with
/// a moved claude set to continue its session.
fn a_view(amx: &Harness) -> String {
    let continues = amx.scenario("continues-a-session");
    amx.in_a_terminal(
        &[
            ("CLAUDE_CONFIG_DIR", ""),
            ("MOCK_CLAUDE_SCENARIO", &continues.to_string_lossy()),
        ],
        &[],
    )
}

/// The line of `drawn` after the one holding `text`.
fn under(drawn: &str, text: &str) -> String {
    let lines: Vec<&str> = drawn.lines().collect();
    let at = lines
        .iter()
        .position(|line| line.contains(text))
        .unwrap_or_else(|| panic!("no line holding {text} in:\n{drawn}"));
    lines.get(at + 1).unwrap_or(&"").to_string()
}

#[test]
fn the_view_draws_a_claude_outside_amx_under_its_own_heading_at_the_foot() {
    let amx = Harness::new();
    let (claude, _) = a_loose_claude(&amx);

    let view = a_view(&amx);
    let drawn = amx.until("the outside heading", || {
        let drawn = amx.capture(&view);
        drawn.contains("OUTSIDE AMX").then_some(drawn)
    });

    let heading = drawn
        .lines()
        .find(|line| line.contains("OUTSIDE AMX"))
        .unwrap_or_default();
    assert_eq!(heading.trim(), "OUTSIDE AMX 1", "{drawn}");
    let row = under(&drawn, "OUTSIDE AMX");
    assert!(row.contains("~/app"), "its dir: {row:?}\n{drawn}");
    assert!(row.contains("idle"), "its status: {row:?}\n{drawn}");
    assert!(
        row.contains(&claude.pid().to_string()),
        "its pid: {row:?}\n{drawn}"
    );
}

#[test]
fn the_view_leaves_out_a_held_claude_and_draws_no_heading_once_none_is_loose() {
    let amx = Harness::new();
    let held = amx.loose(
        "sits-loose",
        amx.home(),
        &[("MOCK_CLAUDE_SESSION", "held-by-a-record")],
    );
    amx.record("an-agent", "%404");
    amx.set_meta(
        "an-agent",
        serde_json::json!({ "session": "held-by-a-record" }),
    );
    let (claude, _) = a_loose_claude(&amx);

    let view = a_view(&amx);
    let drawn = amx.until("the outside heading", || {
        let drawn = amx.capture(&view);
        (drawn.contains("OUTSIDE AMX") && drawn.contains("an-agent")).then_some(drawn)
    });
    assert!(drawn.contains("OUTSIDE AMX 1"), "one, not two:\n{drawn}");
    assert!(
        !drawn.contains(&held.pid().to_string()),
        "a record holds its session:\n{drawn}"
    );

    drop(claude);
    amx.until("the heading to go with the last loose claude", || {
        let drawn = amx.capture(&view);
        (drawn.contains("an-agent") && !drawn.contains("OUTSIDE AMX")).then_some(())
    });
}

#[test]
fn enter_on_a_claude_outside_amx_moves_it_onto_the_wall() {
    let amx = Harness::new();
    let (mut claude, dir) = a_loose_claude(&amx);
    let pid = claude.pid().to_string();

    let view = a_view(&amx);
    amx.until("the claude's row", || {
        amx.capture(&view).contains(&pid).then_some(())
    });
    press(&amx, &view, "Down");
    amx.until("the cursor on the claude's row", || {
        coloured(&amx, &view)
            .lines()
            .any(|line| line.contains(&pid) && line.contains(&bar()))
            .then_some(())
    });
    press(&amx, &view, "Enter");

    amx.until("the old claude to end", || claude.ended().then_some(()));
    let id = amx.until("a record in the claude's dir", || {
        common::agents(&amx)
            .into_iter()
            .find(|id| amx.meta(id)["dir"] == dir.display().to_string())
    });
    amx.until("its row among the agents, and no outside heading", || {
        let drawn = amx.capture(&view);
        (drawn.contains(&id) && !drawn.contains("OUTSIDE AMX")).then_some(())
    });
}
