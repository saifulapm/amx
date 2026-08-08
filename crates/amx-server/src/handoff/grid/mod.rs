//! The styled visible grid, as a replay.
//!
//! libghostty-vt has no serialization API and the grid is one mutable FFI
//! object that cannot be copied out (06 R4/R5), so D-M3-4 does what herdr's
//! `initial_history_ansi` does with better inputs: synthesize the screen from
//! the published POD snapshot — cells already carry resolved fg/bg and SGR
//! attributes, rows already carry wrap flags — as cursor-addressed SGR runs
//! plus a cursor, and replay it into a fresh terminal on the other side.
//!
//! That is what makes "no visible screen content lost" literal rather than
//! approximate: colors, attributes and layout survive, not just text. The
//! fidelity bound that does not close is history, which crosses unstyled —
//! R-M1-1's accepted precedent, inherited rather than rediscovered. R-M3-2 is
//! the risk this module carries: wide characters, spacer cells, wrapped rows
//! and grapheme clustering all have to survive synthesize→replay, and property
//! tests over random styled grids are the defense.
//!
//! # How a column is put back
//!
//! Four cell classes need four different sequences, and the differences are not
//! cosmetic — the first is the difference between a column that was *never
//! written* and one holding a space, which the snapshot distinguishes and a
//! client's damage encoder therefore does too:
//!
//! - a cell with a grapheme cluster is **printed**, under the SGR run its
//!   colours and attributes ask for;
//! - a blank cell with nothing to show is **stepped over** with `CUF`, because
//!   printing a space there would turn an unwritten column into a written one;
//! - a blank cell that carries a background — what `ECH`/`EL` leave behind — is
//!   **erased** with `ECH` under that background, which is the one sequence
//!   that paints a column without giving it text;
//! - a blank cell that carries *attributes* is **printed as a space** under
//!   them, because no sequence can do better. The library is explicit about the
//!   shape: `Screen.blankCell` returns `style.bgCell()`, a cell holding a
//!   background and nothing else, so an erase can colour a column and can never
//!   underline one. The space renders identically and costs the never-written
//!   bit; the alternative — dropping the attributes — costs a visible underline.
//!
//! # Wrapped rows
//!
//! A row's soft-wrap flag is not a property anything can set; it is what the
//! terminal records when a print runs off the last column. So wrapped rows are
//! emitted as one continuous stream — a *run* — addressed once at its first row
//! and not repositioned until the wrap has actually happened, because a cursor
//! movement before that clears the pending-wrap state and loses the very flag
//! being reproduced. Two columns of every run therefore have to be printed even
//! when they are blank, and each is taken back the only way that works there:
//!
//! - the **last column** of a wrapping row is printed and then un-printed with
//!   `ICH`, which clears a cell and — alone among the erasing sequences —
//!   leaves the row's soft-wrap alone;
//! - the **first column** of a continuation row is printed to resolve the
//!   pending wrap, and then erased with `ECH`, which is safe there because the
//!   flag it resets belongs to *this* row and this row's own last column sets
//!   it again afterwards.
//!
//! The exception is the spacer head — the padding libghostty-vt leaves when a
//! wide character will not fit in the last column. It is not printed at all:
//! the wide character on the *next* row is written at that column and the
//! library lays the padding down itself, exactly as it did on the exporter.
//!
//! # What the replay assumes, and what it cannot carry
//!
//! The target screen must be **blank**. Nothing here clears it, deliberately:
//! `ED 2` on the primary screen is not a clear at all when the last non-empty
//! row looks like a prompt — `Terminal.eraseDisplay` turns it into a scroll,
//! which would push rows into the scrollback and move every row id the
//! manifest just carried across. [`super::manifest::PaneSeed`] guarantees the
//! blank screen instead, by scrolling the replayed history off the top.
//!
//! Bounds this module cannot close, stated rather than papered over (R-M3-2):
//!
//! - **History crosses unstyled.** Scrollback rides as packed rows, which carry
//!   characters and a wrap flag and nothing else — R-M1-1's accepted precedent,
//!   inherited by the manifest.
//! - **A blank column carrying attributes crosses as a space** carrying the
//!   same attributes, as above. Found by the property test, not reasoned about
//!   in advance: it is reachable only by mutating a grid after the fact, and
//!   there is no sequence that produces it.
//! - **A spacer head is re-derived, not carried.** Its paint comes from the
//!   wide character that wraps past it — that print is the only thing that
//!   makes one — so a head whose paint was left over from an earlier print
//!   comes back with the wrapping character's paint instead, and a head whose
//!   wide character has since been erased comes back as a blank column. Every
//!   sequence that could touch the cell afterwards clears it outright:
//!   `Screen.splitCellBoundary` erases a spacer head whenever the wide
//!   character below it is disturbed. The wrap flag itself always survives.
//! - **A pane on the alternate screen crosses with a blank primary.** Only one
//!   grid is published, and it is the active one; the primary screen behind an
//!   alternate is not reachable through the C API at all. Its scrollback still
//!   crosses, so leaving the alternate screen shows the replayed history rather
//!   than the pre-alternate grid.
//! - **A scrolling region (DECSTBM) does not cross.** The terminal exposes no
//!   accessor for it, so there is nothing to read and nothing to replay.
//! - **A palette-indexed underline colour does not cross.** `amx-vt` already
//!   flattens one to "no explicit colour" on the way into the snapshot
//!   (`render/rows.rs::style_color`), so it is lost before this module sees it.
//! - **A cursor scrolled out of the viewport keeps only its visibility.** The
//!   position is undefined in that state and is not replayed; amx keeps the
//!   server's viewport pinned to the live bottom (04 §3), so it does not arise.
//!
//! # The three files
//!
//! This one is the grid: the pen, the run, and the four column classes above.
//! [`replay`] is everything that goes *around* a grid — the paint prologue, the
//! packed history rows, the modes and the cursor — and [`emit`] is the byte
//! level both reach for. Split by responsibility on arrival at 601 lines rather
//! than at the hard limit (R-M1-3); the directory needs no change to
//! `handoff/mod.rs`, which is what W05 established when `protocol.rs` became
//! `protocol/`.

