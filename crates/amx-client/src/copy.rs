//! Copy mode: vi keys over the scrollback cache, in stable-row coordinates
//! (04 §7).
//!
//! Selections and the viewport are addressed by [`RowId`], never by screen
//! offset. That single choice is what makes the two contract behaviors fall
//! out for free: output committing to history while the user is scrolled back
//! moves no coordinate they are holding, and `history.invalidated{from_row}`
//! names exactly the selections that must die — the ones touching a row at or
//! beyond `from_row`.
//!
//! The module splits the same way the rest of the input path does. [`decode`]
//! is the pure key table; `input`'s `Mode::Copy` arm uses it (via
//! [`mode_after`]) so a byte's power to end the mode is decided *here*, in the
//! file that owns the mode, and never drifts from what the engine does with
//! the same byte. [`CopyMode`] is the engine: it holds the cursor, the
//! selection anchor and the view top, moves them over a [`Scrollback`], and
//! turns `y` into an OSC 52 write for the client's own terminal.
//!
//! Keys: `hjkl` movement, `0`/`$` line start/end, `g`/`G` oldest/newest,
//! `v` toggles the selection anchor, `y` yanks and leaves, `Esc`/`q` leave.
//! `e` (scrollback in `$EDITOR`, 04 §7) needs process spawning the client
//! does not do yet and is not claimed here. History rows are unstyled text in
//! M0 (the T12 spike), so columns are character positions within that text.

use amx_core::{RowId, RowRange};

use crate::app::Mode;
use crate::cache::{RowSlot, Scrollback};

/// The navigate-layer key that enters copy mode (04 §7: copy mode is entered
/// from navigate). Owned here so the entry key and the mode's own key table
/// live in one file.
pub const ENTER: u8 = b'c';

/// `Esc`.
const ESC: u8 = 0x1b;

/// One copy-mode key, decoded.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Key {
    /// `h`: cursor left.
    Left,
    /// `j`: cursor down (newer).
    Down,
    /// `k`: cursor up (older).
    Up,
    /// `l`: cursor right.
    Right,
    /// `0`: start of line.
    LineStart,
    /// `$`: end of line.
    LineEnd,
    /// `g`: oldest fetchable row.
    Top,
    /// `G`: newest committed row.
    Bottom,
    /// `v`: toggle the selection anchor.
    Select,
    /// `y`: yank the selection and leave the mode.
    Yank,
    /// `Esc`/`q`: leave the mode.
    Leave,
    /// Anything else: consumed, meaningless.
    Other,
}

/// The copy-mode key table. Pure, so the input machine and the engine cannot
/// disagree about what a byte means.
#[must_use]
pub const fn decode(b: u8) -> Key {
    match b {
        b'h' => Key::Left,
        b'j' => Key::Down,
        b'k' => Key::Up,
        b'l' => Key::Right,
        b'0' => Key::LineStart,
        b'$' => Key::LineEnd,
        b'g' => Key::Top,
        b'G' => Key::Bottom,
        b'v' => Key::Select,
        b'y' => Key::Yank,
        ESC | b'q' => Key::Leave,
        _ => Key::Other,
    }
}

/// The mode the input machine is left in after `b` in copy mode: `y`, `Esc`
/// and `q` end it (`y` yanks on its way out), everything else is consumed by
/// the engine. This is the whole of the `Mode::Copy` arm's dispatch, kept
/// here beside [`decode`] so both readers of a byte read one table.
#[must_use]
pub const fn mode_after(b: u8) -> Mode {
    match decode(b) {
        Key::Yank | Key::Leave => Mode::Terminal,
        _ => Mode::Copy,
    }
}

/// A position in the scrollback: a stable row id and a character column
/// within that row's text.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Point {
    /// The row, by stable id.
    pub row: RowId,
    /// Character offset into the row's text.
    pub col: u16,
}

/// What one engine key did, beyond moving state.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Outcome {
    /// Still in copy mode.
    Continue,
    /// The mode ended with nothing to write.
    Exit,
    /// The mode ended with a yank: write these bytes (an OSC 52 sequence) to
    /// the client's *own* terminal so its emulator sets the clipboard.
    Yank(Vec<u8>),
}

/// The copy-mode engine: cursor, selection and view over one pane's cache.
#[derive(Clone, Debug)]
pub struct CopyMode {
    /// Viewport height in rows.
    height: u16,
    /// Oldest visible row — the scroll position, in stable coordinates.
    top: RowId,
    /// The cursor.
    cursor: Point,
    /// The selection anchor, set by `v`. The selection is `anchor..=cursor`
    /// in either order.
    anchor: Option<Point>,
}

