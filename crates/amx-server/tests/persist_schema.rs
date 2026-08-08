//! The on-disk schema, as a contract: version 1, an N/N−1 read window, and
//! the same unknown-field tolerance the wire keeps in both directions.
//!
//! Everything here is pure — no file is written. U02 owns the I/O and the
//! checked-in v1 golden; what this file pins is the shape those bytes have and
//! the window a reader accepts them through.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use std::path::PathBuf;

use amx_core::agent::{AgentKind, RefKind, RefSource, SessionRef, StartSource};
use amx_core::{Direction, Layout, PaneId, RowId, RowRange, ShortNumber, WorkspaceId, Worktree};
use amx_server::persist::{
    AgentSnapshot, HISTORY_DIR, PaneSnapshot, PersistError, READ_WINDOW, SIDECAR_MAGIC,
    SIDECAR_VERSION, SNAPSHOT_NAME, SidecarHeader, Snapshot, VERSION, WorkspaceSnapshot,
    sidecar_name,
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
            // M3's additive block (D-M3-10), on the same R-M1-8 terms as the
            // pane fields below: a workspace `amx work` created carries it, one
            // created by hand writes exactly the bytes M1 wrote.
            worktree: Some(Worktree {
                repo: PathBuf::from("/home/s/amx"),
                branch: "reflow-fix".to_owned(),
                path: PathBuf::from("/home/s/amx--reflow-fix"),
            }),
        }],
        panes: vec![
            PaneSnapshot {
                id: pane_id(),
                short: ShortNumber::new(1),
                label: Some("editor".to_owned()),
                cwd: Some(PathBuf::from("/home/s/amx")),
                // M2's two additive fields, present on one pane and absent on
                // the other, so the golden freezes both shapes at once: an
                // agent pane carries them, a plain shell writes exactly the
                // bytes M1 wrote (R-M1-8, and VERSION stays 1).
                argv: Some(vec!["claude".to_owned()]),
                agent: Some(AgentSnapshot {
                    kind: AgentKind::new("claude").expect("a valid agent id"),
                    name: Some("editor".to_owned()),
                    session_ref: Some(
                        SessionRef::new(RefKind::Id, "c9a3c73b-b184-4871-8e98-79b46b87b635")
                            .expect("a valid session ref"),
                    ),
                    source: Some(RefSource::for_agent(
                        &AgentKind::new("claude").expect("a valid agent id"),
                    )),
                    start_source: StartSource::Started,
                }),
            },
            PaneSnapshot {
                id: other_pane_id(),
                short: ShortNumber::new(2),
                label: None,
                cwd: None,
                argv: None,
                agent: None,
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

#[test]
fn pane_snapshot_with_agent_fields_reads_at_version_1() {
    // R-M1-8's precedent, applied to M2's two fields: `argv` and `agent` are
    // additive and optional under the unknown-field contract, so VERSION stays
    // 1 and the read window stays {1}. No version-window machinery moves for a
    // field addition — only the golden regenerates.
    assert_eq!(VERSION, 1);
    assert_eq!(READ_WINDOW, &[VERSION]);

    let snapshot = populated();
    let json = serde_json::to_value(&snapshot).unwrap();
    assert_eq!(json["version"], 1);
    assert_eq!(json["panes"][0]["argv"][0], "claude");
    assert_eq!(json["panes"][0]["agent"]["kind"], "claude");
    assert_eq!(json["panes"][0]["agent"]["source"], "amx:claude");
    assert_eq!(json["panes"][0]["agent"]["start_source"], "started");

    // Absent, not null, on a pane with no agent: a v1 writer that never saw an
    // agent produces exactly the bytes M1 produced.
    assert!(json["panes"][1].get("argv").is_none(), "{json}");
    assert!(json["panes"][1].get("agent").is_none(), "{json}");

    let decoded: Snapshot = serde_json::from_value(json).unwrap();
    assert_eq!(decoded, snapshot);
    decoded.check_version().expect("v1 reads at v1");

    // A file written *before* M2 still reads: the fields default to absent.
    let pre_m2 = serde_json::json!({
        "version": 1,
        "workspaces": [],
        "panes": [{ "id": pane_id(), "short": 1, "cwd": "/home/s/amx" }],
    });
    let decoded: Snapshot = serde_json::from_value(pre_m2).unwrap();
    assert!(decoded.panes[0].argv.is_none());
    assert!(decoded.panes[0].agent.is_none());
}

#[test]
fn a_hand_edited_agent_ref_does_not_survive_the_read() {
    // D-M2-7's snapshot-read gate, and the reason the ref types deserialize
    // through their own constructors: a `session.json` is user-editable, and a
    // ref that reached V15's `plan()` unvalidated would put whatever it held
    // into an argv. Each of these is a whole-file parse failure, so restore
    // reports the loss and brings the pane back as a plain shell rather than
    // acting on the bytes.
    let hostile = [
        // A relative path, which would resolve against whatever cwd the server
        // happens to have.
        serde_json::json!({ "kind": "path", "value": "../../.ssh/id_ed25519" }),
        // Control characters, which a terminal would interpret if the ref were
        // ever printed and an argv splitter might act on.
        serde_json::json!({ "kind": "id", "value": "abc\u{001b}[2Jdef" }),
        // Empty.
        serde_json::json!({ "kind": "id", "value": "" }),
    ];
    for value in hostile {
        let file = serde_json::json!({
            "version": 1,
            "workspaces": [],
            "panes": [{
                "id": pane_id(),
                "short": 1,
                "agent": {
                    "kind": "claude",
                    "session_ref": value,
                    "start_source": "started",
                },
            }],
        });
        assert!(
            serde_json::from_value::<Snapshot>(file).is_err(),
            "a hand-edited ref of {value} must not parse",
        );
    }

    // And a source that is not one of amx's own hook paths, which is the shape
    // half of D-M2-7's allowlist. The other half — that the id names the same
    // stanza the ref claims — needs the registry and is V15's, at plan time.
    let foreign = serde_json::json!({
        "version": 1,
        "workspaces": [],
        "panes": [{
            "id": pane_id(),
            "short": 1,
            "agent": {
                "kind": "claude",
                "source": "herdr:claude",
                "start_source": "started",
            },
        }],
    });
    assert!(serde_json::from_value::<Snapshot>(foreign).is_err());
}