mod emit;
mod replay;

use amx_vt::{Cell, CellWide, Rgb, Row, Snapshot, Style, Underline};

use self::emit::{put_csi, put_cup, put_number};
pub use self::replay::{put_cursor, put_modes, put_paint_prologue, put_rows_replay};

/// Reset every SGR attribute to its default.
const SGR_RESET: &[u8] = b"\x1b[0m";

/// A cell's paint: two resolved colours and the SGR attributes.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
struct Pen {
    foreground: Option<Rgb>,
    background: Option<Rgb>,
    style: Style,
}

impl Pen {
    /// What this cell is painted with.
    fn of(cell: &Cell) -> Self {
        Self {
            foreground: cell.foreground,
            background: cell.background,
            style: cell.style,
        }
    }

    /// Whether a blank column painted this way needs a character written into
    /// it to look right.
    ///
    /// A background can be *erased* into a column — `ECH` leaves exactly that
    /// and nothing else. An attribute cannot: `Screen.blankCell` returns
    /// `style.bgCell()`, which carries a background and drops everything else,
    /// so libghostty-vt offers no sequence at all that underlines a column
    /// without writing one. A blank column carrying attributes therefore
    /// crosses as a space carrying the same attributes — see the module
    /// documentation's fidelity bounds.
    fn needs_ink(self) -> bool {
        self.style != Style::default() || self.foreground.is_some()
    }

    /// Whether a blank column painted this way is indistinguishable from one
    /// that was never written.
    fn invisible(self) -> bool {
        self.background.is_none() && !self.needs_ink()
    }
}

