//! What survives the trip from the wire's cell to the bytes this client writes.
//!
//! DR-1's residual: the codec landed in M3 carrying ten attributes, and
//! `model::grid::Attrs` kept four of them plus two colours, so "the smart
//! client renders what the server sent" was a claim about six of ten. These
//! tests are the end of that. Every attribute `amx_proto::stream::CellStyle`
//! can express is asserted twice — once where the decode leaves it, and once in
//! the escape sequence the frame writer emits for it — because a field that is
//! decoded and never written is the same blank screen as a field that was
//! dropped.
//!
//! Fails without the change in the plainest way there is: the struct these
//! tests read has no field for five of the attributes, and the two that
//! collapsed to one boolean cannot be told apart.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

mod support;

use std::collections::HashMap;

use amx_client::app::App;
use amx_client::model::{Attrs, Cell, ClientModel, Color, WorkspaceModel};
use amx_client::render::FrameWriter;
use amx_client::stream::{Bindings, apply};
use amx_core::{GridGeneration, Layout as BspLayout, PaneId, WorkspaceId};
use amx_proto::stream::{
    CellRef, CellStyle, CellWide, Cells, Cursor, CursorShape, GridMessage, Rgb, Underline,
};
use amx_proto::{ClientInfo, FrameHeader};
use bytes::BufMut;

const CHANNEL: u8 = 7;

/// Every boolean attribute the wire carries, set, with a coloured curly
/// underline on top — the richest cell `CellStyle` can express.
fn every_attribute() -> CellStyle {
    CellStyle {
        bold: true,
        italic: true,
        faint: true,
        blink: true,
        inverse: true,
        invisible: true,
        strikethrough: true,
        overline: true,
        underline: Underline::Curly,
        underline_color: Some(Rgb { r: 9, g: 9, b: 9 }),
    }
}

fn cursor() -> Cursor {
    Cursor {
        row: 0,
        col: 0,
        visible: true,
        shape: CursorShape::default(),
        blink: false,
    }
}

/// The single-cell grid `style` produces, as the client caches it.
fn decoded_attrs(style: CellStyle) -> Attrs {
    let mut packed = Vec::new();
    // One row, self-delimiting: a `u16` cell count, then the cells.
    packed.put_u16_le(1);
    amx_proto::stream::cell::encode(
        &CellRef {
            text: b"x",
            wide: CellWide::Narrow,
            foreground: Some(Rgb { r: 1, g: 2, b: 3 }),
            background: None,
            style,
        },
        &mut packed,
    );

    let mut payload = Vec::new();
    GridMessage::Reset {
        generation: GridGeneration::from_raw(1),
        rows: 1,
        cols: 1,
        cells: Cells::new(&packed),
        cursor: cursor(),
    }
    .encode(&mut payload);

    let pane = PaneId::new_v4();
    let mut model = ClientModel::new(1, 1);
    let mut caches = HashMap::new();
    let mut bindings = Bindings::new();
    bindings.bind_grid(pane, CHANNEL);

    let len = u32::try_from(payload.len()).expect("the payload fits a frame");
    apply(
        &mut model,
        &mut caches,
        &bindings,
        FrameHeader::new(len, CHANNEL),
        &payload,
    );

    amx_client::stream::grid_of(&model, pane)
        .expect("the reset created the pane's grid")
        .cell(0, 0)
        .expect("the grid has its one cell")
        .attrs
}

/// The bytes the frame writer emits for one cell wearing `attrs`.
fn written(attrs: Attrs) -> String {
    let mut writer = FrameWriter::new();
    writer.begin_frame();
    writer.write_cell(&Cell { ch: 'x', attrs });
    String::from_utf8(writer.bytes().to_vec()).expect("the frame is utf-8")
}

#[test]
fn every_attribute_the_wire_carries_reaches_the_cache() {
    let attrs = decoded_attrs(every_attribute());

    assert_eq!(attrs.fg, Color::Rgb(1, 2, 3));
    assert_eq!(attrs.bg, Color::Default);
    assert_eq!(attrs.underline_color, Color::Rgb(9, 9, 9));
    assert_eq!(attrs.underline, Underline::Curly);
    assert!(attrs.bold);
    assert!(attrs.faint);
    assert!(attrs.italic);
    assert!(attrs.blink);
    assert!(attrs.reverse, "inverse is spelled `reverse` in the cache");
    assert!(attrs.invisible);
    assert!(attrs.strikethrough);
    assert!(attrs.overline);
}

