//! T15 acceptance: the picker is one fuzzy-list primitive.
//!
//! 04 §7: "one fuzzy-list primitive for every choose-one interaction … no
//! other dialog type exists". The proof here is structural: one helper drives
//! `Picker` by feeding it keys, and three genuinely different sources —
//! workspace labels, panes out of a real layout tree, and the control method
//! table — all pass through that same helper. Nothing about any source
//! reaches the picker but a list of labels.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use amx_client::picker::{Picker, PickerEvent};
use amx_core::{Direction, Layout, PaneId};

/// The one code path: build the picker, type the query, press Enter, map the
/// chosen index back to the caller's label. Every source below goes through
/// here and nowhere else.
fn choose(labels: Vec<String>, query: &str) -> Option<String> {
    let mut picker = Picker::new(labels);
    for b in query.bytes() {
        match picker.key(b) {
            PickerEvent::Continue => {}
            other => panic!("typing must keep the picker open: {other:?}"),
        }
    }
    match picker.key(b'\r') {
        PickerEvent::Chosen(at) => Some(picker.items()[at].clone()),
        _ => None,
    }
}

#[test]
fn picker_filters_workspaces_panes_and_commands_through_one_code_path() {
    // Source one: workspace labels.
    let workspaces = vec!["build".to_owned(), "logs".to_owned(), "scratch".to_owned()];
    assert_eq!(choose(workspaces, "lo"), Some("logs".to_owned()));

    // Source two: panes, straight out of a real layout tree.
    let (a, b, c) = (PaneId::new_v4(), PaneId::new_v4(), PaneId::new_v4());
    let mut layout = Layout::with_root(a);
    layout.split(a, Direction::Right, b, 0.5).expect("split");
    layout.split(b, Direction::Down, c, 0.5).expect("split");
    let panes: Vec<String> = layout
        .panes()
        .iter()
        .enumerate()
        .map(|(at, id)| format!("pane {}: {id}", at + 1))
        .collect();
    assert_eq!(panes.len(), 3);
    let chosen = choose(panes.clone(), "pane 2").expect("a pane matches");
    assert_eq!(chosen, panes[1]);

    // Source three: commands, from the derived control method table.
    let commands: Vec<String> = amx_proto::control::SPECS
        .iter()
        .map(|spec| spec.wire.to_owned())
        .collect();
    assert_eq!(
        choose(commands.clone(), "create"),
        Some("workspace.create".to_owned())
    );
    // Fuzzy, not prefix: a subsequence with gaps still finds its method.
    assert_eq!(choose(commands, "pnsw"), Some("pane.swap".to_owned()));
}

#[test]
fn picker_selection_moves_edits_and_cancels() {
    let items = vec!["alpha".to_owned(), "beta".to_owned(), "alpaca".to_owned()];
    let mut picker = Picker::new(items);

    // The empty query matches everything in source order.
    assert_eq!(picker.matches(), &[0, 1, 2]);

    // Typing narrows; the tighter match ranks first.
    for b in b"alp" {
        picker.key(*b);
    }
    assert_eq!(picker.matches(), &[0, 2], "beta has no `alp` subsequence");
    assert_eq!(picker.selected(), Some(0));

    // ctrl+n moves down, ctrl+p back up, both clamped.
    picker.key(0x0e);
    assert_eq!(picker.selected(), Some(2));
    picker.key(0x0e);
    assert_eq!(
        picker.selected(),
        Some(2),
        "the selection clamps at the end"
    );
    picker.key(0x10);
    assert_eq!(picker.selected(), Some(0));

    // Backspace widens again.
    picker.key(0x7f);
    assert_eq!(picker.query(), "al");
    assert_eq!(picker.matches(), &[0, 2]);

    // A query nothing matches leaves Enter with nothing to choose.
    for b in b"zzz" {
        picker.key(*b);
    }
    assert_eq!(picker.selected(), None);
    assert_eq!(picker.key(b'\r'), PickerEvent::Continue);

    // Esc ends the interaction with nothing chosen.
    assert_eq!(picker.key(0x1b), PickerEvent::Cancelled);
}