/// Append the bytes that rebuild `snapshot`'s visible grid.
///
/// The cursor is *not* part of this: [`put_cursor`] emits it after the modes,
/// for the reason stated there. The target must be a terminal of the same size
/// with a blank screen; see the module documentation for why nothing here
/// clears one.
pub fn synthesize(snapshot: &Snapshot, out: &mut Vec<u8>) {
    let grid = snapshot.grid();
    let cols = snapshot.cols();
    out.extend_from_slice(SGR_RESET);
    let mut pen = Pen::default();
    let mut unwrite = Vec::new();
    let mut start = 0;
    while start < grid.len() {
        // A run is this row plus every row it soft-wraps into: one stream, one
        // cursor address, and no repositioning until the wrap has happened.
        let mut end = start;
        while end + 1 < grid.len() && grid[end].wrapped() {
            end += 1;
        }
        put_cup(out, start, 0);
        for (offset, row) in grid[start..=end].iter().enumerate() {
            let index = start + offset;
            let paint = Paint {
                index,
                cols,
                after_wrap: index > start,
                wraps: index < end,
                next_wide: index < end && starts_wide(&grid[index + 1]),
            };
            if let Some(pen) = put_row(row, paint, &mut pen, out) {
                unwrite.push((index, pen));
            }
        }
        start = end + 1;
    }
    put_unwrite(&unwrite, cols, &mut pen, out);
    if pen != Pen::default() {
        out.extend_from_slice(SGR_RESET);
    }
}

/// Where a row sits and what its neighbours make it responsible for.
#[derive(Clone, Copy, Debug)]
struct Paint {
    /// The row's index in the grid.
    index: usize,
    /// The grid's width.
    cols: u16,
    /// The row is the continuation of one already emitted, so its first column
    /// has to be printed for the pending wrap to resolve.
    after_wrap: bool,
    /// The row wraps into the next one, so its last column has to be printed
    /// for the wrap to happen at all.
    wraps: bool,
    /// The next row begins with a wide character, which is what makes the
    /// spacer head at the end of this one: printed there, it wraps by itself
    /// and the library lays the padding down as it did the first time.
    next_wide: bool,
}

/// Whether a row begins with a wide character carrying a cluster.
fn starts_wide(row: &Row) -> bool {
    row.cells()
        .first()
        .is_some_and(|cell| cell.wide == CellWide::Wide && !row.text(cell).is_empty())
}

/// Take back the spaces the wrap mechanics demanded.
///
/// The last column of a wrapping row cannot be left unwritten — nothing else
/// sets a soft-wrap flag — so it is printed and then un-printed here, once the
/// wrap it caused has already happened. `ICH` is the sequence that can do it:
/// `Terminal.insertBlanks` clears the cell under the cursor and, alone among
/// the erasing sequences, leaves the row's soft-wrap alone, where `ECH` and
/// `DCH` both call `cursorResetWrap` and would undo the wrap being reproduced.
/// The blank it leaves carries the cursor's background, which is why the pen
/// the column wanted is set again first.
fn put_unwrite(rows: &[(usize, Pen)], cols: u16, pen: &mut Pen, out: &mut Vec<u8>) {
    for &(index, wanted) in rows {
        put_cup(out, index, usize::from(cols).saturating_sub(1));
        put_pen(wanted, pen, out);
        put_csi(out, 1, b'@');
    }
}

