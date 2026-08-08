//! Protocol goldens: every binary stream message, and the coverage law.
//!
//! T04's suite (`crates/amx-proto/tests/goldens.rs`) pins the control channel:
//! one golden per method plus the envelope shapes, regenerated with
//! `AMX_UPDATE_GOLDENS=1`. This suite extends that same tree and mechanic to
//! the binary stream surface, and adds the law T04 could not state: a method
//! or stream message without a golden is a test failure, not an oversight —
//! with the owed names *derived* from the message enums through exhaustive
//! matches, so a new variant refuses to compile until it says what it owes.
//!
//! Grid stream bytes are produced by the real pipeline: a libghostty-vt
//! terminal fed fixed input, published into a snapshot, run through the
//! server's encoder — the same `amx_proto::stream` codec the client decodes.
//! The goldens therefore freeze the entire path a client decodes, cell layout
//! included.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use std::collections::BTreeSet;
use std::path::Path;

use amx_core::{GridGeneration, PaneId, RowHash, RowId, RowRange};
use amx_proto::control::Method;
use amx_proto::hello::{ClientInfo, Hello};
use amx_proto::rpc::{RequestId, Response};
use amx_proto::stream::grid::{self, Decoded, GridMessage};
use amx_proto::stream::{
    Cells, Cursor, FlowControl, HistoryChunk, RawDirection, RawPaneIo, StreamId, StreamKind,
};
use amx_proto::version;
use amx_server::damage::{DirtySet, Encoder};
use amx_server::history::{RowPack, hash_bytes};
use amx_vt::{Effects, RenderState, Snapshot, Snapshots, Terminal, TerminalOptions};
use rig::{check_bytes_golden, check_json_golden, goldens_dir};
use serde_json::json;

/// A pane id every stream golden names, matching T04's fixture style.
fn pane_id() -> PaneId {
    "00000000-0000-0000-0000-0000000000a1"
        .parse()
        .expect("a pane id")
}

/// A terminal driven to a fixed screen, published through the real pipeline.
struct Vt {
    terminal: Terminal,
    render: RenderState,
    snapshots: Snapshots,
    effects: Effects,
}

impl Vt {
    fn new(rows: u16, cols: u16) -> Self {
        let terminal = Terminal::new(TerminalOptions {
            cols,
            rows,
            max_scrollback: 8192,
        })
        .expect("a terminal");
        Self {
            terminal,
            render: RenderState::new().expect("a render state"),
            snapshots: Snapshots::new(cols, rows),
            effects: Effects::new(),
        }
    }

    fn write(&mut self, bytes: &[u8]) {
        self.terminal.write(bytes, &mut self.effects);
    }

    fn publish(&mut self) -> std::sync::Arc<Snapshot> {
        self.render
            .update(&mut self.terminal)
            .expect("update the render state");
        self.snapshots
            .publish(&mut self.render)
            .expect("publish a snapshot");
        self.snapshots.latest()
    }
}

/// The visible rows of `snapshot`, as text, for the goldens' `message` field.
fn text_rows(snapshot: &Snapshot) -> Vec<String> {
    snapshot
        .grid()
        .iter()
        .map(|row| {
            let mut text = Vec::new();
            for cell in row.cells() {
                text.extend_from_slice(row.text(cell));
            }
            String::from_utf8(text).expect("cell text is utf-8")
        })
        .collect()
}

// ------------------------------------------------------------ stream goldens

