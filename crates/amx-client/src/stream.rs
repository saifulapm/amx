//! Bound streams, client side: which channel carries what, and how a frame
//! lands in the model.
//!
//! [`Bindings`] mirrors the server's per-connection stream table from the
//! client's seat: grid and history channels route inbound frames to the pane
//! they were bound for, raw channels carry this client's keystrokes out.
//! [`apply`] is the one place a stream frame becomes state — a grid message
//! into the pane's cached [`PaneGrid`], a history chunk into its
//! [`Scrollback`] — so the render loop deals in "what changed", never bytes.

use std::collections::HashMap;

use amx_core::PaneId;
use amx_proto::FrameHeader;
use amx_proto::rpc::RequestId;
use amx_proto::stream::cell::{CellWide, PackedCell};
use amx_proto::stream::grid::{Decoded, decode};
use amx_proto::stream::{HistoryChunk, PackedRows};

use crate::cache::Scrollback;
use crate::model::{Attrs, Cell, ClientModel, Color, PaneGrid};

/// This client's live stream bindings.
#[derive(Debug, Default)]
pub struct Bindings {
    /// Inbound grid channels by channel byte.
    grid: HashMap<u8, PaneId>,
    /// Which pane already has a grid stream, to bind each once.
    grid_panes: HashMap<PaneId, u8>,
    /// Outbound raw channels by pane.
    raw: HashMap<PaneId, u8>,
    /// Inbound history channels by channel byte.
    history: HashMap<u8, PaneId>,
    /// Which pane already has a history stream.
    history_panes: HashMap<PaneId, u8>,
}

impl Bindings {
    /// An empty table.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a bound grid stream.
    pub fn bind_grid(&mut self, pane: PaneId, channel: u8) {
        self.grid.insert(channel, pane);
        self.grid_panes.insert(pane, channel);
    }

    /// Record a bound raw input stream.
    pub fn bind_raw(&mut self, pane: PaneId, channel: u8) {
        self.raw.insert(pane, channel);
    }

    /// Record a bound history stream.
    pub fn bind_history(&mut self, pane: PaneId, channel: u8) {
        self.history.insert(channel, pane);
        self.history_panes.insert(pane, channel);
    }

    /// Whether `pane` already has a grid stream bound.
    #[must_use]
    pub fn has_grid(&self, pane: PaneId) -> bool {
        self.grid_panes.contains_key(&pane)
    }

    /// The channel carrying this client's input to `pane`, if bound.
    #[must_use]
    pub fn raw_channel(&self, pane: PaneId) -> Option<u8> {
        self.raw.get(&pane).copied()
    }

    /// The pane a grid channel carries, if anything bound it.
    ///
    /// The reverse of [`Bindings::bind_grid`], for the one caller that starts
    /// from a channel: a reattach asking which pane's cells a torn frame
    /// belonged to ([`crate::net::Torn`]).
    #[must_use]
    pub fn grid_pane(&self, channel: u8) -> Option<PaneId> {
        self.grid.get(&channel).copied()
    }

    /// Whether `pane` already has a history stream bound.
    #[must_use]
    pub fn has_history(&self, pane: PaneId) -> bool {
        self.history_panes.contains_key(&pane)
    }

    /// Drop every binding that names `pane` — after `pane.close`, say.
    ///
    /// The channels stay burned: stream ids are never reused on a connection,
    /// so a late frame for the old binding reads as unbound rather than
    /// landing somewhere new.
    pub fn forget_pane(&mut self, pane: PaneId) {
        self.grid.retain(|_, p| *p != pane);
        self.grid_panes.remove(&pane);
        self.raw.remove(&pane);
        self.history.retain(|_, p| *p != pane);
        self.history_panes.remove(&pane);
    }
}

/// What applying one stream frame changed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Applied {
    /// A pane's cached grid changed; a repaint is due.
    Grid(PaneId),
    /// A history chunk landed in a pane's scrollback cache.
    History {
        /// The pane whose cache grew.
        pane: PaneId,
        /// The caller-chosen request token the chunk answered.
        request: u64,
        /// Whether this was the request's final chunk.
        done: bool,
    },
    /// The frame was not for anything this client tracks.
    Nothing,
}