impl CopyMode {
    /// Open copy mode at the bottom of `cache`'s history, over a viewport of
    /// `height` rows. `None` when the pane has no fetchable history to enter.
    #[must_use]
    pub fn open(cache: &Scrollback, height: u16) -> Option<Self> {
        let newest = cache.newest()?;
        let height = height.max(1);
        let top = newest
            .get()
            .saturating_sub(u64::from(height) - 1)
            .max(cache.floor().0.get());
        Some(Self {
            height,
            top: RowId::from_raw(top),
            cursor: Point {
                row: newest,
                col: 0,
            },
            anchor: None,
        })
    }

    /// The cursor.
    #[must_use]
    pub const fn cursor(&self) -> Point {
        self.cursor
    }

    /// The oldest visible row.
    #[must_use]
    pub const fn top(&self) -> RowId {
        self.top
    }

    /// The rows the viewport covers.
    #[must_use]
    pub fn viewport(&self) -> RowRange {
        let last = self
            .top
            .get()
            .saturating_add(u64::from(self.height))
            .saturating_sub(1);
        RowRange::new(self.top, RowId::from_raw(last))
    }

    /// The selection, ordered oldest-first, if one is active.
    #[must_use]
    pub fn selection(&self) -> Option<(Point, Point)> {
        let anchor = self.anchor?;
        let (a, b) = (anchor, self.cursor);
        if (a.row.get(), a.col) <= (b.row.get(), b.col) {
            Some((a, b))
        } else {
            Some((b, a))
        }
    }

    /// The range requests that would fill the viewport's cache misses —
    /// what the embedding sends to the server, and empty exactly when
    /// scrolling is being served from the cache alone.
    pub fn wanted(&self, cache: &Scrollback, out: &mut Vec<RowRange>) {
        cache.misses(self.viewport(), out);
    }

    /// Apply one key to the engine.
    ///
    /// The byte's mode consequence must agree with [`mode_after`]: both go
    /// through [`decode`], so `y`/`Esc`/`q` end the mode here exactly when the
    /// input machine leaves it.
    pub fn key(&mut self, b: u8, cache: &Scrollback) -> Outcome {
        match decode(b) {
            Key::Leave => return Outcome::Exit,
            Key::Yank => return self.yank(cache),
            Key::Left => self.cursor.col = self.cursor.col.saturating_sub(1),
            Key::Right => {
                self.cursor.col = self.cursor.col.saturating_add(1);
                self.clamp_col(cache);
            }
            Key::Up => self.step(cache, true),
            Key::Down => self.step(cache, false),
            Key::LineStart => self.cursor.col = 0,
            Key::LineEnd => {
                if let Some(end) = line_end(cache, self.cursor.row) {
                    self.cursor.col = end;
                }
            }
            Key::Top => {
                self.cursor.row = cache.floor().0;
                self.clamp_row(cache);
                self.clamp_col(cache);
            }
            Key::Bottom => {
                if let Some(newest) = cache.newest() {
                    self.cursor.row = newest;
                }
                self.clamp_col(cache);
            }
            Key::Select => {
                self.anchor = match self.anchor {
                    Some(_) => None,
                    None => Some(self.cursor),
                };
            }
            Key::Other => {}
        }
        self.follow_cursor();
        Outcome::Continue
    }

    /// Answer `history.invalidated{from_row}` — call after the cache itself
    /// was invalidated. Any selection touching a row at or beyond `from_row`
    /// is cancelled (04 §3), and the cursor and view are pulled back onto
    /// rows that survive. Returns `false` when nothing survives and the
    /// embedding should leave copy mode.
    pub fn on_invalidated(&mut self, from_row: RowId, cache: &Scrollback) -> bool {
        if self
            .anchor
            .is_some_and(|a| a.row >= from_row || self.cursor.row >= from_row)
        {
            self.anchor = None;
        }
        self.resync(cache)
    }

    /// Re-clamp the cursor and view after the cache's bounds moved (an
    /// invalidation pulled the head back, or an eviction raised the floor).
    /// Returns `false` when no fetchable history remains.
    pub fn resync(&mut self, cache: &Scrollback) -> bool {
        let Some(newest) = cache.newest() else {
            return false;
        };
        let floor = cache.floor().0;
        if self.cursor.row > newest {
            self.cursor.row = newest;
        }
        if self.cursor.row < floor {
            self.cursor.row = floor;
        }
        if let Some((a, _)) = self.selection()
            && a.row < floor
        {
            // An eviction under the selection: its oldest rows can never be
            // fetched again, so the selection cannot be yanked honestly.
            self.anchor = None;
        }
        self.clamp_col(cache);
        if self.top > self.cursor.row || self.top < floor {
            self.top = self.cursor.row;
        }
        self.follow_cursor();
        true
    }