#[test]
fn grid_stream_goldens_match() {
    let mut vt = Vt::new(4, 10);
    vt.write(b"hi \x1b[31mred\x1b[0m\r\n\xe4\xbd\xa0 w\r\n\r\nend");
    let snapshot = vt.publish();
    let rows = text_rows(&snapshot);
    let cursor = format!("{:?}", snapshot.cursor());
    let mut encoder = Encoder::new();
    let cap = usize::try_from(amx_proto::frame::DEFAULT_STREAM_FRAME).expect("cap fits");

    encoder
        .reset(GridGeneration::from_raw(7), &snapshot, cap)
        .expect("encode a keyframe");
    check_bytes_golden(
        "stream/grid_reset",
        &json!({
            "kind": "grid.reset keyframe",
            "generation": 7,
            "rows": 4,
            "cols": 10,
            "text": rows,
            "cursor": cursor,
        }),
        encoder.payload(),
    );
    let decoded = grid::decode(encoder.payload()).expect("the keyframe decodes");
    assert!(
        matches!(
            decoded,
            Decoded::Reset { generation, rows, cols, ref grid, .. }
                if generation == GridGeneration::from_raw(7)
                    && rows == 4 && cols == 10 && grid.len() == 4
        ),
        "the keyframe round-trips: {decoded:?}"
    );

    let mut dirty = DirtySet::new(4, 10);
    dirty.mark(0);
    dirty.mark(2);
    encoder
        .delta(GridGeneration::from_raw(8), &snapshot, &dirty, cap)
        .expect("encode a delta");
    assert!(encoder.complete(), "both dirty rows fit one frame");
    check_bytes_golden(
        "stream/grid_delta",
        &json!({
            "kind": "incremental delta",
            "generation": 8,
            "dirty_rows": [0, 2],
            "text": [rows[0], rows[2]],
            "cursor": cursor,
        }),
        encoder.payload(),
    );
    let decoded = grid::decode(encoder.payload()).expect("the delta decodes");
    assert!(
        matches!(
            decoded,
            Decoded::Delta { generation, ref rects, ref grid, .. }
                if generation == GridGeneration::from_raw(8)
                    && rects.len() == 2 && grid.len() == 2
        ),
        "the delta round-trips: {decoded:?}"
    );

    encoder.cursor(&snapshot);
    check_bytes_golden(
        "stream/grid_cursor",
        &json!({ "kind": "cursor-only", "cursor": cursor }),
        encoder.payload(),
    );

    let range = RowRange::new(RowId::from_raw(40), RowId::from_raw(41));
    let hashes = [hash_bytes(b"alpha"), hash_bytes(b"beta")];
    let mut scrolled = Vec::new();
    GridMessage::Scrolled {
        range,
        hashes: &hashes,
    }
    .encode(&mut scrolled);
    check_bytes_golden(
        "stream/grid_scrolled",
        &json!({
            "kind": "rows committed to history",
            "rows": [40, 41],
            "hashes": hashes.iter().map(hash_hex).collect::<Vec<_>>(),
        }),
        &scrolled,
    );
    let decoded = grid::decode(&scrolled).expect("the scroll notice decodes");
    assert_eq!(
        decoded,
        Decoded::Scrolled {
            range,
            hashes: hashes.to_vec()
        }
    );
}

#[test]
fn raw_stream_goldens_match() {
    let message = RawPaneIo {
        pane: pane_id(),
        direction: RawDirection::ToPane,
        bytes: b"ls -la\r",
    };
    let mut bytes = Vec::new();
    message.encode(&mut bytes);
    check_bytes_golden(
        "stream/raw_pane_io",
        &json!({
            "kind": "raw pane input, as if typed",
            "pane": pane_id(),
            "direction": "to_pane",
            "text": "ls -la\\r",
        }),
        &bytes,
    );
    assert_eq!(RawPaneIo::decode(&bytes).expect("the run decodes"), message);
}

#[test]
fn history_chunk_goldens_match() {
    let mut rows = Vec::new();
    amx_proto::stream::history::put_row(b"line0", false, &mut rows);
    amx_proto::stream::history::put_row(b"line1 wraps", true, &mut rows);
    let chunk = HistoryChunk {
        request: RequestId::from(9),
        range: RowRange::new(RowId::from_raw(40), RowId::from_raw(41)),
        more: true,
        rows: &rows,
    };
    let mut bytes = Vec::new();
    chunk.encode(&mut bytes);
    check_bytes_golden(
        "stream/history_chunk",
        &json!({
            "kind": "one chunk of a history range transfer",
            "request": 9,
            "rows": ["line0", "line1 wraps"],
            "more": true,
        }),
        &bytes,
    );
    assert_eq!(
        HistoryChunk::decode(&bytes).expect("the chunk decodes"),
        chunk
    );
}

