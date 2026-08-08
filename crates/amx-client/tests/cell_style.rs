//! What survives the trip from the wire's cell to the client's cached one.
//!
//! `model::grid::Attrs` documents itself as a *reduced* projection of
//! `amx_proto::stream::CellStyle`: ten attributes on the wire, four of them
//! plus the two colours in the cache. That sentence used to say the opposite —
//! that the codec did not exist at all — and it stayed wrong through a whole
//! milestone because nothing executable disagreed with it. These tests are the
//! thing that disagrees. Widening the projection makes them fail, which is the
//! point: the doc comment has to be rewritten in the same change.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

use std::collections::HashMap;

use amx_client::model::{Attrs, ClientModel, Color};
use amx_client::stream::{Bindings, apply};
use amx_core::{GridGeneration, PaneId};
use amx_proto::FrameHeader;
use amx_proto::stream::{
    CellRef, CellStyle, CellWide, Cells, Cursor, CursorShape, GridMessage, Rgb, Underline,
};
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

#[test]
fn the_four_attributes_the_client_keeps_survive_the_wire() {
    let attrs = decoded_attrs(every_attribute());

    assert_eq!(attrs.fg, Color::Rgb(1, 2, 3));
    assert_eq!(attrs.bg, Color::Default);
    assert!(attrs.bold);
    assert!(attrs.italic);
    assert!(attrs.underline);
    assert!(attrs.reverse, "inverse is spelled `reverse` in the cache");
}

#[test]
fn the_five_attributes_the_client_drops_are_indistinguishable_after_decode() {
    // Only the dropped attributes differ. If the cache ever grows a field for
    // one of them, these two stop being equal — and `Attrs`' doc comment,
    // which says which four are kept, is the thing to fix.
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

    assert_eq!(decoded_attrs(bare), decoded_attrs(dressed));
}

#[test]
fn the_underline_style_and_its_colour_collapse_to_one_boolean() {
    let single = CellStyle {
        underline: Underline::Single,
        ..CellStyle::default()
    };
    let curly = CellStyle {
        underline: Underline::Curly,
        underline_color: Some(Rgb { r: 200, g: 0, b: 0 }),
        ..CellStyle::default()
    };
    let none = CellStyle::default();

    assert_eq!(decoded_attrs(single), decoded_attrs(curly));
    assert!(decoded_attrs(curly).underline);
    assert!(!decoded_attrs(none).underline);
}