/// Append one row; the answer is the pen its last column must be un-printed
/// under, when the wrap mechanics forced a space into it.
fn put_row(row: &Row, paint: Paint, pen: &mut Pen, out: &mut Vec<u8>) -> Option<Pen> {
    let cells = row.cells();
    let width = cells.len().min(usize::from(paint.cols));
    let limit = if paint.wraps {
        width
    } else {
        painted_len(row, width)
    };
    // A continuation row's first column has to be printed or the pending wrap
    // never resolves and the row above never records one. If that column is
    // meant to be blank the space is written anyway and erased on the spot:
    // `ECH` is safe here, because the flag it resets is *this* row's, which
    // nothing has set yet and the row's own last column will set later.
    let mut erase_first = false;
    if paint.after_wrap && width > 0 && steppable(row, &cells[0]) {
        put_pen(Pen::default(), pen, out);
        out.push(b' ');
        put_cup(out, paint.index, 0);
        erase_first = true;
    }
    // The space just printed is a column the row has to account for, even in a
    // row that has nothing else worth emitting.
    let limit = limit.max(usize::from(erase_first));
    let mut unwrite = None;
    let mut skipped = 0u16;
    let mut at = 0usize;
    while at < limit {
        let cell = &cells[at];
        let text = row.text(cell);
        if !text.is_empty() {
            put_skip(&mut skipped, out);
            put_pen(Pen::of(cell), pen, out);
            out.extend_from_slice(text);
            // A wide character occupies its spacer tail as well, and the tail
            // carries no cluster of its own to print.
            at += if cell.wide == CellWide::Wide { 2 } else { 1 };
            continue;
        }
        // A spacer the library will lay down itself is left to it: the tail
        // belongs to the wide character before it, and the head belongs to the
        // wide character that wraps past it — but only if that character is
        // still there. A head whose wide character has since been erased
        // cannot be re-derived by printing anything, so it crosses as the blank
        // column it looks like (see the module's fidelity bounds).
        let spacer = match cell.wide {
            CellWide::SpacerTail => true,
            CellWide::SpacerHead => paint.wraps && at + 1 == width && paint.next_wide,
            CellWide::Narrow | CellWide::Wide => false,
        };
        let wanted = Pen::of(cell);
        let last = paint.wraps && at + 1 == width;
        if !spacer && (wanted.needs_ink() || last) {
            // Either the column carries attributes no erase can paint, or it is
            // the last column of a wrapping row and something has to be printed
            // there for the wrap to happen. A column that only wanted the wrap
            // is taken back afterwards; one that wanted the attributes keeps
            // its space, which is the closest thing the C API can express.
            put_skip(&mut skipped, out);
            put_pen(wanted, pen, out);
            out.push(b' ');
            if last && !wanted.needs_ink() {
                unwrite = Some(wanted);
            }
            at += 1;
            continue;
        }
        let mut run = 1usize;
        if !spacer {
            while at + run + 1 < width.max(1)
                && at + run < limit
                && blank_painted(row, &cells[at + run], wanted)
            {
                run += 1;
            }
        }
        // `ECH` is the one sequence that paints a column without writing text
        // into it; it leaves the cursor where it was, so the columns are still
        // stepped over below. An invisible blank needs none of it — unless a
        // space was just printed into it to resolve a wrap.
        if !spacer && (!wanted.invisible() || (erase_first && at == 0)) {
            put_skip(&mut skipped, out);
            put_pen(wanted, pen, out);
            put_csi(out, u32::try_from(run).unwrap_or(u32::MAX), b'X');
        }
        skipped = skipped.saturating_add(u16::try_from(run).unwrap_or(u16::MAX));
        at += run;
    }
    if paint.wraps {
        // The cursor has to reach the last column for the wrap to happen, and
        // a row whose last column is a spacer head leaves that step unspent.
        put_skip(&mut skipped, out);
    }
    unwrite
}

/// Whether this cell is a blank painted exactly like `paint`.
fn blank_painted(row: &Row, cell: &Cell, paint: Pen) -> bool {
    steppable(row, cell) && Pen::of(cell) == paint
}

/// Whether the cursor can pass over this cell without printing into it.
fn steppable(row: &Row, cell: &Cell) -> bool {
    row.text(cell).is_empty()
        && !matches!(cell.wide, CellWide::SpacerTail | CellWide::SpacerHead)
        && !Pen::of(cell).needs_ink()
}