#[test]
fn history_stream_goldens_match() {
    let mut vt = Vt::new(4, 10);
    vt.write(b"line0\r\nline1\r\nline2\r\nline3\r\nline4\r\nline5\r\n");
    let _ = vt.publish();

    let mut scratch = String::new();
    let mut packed = Vec::new();
    let mut rows = Vec::new();
    let mut hashes = Vec::new();
    for offset in 0..2_u32 {
        let start = packed.len();
        RowPack::append(&vt.terminal, offset, &mut scratch, &mut packed)
            .expect("pack a history row");
        rows.push(scratch.clone());
        hashes.push(hash_hex(&hash_bytes(&packed[start..])));
    }

    check_bytes_golden(
        "stream/history_rows",
        &json!({
            "kind": "packed history rows (chunk payload layout)",
            "rows": rows,
            "hashes": hashes,
        }),
        &packed,
    );
}

#[test]
fn stream_binding_goldens_match() {
    let pane = pane_id();
    check_json_golden("stream/kind_pane_grid", &StreamKind::PaneGrid { pane });
    check_json_golden("stream/kind_history", &StreamKind::History { pane });
    check_json_golden("stream/kind_raw_pane_io", &StreamKind::RawPaneIo { pane });
    check_json_golden("stream/kind_graphics", &StreamKind::Graphics { pane });

    let stream = StreamId::new(1);
    check_json_golden(
        "stream/flow_stream_cap",
        &FlowControl::StreamCap {
            stream,
            max_frame: 65536,
        },
    );
    check_json_golden("stream/flow_pause", &FlowControl::Pause { stream });
    check_json_golden("stream/flow_resume", &FlowControl::Resume { stream });
    check_json_golden("stream/flow_resync", &FlowControl::Resync { stream });
}

// ------------------------------------------------------- envelope additions

#[test]
fn envelope_goldens_match() {
    check_json_golden(
        "proto/response_ok",
        &Response::ok(RequestId::from(1), json!({ "seq": 41 })),
    );
    check_json_golden(
        "proto/hello_first_attach",
        &Hello {
            proto: version::window(),
            features: BTreeSet::new(),
            client: ClientInfo {
                name: "amx".to_owned(),
                version: "0.1.0".to_owned(),
                term: None,
            },
            attach: false,
            resume: None,
        },
    );
}

// ------------------------------------------------------------- the coverage law