#[test]
fn every_attribute_the_cache_holds_reaches_the_terminal() {
    let frame = written(decoded_attrs(every_attribute()));

    for (sequence, what) in [
        ("\x1b[1m", "bold"),
        ("\x1b[2m", "faint"),
        ("\x1b[3m", "italic"),
        ("\x1b[5m", "blink"),
        ("\x1b[7m", "inverse"),
        ("\x1b[8m", "invisible"),
        ("\x1b[9m", "strikethrough"),
        ("\x1b[53m", "overline"),
        ("\x1b[4:3m", "the curly underline"),
        ("\x1b[38;2;1;2;3m", "the foreground"),
        ("\x1b[58;2;9;9;9m", "the underline colour"),
    ] {
        assert!(
            frame.contains(sequence),
            "{what} must reach the terminal: {frame:?}",
        );
    }
    assert!(
        !frame.contains("\x1b[48;"),
        "a cell with no background asks for none: {frame:?}",
    );
}

#[test]
fn the_attributes_that_used_to_be_dropped_are_distinguishable() {
    // The five `Attrs` had no field for, and the two spellings of underline it
    // collapsed to one boolean. Every pair here decoded to the same cache entry
    // before this change, and every pair now differs.
    let bare = CellStyle {
        bold: true,
        italic: true,
        inverse: true,
        underline: Underline::Single,
        ..CellStyle::default()
    };
    let dressed = CellStyle {
        faint: true,
        blink: true,
        invisible: true,
        strikethrough: true,
        overline: true,
        ..bare
    };
    assert_ne!(decoded_attrs(bare), decoded_attrs(dressed));

    for (one, other) in [
        (
            CellStyle {
                faint: true,
                ..CellStyle::default()
            },
            CellStyle::default(),
        ),
        (
            CellStyle {
                blink: true,
                ..CellStyle::default()
            },
            CellStyle::default(),
        ),
        (
            CellStyle {
                invisible: true,
                ..CellStyle::default()
            },
            CellStyle::default(),
        ),
        (
            CellStyle {
                strikethrough: true,
                ..CellStyle::default()
            },
            CellStyle::default(),
        ),
        (
            CellStyle {
                overline: true,
                ..CellStyle::default()
            },
            CellStyle::default(),
        ),
    ] {
        assert_ne!(
            decoded_attrs(one),
            decoded_attrs(other),
            "each attribute is its own field, not a shared one",
        );
    }
}

#[test]
fn the_underline_style_and_its_colour_are_not_a_boolean() {
    let single = CellStyle {
        underline: Underline::Single,
        ..CellStyle::default()
    };
    let curly = CellStyle {
        underline: Underline::Curly,
        ..CellStyle::default()
    };
    let coloured = CellStyle {
        underline_color: Some(Rgb { r: 200, g: 0, b: 0 }),
        ..curly
    };

    assert_ne!(decoded_attrs(single), decoded_attrs(curly));
    assert_ne!(decoded_attrs(curly), decoded_attrs(coloured));
    assert_eq!(
        decoded_attrs(CellStyle::default()).underline,
        Underline::None
    );

    // And each shape is its own sequence on the way out. `Single` keeps the
    // bare `4` every terminal has always understood; the shapes that have no
    // single-parameter spelling take the sub-parameter form.
    for (underline, sequence) in [
        (Underline::Single, "\x1b[4m"),
        (Underline::Double, "\x1b[4:2m"),
        (Underline::Curly, "\x1b[4:3m"),
        (Underline::Dotted, "\x1b[4:4m"),
        (Underline::Dashed, "\x1b[4:5m"),
    ] {
        let frame = written(Attrs {
            underline,
            ..Attrs::default()
        });
        assert!(
            frame.contains(sequence),
            "{underline:?} must be written as {sequence:?}: {frame:?}",
        );
    }
    assert!(
        !written(Attrs::default()).contains("\x1b[4"),
        "an unstyled cell asks for no underline at all",
    );
}