/// Land one stream frame in the model.
///
/// Undecodable frames and frames for unknown channels are dropped rather than
/// fatal — a stale frame for an unbound stream is a defined protocol
/// condition, not a broken session.
pub fn apply(
    model: &mut ClientModel,
    caches: &mut HashMap<PaneId, Scrollback>,
    bindings: &Bindings,
    header: FrameHeader,
    payload: &[u8],
) -> Applied {
    if let Some(&pane) = bindings.grid.get(&header.channel) {
        let Ok(message) = decode(payload) else {
            return Applied::Nothing;
        };
        apply_grid(model, pane, message);
        return Applied::Grid(pane);
    }
    if let Some(&pane) = bindings.history.get(&header.channel) {
        let Ok(chunk) = HistoryChunk::decode(payload) else {
            return Applied::Nothing;
        };
        let RequestId::Number(request) = chunk.request else {
            return Applied::Nothing;
        };
        let done = !chunk.more;
        apply_history(caches.entry(pane).or_default(), &chunk);
        return Applied::History {
            pane,
            request,
            done,
        };
    }
    Applied::Nothing
}

/// Apply one decoded grid message to the pane's cached grid.
///
/// The grid stream reaches the live grid and nothing else. It used to reach the
/// scrollback cache too, through the scroll notice DR-7 retired; the committed
/// window now moves only where it is announced — `history.committed` on the
/// event bus, and the per-pane `history_head` on `session.state`.
fn apply_grid(model: &mut ClientModel, pane: PaneId, message: Decoded) {
    match message {
        Decoded::Reset {
            generation,
            rows,
            cols,
            grid,
            cursor,
        } => {
            let mut cells = Vec::new();
            for row in &grid {
                push_row(row, cols, &mut cells);
            }
            model
                .pane_mut(pane, rows, cols)
                .apply_reset(generation, rows, cols, &cells, cursor);
        }
        Decoded::Delta {
            generation,
            rects,
            grid,
            cursor,
        } => {
            let target = model.pane_mut(pane, 0, 0);
            let mut cells = Vec::new();
            let mut rows = grid.iter();
            for rect in &rects {
                for _ in 0..rect.rows {
                    match rows.next() {
                        Some(row) => push_row(row, rect.cols, &mut cells),
                        None => push_row(&[], rect.cols, &mut cells),
                    }
                }
            }
            target.apply_delta(generation, &rects, &cells, cursor);
        }
        Decoded::Cursor(cursor) => {
            model.pane_mut(pane, 0, 0).apply_cursor(cursor);
        }
    }
}

/// Land one history chunk in the pane's scrollback cache.
fn apply_history(cache: &mut Scrollback, chunk: &HistoryChunk<'_>) {
    // Commit before fill: a fetch implies the server considered the range
    // committed, and `fill` refuses rows outside the committed window.
    cache.commit(chunk.range);
    let mut texts = Vec::new();
    for row in PackedRows::new(chunk.rows) {
        let Ok(row) = row else { return };
        texts.push(String::from_utf8_lossy(row.text).into_owned());
    }
    cache.fill(chunk.range, texts.iter().map(String::as_str));
}

/// Convert one wire row into `width` model cells, padding with blanks.
fn push_row(row: &[PackedCell], width: u16, out: &mut Vec<Cell>) {
    let before = out.len();
    for cell in row.iter().take(usize::from(width)) {
        out.push(cell_of(cell));
    }
    out.resize(before + usize::from(width), Cell::default());
}

/// One wire cell as the render model holds it.
///
/// The model is char-cell simple in M0: a multi-codepoint grapheme keeps its
/// first scalar, and a wide cell's spacer tail becomes the zero char, which
/// the frame writer skips so the wide glyph keeps both of its columns.
fn cell_of(cell: &PackedCell) -> Cell {
    let ch = match cell.wide {
        CellWide::SpacerTail => '\0',
        _ => cell.text.chars().next().unwrap_or(' '),
    };
    Cell {
        ch,
        attrs: Attrs {
            fg: color_of(cell.foreground),
            bg: color_of(cell.background),
            bold: cell.style.bold,
            italic: cell.style.italic,
            underline: cell.style.underline != amx_proto::stream::Underline::None,
            reverse: cell.style.inverse,
        },
    }
}

fn color_of(rgb: Option<amx_proto::stream::Rgb>) -> Color {
    match rgb {
        Some(rgb) => Color::Rgb(rgb.r, rgb.g, rgb.b),
        None => Color::Default,
    }
}

/// The grid a pane holds, for callers that need its size after an apply.
#[must_use]
pub fn grid_of(model: &ClientModel, pane: PaneId) -> Option<&PaneGrid> {
    model.pane(pane)
}
