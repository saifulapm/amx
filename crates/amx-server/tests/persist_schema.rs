//! The on-disk schema, as a contract: version 1, an N/N−1 read window, and
//! the same unknown-field tolerance the wire keeps in both directions.
//!
//! Everything here is pure — no file is written. U02 owns the I/O and the
//! checked-in v1 golden; what this file pins is the shape those bytes have and
//! the window a reader accepts them through.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use std::path::PathBuf;

use amx_core::{Direction, Layout, PaneId, RowId, RowRange, ShortNumber, WorkspaceId};
use amx_server::persist::{
    HISTORY_DIR, PaneSnapshot, PersistError, READ_WINDOW, SIDECAR_MAGIC, SIDECAR_VERSION,
    SNAPSHOT_NAME, SidecarHeader, Snapshot, VERSION, WorkspaceSnapshot, sidecar_name,
};

fn pane_id() -> PaneId {
    "00000000-0000-0000-0000-0000000000a1".parse().unwrap()
}

fn other_pane_id() -> PaneId {
    "00000000-0000-0000-0000-0000000000a2".parse().unwrap()
}

fn workspace_id() -> WorkspaceId {
    "00000000-0000-0000-0000-0000000000b1".parse().unwrap()
}

fn populated() -> Snapshot {
    let mut layout = Layout::with_root(pane_id());
    layout
        .split(pane_id(), Direction::Right, other_pane_id(), 0.5)
        .expect("split the fixture layout");
    Snapshot {
        version: VERSION,
        focused_workspace: Some(workspace_id()),
        workspaces: vec![WorkspaceSnapshot {
            id: workspace_id(),
            short: ShortNumber::new(1),
            label: Some("build".to_owned()),
            layout,
            focus: Some(other_pane_id()),
        }],
        panes: vec![
            PaneSnapshot {
                id: pane_id(),
                short: ShortNumber::new(1),
                label: Some("editor".to_owned()),
                cwd: Some(PathBuf::from("/home/s/amx")),
            },
            PaneSnapshot {
                id: other_pane_id(),
                short: ShortNumber::new(2),
                label: None,
                cwd: None,
            },
        ],
    }
}

#[test]
fn snapshot_schema_round_trips_serde_and_ignores_unknown_fields() {
    let snapshot = populated();
    let json = serde_json::to_value(&snapshot).unwrap();

    // Flat tables keyed by UUID, like `session.state` — never positional, so
    // there is no pairing between two lists that can desync (04 §6).
    assert_eq!(json["version"], 1);
    assert_eq!(json["workspaces"][0]["id"], workspace_id().to_string());
    assert_eq!(json["panes"][0]["cwd"], "/home/s/amx");
    // Unset optionals are absent, not null: the bytes stay small and a later
    // reader distinguishes "no label" from "a null label" by never seeing one.
    assert!(json["panes"][1].get("label").is_none(), "{json}");
    assert!(json["panes"][1].get("cwd").is_none(), "{json}");
    // Layout is the wire's own tree, verbatim: one algebra, not a projection.
    assert!(
        json["workspaces"][0]["layout"]["root"].is_object(),
        "{json}"
    );

    assert_eq!(
        serde_json::from_value::<Snapshot>(json).unwrap(),
        snapshot,
        "the schema must survive its own encoding",
    );

    // A snapshot written by a later amx, inside the window, with fields this
    // build has never heard of at every level. Tolerated, exactly like the
    // wire tolerates them (04 §4), so v2 can add fields without stranding v1.
    let from_the_future = serde_json::json!({
        "version": VERSION,
        "focused_workspace": workspace_id(),
        "workspaces": [{
            "id": workspace_id(),
            "short": 1,
            "layout": { "root": { "leaf": pane_id() }, "zoomed": null },
            "agent_identity": { "kind": "claude", "session": "abc" },
        }],
        "panes": [{
            "id": pane_id(),
            "short": 1,
            "argv": ["nvim", "."],
        }],
        "written_by": "amx 9.0",
    });
    let decoded: Snapshot = serde_json::from_value(from_the_future)
        .expect("unknown fields must be ignored, not refused");
    assert_eq!(decoded.workspaces.len(), 1);
    assert_eq!(decoded.panes[0].id, pane_id());
    assert!(decoded.panes[0].cwd.is_none());
}

#[test]
fn the_read_window_accepts_this_version_and_refuses_a_newer_one() {
    assert_eq!(READ_WINDOW, &[VERSION], "at v1 there is no predecessor");

    for &version in READ_WINDOW {
        let snapshot = Snapshot {
            version,
            ..Snapshot::empty()
        };
        assert!(snapshot.check_version().is_ok(), "version {version}");
    }

    let newer = Snapshot {
        version: VERSION + 1,
        ..populated()
    };
    let err = newer
        .check_version()
        .expect_err("a snapshot from the future is refused, not guessed at");
    assert!(
        matches!(err, PersistError::NewerVersion { found } if found == VERSION + 1),
        "{err}",
    );
    // The message is what restore puts in the report, so it has to say which
    // version it saw rather than "could not read snapshot".
    assert!(err.to_string().contains("newer amx"), "{err}");
}

#[test]
fn an_empty_snapshot_is_the_no_state_case_not_an_error() {
    let empty = Snapshot::empty();
    assert_eq!(empty.version, VERSION);
    assert!(empty.is_empty());
    assert!(empty.check_version().is_ok());
    assert!(!populated().is_empty());
    assert_eq!(Snapshot::default(), empty);
}

#[test]
fn sidecars_are_keyed_by_pane_uuid_and_named_rows_not_ansi() {
    assert_eq!(SNAPSHOT_NAME, "session.json");
    assert_eq!(HISTORY_DIR, "history");
    assert_eq!(
        sidecar_name(pane_id()),
        "00000000-0000-0000-0000-0000000000a1.rows",
        "the file holds packed text rows, not an ANSI replay stream (R-M1-1)",
    );
    assert_ne!(sidecar_name(pane_id()), sidecar_name(other_pane_id()));
}

#[test]
fn a_sidecar_header_carries_the_pane_it_came_from() {
    let range = RowRange::new(RowId::from_raw(40), RowId::from_raw(295));
    let header = SidecarHeader::new(pane_id(), range);
    assert_eq!(header.version, SIDECAR_VERSION);
    assert_eq!(header.pane, pane_id());
    assert_eq!(header.range, range);

    // Magic + version + UUID + two row ids. Fixed, so U02's reader can refuse
    // a file shorter than a header before it interprets a single byte.
    assert_eq!(SidecarHeader::ENCODED_LEN, 4 + 2 + 16 + 8 + 8);
    assert_eq!(SIDECAR_MAGIC.len(), 4);

    // The pane is in the header as well as the file name, so a sidecar that
    // arrived under the wrong name is detectable rather than replayed into
    // somebody else's scrollback.
    assert_ne!(SidecarHeader::new(other_pane_id(), range), header);
}