#[test]
fn a_run_of_one_style_pays_for_its_escape_sequence_once() {
    // The differ's whole reason to exist, restated over the widened struct: the
    // equality guard now covers twelve fields, and a field left out of it would
    // make two different cells share one paint.
    let attrs = decoded_attrs(every_attribute());
    let mut writer = FrameWriter::new();
    writer.begin_frame();
    for _ in 0..4 {
        writer.write_cell(&Cell { ch: 'x', attrs });
    }
    let frame = String::from_utf8(writer.bytes().to_vec()).expect("the frame is utf-8");
    assert_eq!(
        frame.matches("\x1b[0m").count(),
        1,
        "four cells of one style are one paint: {frame:?}",
    );

    // And a cell that differs in any single attribute repaints.
    for style in [
        CellStyle {
            faint: false,
            ..every_attribute()
        },
        CellStyle {
            underline: Underline::Dotted,
            ..every_attribute()
        },
        CellStyle {
            underline_color: None,
            ..every_attribute()
        },
    ] {
        let mut writer = FrameWriter::new();
        writer.begin_frame();
        writer.write_cell(&Cell { ch: 'x', attrs });
        writer.write_cell(&Cell {
            ch: 'y',
            attrs: decoded_attrs(style),
        });
        let frame = String::from_utf8(writer.bytes().to_vec()).expect("the frame is utf-8");
        assert_eq!(
            frame.matches("\x1b[0m").count(),
            2,
            "a changed attribute must repaint: {frame:?}",
        );
    }
}

/// The last clause of the acceptance: a peeked pane and a focused pane render
/// the same cells identically.
///
/// X15 says it holds by construction — both surfaces go through
/// `render::grid::blit` — and construction is exactly what a later surface can
/// fork without noticing. So it is asserted on the bytes: the same cell, drawn
/// once as a pane this terminal is focused on and once inside a peek region,
/// paints the same escape run either way.
#[tokio::test]
async fn a_peeked_pane_and_a_focused_pane_paint_the_same_bytes() {
    let attrs = decoded_attrs(every_attribute());
    // The run for one cell, char included, as the writer emits it from a fresh
    // frame: a reset, every attribute, the colours, the glyph.
    let run = written(attrs);

    let server = support::Server::start("cell-style-peek").await;
    let pty = support::open_pty_sized(24, 80);
    let mut app = App::attach(
        server.socket(),
        pty.slave,
        Vec::new(),
        ClientInfo {
            name: "amx-cell-style-test".to_owned(),
            version: "0.0.0".to_owned(),
            term: None,
        },
    )
    .await
    .expect("attach to the real server");

    let styled = vec![
        Cell { ch: 'x', attrs },
        Cell {
            ch: 'y',
            attrs: Attrs::default(),
        },
    ];
    let blank = Cursor {
        row: 0,
        col: 0,
        visible: false,
        shape: CursorShape::default(),
        blink: false,
    };

    // Focused: the pane this terminal is drawing.
    let shown = PaneId::new_v4();
    let workspace = WorkspaceId::new_v4();
    app.model().set_workspace(
        workspace,
        WorkspaceModel {
            label: None,
            layout: BspLayout::with_root(shown),
        },
    );
    app.model().focus_workspace(workspace);
    app.model().pane_mut(shown, 1, 2).apply_reset(
        GridGeneration::FIRST.next(),
        1,
        2,
        &styled,
        blank,
    );
    app.repaint();
    let focused = String::from_utf8(app.frame().to_vec()).expect("the frame is utf-8");

    // Peeked: a pane in no workspace this client mirrors, in the region D15
    // reserves for it.
    let watched = PaneId::new_v4();
    app.open_peek(watched).await.expect("open the peek");
    app.model().pane_mut(watched, 1, 2).apply_reset(
        GridGeneration::FIRST.next(),
        1,
        2,
        &styled,
        blank,
    );
    app.repaint();
    let peeked = String::from_utf8(app.frame().to_vec()).expect("the frame is utf-8");

    assert!(
        focused.contains(&run),
        "a focused pane paints the cell whole: {focused:?}",
    );
    assert!(
        peeked.contains(&run),
        "a peeked pane paints the same run, byte for byte: {peeked:?}",
    );

    drop(app);
    server.shutdown().await;
}