    /// Move the cursor one row, clamped to the fetchable window.
    fn step(&mut self, cache: &Scrollback, up: bool) {
        let row = self.cursor.row.get();
        self.cursor.row = RowId::from_raw(if up {
            row.saturating_sub(1)
        } else {
            row.saturating_add(1)
        });
        self.clamp_row(cache);
        self.clamp_col(cache);
    }

    /// Clamp the cursor's row into `[floor, newest]`.
    fn clamp_row(&mut self, cache: &Scrollback) {
        let floor = cache.floor().0;
        if self.cursor.row < floor {
            self.cursor.row = floor;
        }
        if let Some(newest) = cache.newest()
            && self.cursor.row > newest
        {
            self.cursor.row = newest;
        }
    }

    /// Clamp the cursor's column to the row's last character, when the row is
    /// cached; an uncached row's width is unknown, so the column stands until
    /// the row arrives.
    fn clamp_col(&mut self, cache: &Scrollback) {
        if let Some(end) = line_end(cache, self.cursor.row) {
            self.cursor.col = self.cursor.col.min(end);
        }
    }

    /// Keep the cursor inside the viewport by moving the view, not the
    /// cursor — this is what scrolling *is* in copy mode.
    fn follow_cursor(&mut self) {
        let row = self.cursor.row.get();
        if row < self.top.get() {
            self.top = self.cursor.row;
        } else {
            let bottom = self.top.get().saturating_add(u64::from(self.height) - 1);
            if row > bottom {
                self.top = RowId::from_raw(row.saturating_sub(u64::from(self.height) - 1));
            }
        }
    }

    /// Yank the selection as an OSC 52 write, ending the mode either way.
    ///
    /// Honesty rule, same as the cache's: a selected row that is not cached
    /// contributes nothing invented — the yank is refused whole rather than
    /// padded with blanks, and the mode still ends because `y` ends it in the
    /// key table.
    fn yank(&self, cache: &Scrollback) -> Outcome {
        let Some((start, end)) = self.selection() else {
            return Outcome::Exit;
        };
        let mut text = String::new();
        for id in start.row.get()..=end.row.get() {
            let RowSlot::Cached(row) = cache.slot(RowId::from_raw(id)) else {
                return Outcome::Exit;
            };
            if id > start.row.get() {
                text.push('\n');
            }
            let from = if id == start.row.get() { start.col } else { 0 };
            let to = (id == end.row.get()).then_some(end.col);
            push_cols(row, from, to, &mut text);
        }
        Outcome::Yank(osc52(&text))
    }
}

/// The last character column of `row`, when it is cached: `0` for an empty
/// row, `None` when the row is not cached.
fn line_end(cache: &Scrollback, row: RowId) -> Option<u16> {
    match cache.slot(row) {
        RowSlot::Cached(text) => {
            let chars = text.chars().count();
            Some(u16::try_from(chars.saturating_sub(1)).unwrap_or(u16::MAX))
        }
        RowSlot::Missing | RowSlot::Unavailable => None,
    }
}

/// Append `row`'s characters from column `from` through column `to`
/// (inclusive; to the end of the row when `None`).
fn push_cols(row: &str, from: u16, to: Option<u16>, out: &mut String) {
    for (at, ch) in row.chars().enumerate() {
        if at < usize::from(from) {
            continue;
        }
        if let Some(to) = to
            && at > usize::from(to)
        {
            break;
        }
        out.push(ch);
    }
}

/// The OSC 52 sequence that sets the `c` (clipboard) selection to `text`,
/// terminated with ST. Written to the client's own terminal: the emulator on
/// the user's side of the wire owns the clipboard, which is exactly why copy
/// mode can be fully client-local.
#[must_use]
pub fn osc52(text: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(9 + text.len().div_ceil(3) * 4);
    out.extend_from_slice(b"\x1b]52;c;");
    base64_into(text.as_bytes(), &mut out);
    out.extend_from_slice(b"\x1b\\");
    out
}

/// The standard base64 alphabet, RFC 4648.
const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Append the padded base64 encoding of `data` to `out`. Hand-rolled: the
/// tree takes no dependency for 20 lines of bit shifting.
fn base64_into(data: &[u8], out: &mut Vec<u8>) {
    for chunk in data.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied();
        let b2 = chunk.get(2).copied();
        out.push(BASE64[usize::from(b0 >> 2)]);
        out.push(BASE64[usize::from((b0 & 0x03) << 4 | b1.unwrap_or(0) >> 4)]);
        match b1 {
            Some(b1) => {
                out.push(BASE64[usize::from((b1 & 0x0f) << 2 | b2.unwrap_or(0) >> 6)]);
                match b2 {
                    Some(b2) => out.push(BASE64[usize::from(b2 & 0x3f)]),
                    None => out.push(b'='),
                }
            }
            None => out.extend_from_slice(b"=="),
        }
    }
}