/// How many columns of a row are worth emitting.
///
/// Trailing columns that were never written and carry no background are left
/// off: on a blank target they are already what they should be, and stepping
/// over them would only cost bytes.
fn painted_len(row: &Row, width: usize) -> usize {
    let cells = row.cells();
    for at in (0..width).rev() {
        let cell = &cells[at];
        if !row.text(cell).is_empty() || !Pen::of(cell).invisible() {
            return at + 1;
        }
    }
    0
}

/// Step the cursor over the columns nothing was emitted for.
fn put_skip(skipped: &mut u16, out: &mut Vec<u8>) {
    if *skipped > 0 {
        put_csi(out, u32::from(*skipped), b'C');
        *skipped = 0;
    }
}

/// Emit the SGR run `want` asks for, if it is not already in force.
///
/// Always a full reset plus the whole paint rather than a difference: a
/// synthesized screen is written once per handoff, and a run that only says
/// what changed is a run that goes wrong the first time an attribute has no
/// reset code.
fn put_pen(want: Pen, pen: &mut Pen, out: &mut Vec<u8>) {
    if want == *pen {
        return;
    }
    *pen = want;
    let style = want.style;
    let mut used = 0;
    out.extend_from_slice(b"\x1b[");
    put_sgr(out, &mut used, &[0], false);
    for (on, code) in [
        (style.bold, 1),
        (style.faint, 2),
        (style.italic, 3),
        (style.blink, 5),
        (style.inverse, 7),
        (style.invisible, 8),
        (style.strikethrough, 9),
        (style.overline, 53),
    ] {
        if on {
            put_sgr(out, &mut used, &[code], false);
        }
    }
    // `4:n` is the sub-parameter form the vendored parser reads for every
    // underline shape; plain `4` and `21` are the two it also spells directly.
    match style.underline {
        Underline::None => {}
        Underline::Single => put_sgr(out, &mut used, &[4], false),
        Underline::Double => put_sgr(out, &mut used, &[21], false),
        Underline::Curly => put_sgr(out, &mut used, &[4, 3], true),
        Underline::Dotted => put_sgr(out, &mut used, &[4, 4], true),
        Underline::Dashed => put_sgr(out, &mut used, &[4, 5], true),
    }
    put_color(out, &mut used, 38, want.foreground);
    put_color(out, &mut used, 48, want.background);
    put_color(out, &mut used, 58, style.underline_color);
    out.push(b'm');
}

/// How many parameters one emitted SGR sequence may carry.
///
/// The vendored parser drops a CSI outright once it has filled `MAX_PARAMS`
/// (24, `Parser.zig:203`) — every attribute of the sequence, not just the
/// overflow — and a cell wearing every attribute plus three direct colours
/// needs more than that. So a pen is emitted as however many sequences it
/// takes; SGR accumulates, so only the first one carries the reset.
const SGR_PARAM_LIMIT: usize = 16;

/// Append one SGR parameter group, starting a new sequence if this one is full.
fn put_sgr(out: &mut Vec<u8>, used: &mut usize, params: &[u32], colon: bool) {
    if *used + params.len() > SGR_PARAM_LIMIT {
        out.extend_from_slice(b"m\x1b[");
        *used = 0;
    } else if *used > 0 {
        out.push(b';');
    }
    for (index, value) in params.iter().enumerate() {
        if index > 0 {
            out.push(if colon { b':' } else { b';' });
        }
        put_number(out, *value);
    }
    *used += params.len();
}

/// Append one direct-colour SGR parameter, if the colour is set.
fn put_color(out: &mut Vec<u8>, used: &mut usize, code: u32, color: Option<Rgb>) {
    let Some(rgb) = color else { return };
    put_sgr(
        out,
        used,
        &[
            code,
            2,
            u32::from(rgb.r),
            u32::from(rgb.g),
            u32::from(rgb.b),
        ],
        false,
    );
}