#[test]
fn protocol_goldens_cover_every_control_method_and_stream_message() {
    // Control: one golden per method row, derived from the table itself, so a
    // new method fails here until its golden lands.
    let mut proto: BTreeSet<String> = Method::ALL
        .iter()
        .map(|method| format!("method_{}", method.wire_name().replace('.', "_")))
        .collect();
    for envelope in [
        "hello",
        "hello_first_attach",
        "notification",
        // The `events.subscribe` delivery envelope, and the `gap` that rides
        // the same method — frozen separately because a consumer that handled
        // only the first would silently skip the one message that tells it it
        // missed something (04 §2).
        "notification_event",
        "notification_event_gap",
        "response_error",
        // Frozen apart from the generic error envelope because the client
        // branches on its code rather than reading its message (D-M3-7).
        "response_wait_abandoned",
        "response_ok",
        "welcome",
    ] {
        proto.insert(envelope.to_owned());
    }
    assert_files(&goldens_dir().join("proto"), &proto);

    // Streams: walked variant by variant so a new `StreamKind` — or a new
    // message on any stream's codec — refuses to compile until the matches
    // below say what goldens it owes.
    let pane = pane_id();
    let mut stream = BTreeSet::new();
    for kind in [
        StreamKind::PaneGrid { pane },
        StreamKind::History { pane },
        StreamKind::RawPaneIo { pane },
        StreamKind::Graphics { pane },
    ] {
        match kind {
            StreamKind::PaneGrid { .. } => {
                stream.extend(grid_message_goldens());
                stream.insert("kind_pane_grid".to_owned());
            }
            StreamKind::History { .. } => {
                // Chunk envelope plus the row payload layout it carries.
                stream.insert("history_chunk".to_owned());
                stream.insert("history_rows".to_owned());
                stream.insert("kind_history".to_owned());
            }
            StreamKind::RawPaneIo { .. } => {
                stream.insert("raw_pane_io".to_owned());
                stream.insert("kind_raw_pane_io".to_owned());
            }
            StreamKind::Graphics { .. } => {
                // Reserved for kitty images, explicitly not carried in M0.
                stream.insert("kind_graphics".to_owned());
            }
        }
    }
    for flow in [
        FlowControl::StreamCap {
            stream: StreamId::new(1),
            max_frame: 1,
        },
        FlowControl::Pause {
            stream: StreamId::new(1),
        },
        FlowControl::Resume {
            stream: StreamId::new(1),
        },
        FlowControl::Resync {
            stream: StreamId::new(1),
        },
    ] {
        let name = match flow {
            FlowControl::StreamCap { .. } => "flow_stream_cap",
            FlowControl::Pause { .. } => "flow_pause",
            FlowControl::Resume { .. } => "flow_resume",
            FlowControl::Resync { .. } => "flow_resync",
        };
        stream.insert(name.to_owned());
    }
    assert_files(&goldens_dir().join("stream"), &stream);
}

/// The golden each grid stream message variant owes, derived by constructing
/// one of each and matching exhaustively: a new `GridMessage` variant fails to
/// compile here until it names its golden.
fn grid_message_goldens() -> Vec<String> {
    let cursor = Cursor {
        row: 0,
        col: 0,
        visible: true,
        shape: amx_proto::stream::CursorShape::Block,
        blink: false,
    };
    let generation = GridGeneration::FIRST;
    let range = RowRange::new(RowId::from_raw(0), RowId::from_raw(0));
    [
        GridMessage::Reset {
            generation,
            rows: 0,
            cols: 0,
            cells: Cells::new(&[]),
            cursor,
        },
        GridMessage::Delta {
            generation,
            rects: &[],
            cells: Cells::new(&[]),
            cursor,
        },
        GridMessage::Scrolled { range, hashes: &[] },
        GridMessage::Cursor(cursor),
    ]
    .iter()
    .map(|message| {
        match message {
            GridMessage::Reset { .. } => "grid_reset",
            GridMessage::Delta { .. } => "grid_delta",
            GridMessage::Scrolled { .. } => "grid_scrolled",
            GridMessage::Cursor(_) => "grid_cursor",
        }
        .to_owned()
    })
    .collect()
}

/// Assert `dir` holds exactly `expected` golden files — nothing owed missing,
/// nothing stray drifting.
fn assert_files(dir: &Path, expected: &BTreeSet<String>) {
    let present: BTreeSet<String> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("reading {dir:?}: {e}"))
        .map(|entry| entry.expect("a directory entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .map(|path| {
            path.file_stem()
                .expect("a file stem")
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    let missing: Vec<_> = expected.difference(&present).collect();
    let stray: Vec<_> = present.difference(expected).collect();
    assert!(
        missing.is_empty() && stray.is_empty(),
        "goldens in {dir:?} drifted from the protocol surface;\n  missing: {missing:?}\n  stray: {stray:?}\n\
         (a missing file wants AMX_UPDATE_GOLDENS=1; a stray one wants deleting or a row here)"
    );
}

/// A row hash as fixed-width hex, for readable goldens.
fn hash_hex(hash: &RowHash) -> String {
    format!("{:016x}", hash.get())
}
