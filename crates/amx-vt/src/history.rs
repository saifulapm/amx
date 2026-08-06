//! Raw scrollback reads and viewport addressing.
//!
//! This is deliberately the *minimal* surface the vendored headers support
//! today, and no more. The scrollback identity model in 04 §3 — monotonic row
//! ids, an eviction floor, `history.invalidated{from_row}` — is amx-side
//! bookkeeping that libghostty-vt has no concept of (M0 plan R4), and it is
//! T12's to design. Nothing here assigns an identity to a row, hashes one,
//! tracks eviction, or claims a row read twice is the same row.
//!
//! **What is deliberately not built here:** `RowId` allocation, the eviction
//! floor, content hashes, invalidation causes, and any notion that a row
//! survives a reflow. Building them on top of these reads without the T12 spike
//! would mean inventing an identity the library does not provide.
//!
//! ## What the headers do settle (M0 plan R5)
//!
//! R5 records as unverified whether history can be read "without moving the
//! live viewport", and warns that `GHOSTTY_SCROLL_VIEWPORT_ROW` mutates the
//! viewport that other clients and the application can see. The headers answer
//! it: [`Terminal::read_row`] resolves a [`Point`] tagged
//! [`PointSpace::History`] through `ghostty_terminal_grid_ref` and reads the
//! cells off the resulting reference. That path does not touch the viewport at
//! all, so an arbitrary history range is readable while the user stays scrolled
//! wherever they are.
//!
//! The cost is real and is the part R5 got right. `terminal.h:1489` says the
//! `screen` and `history` tags "may require traversing the full scrollback page
//! list to resolve the y coordinate", and `grid_ref.h:28` says the grid
//! reference API is "not meant to be used as the core of a render loop … not
//! built to sustain the framerates needed". One resolve per cell is what the
//! C API offers for this today; it belongs in a bulk history transfer served on
//! the parser thread, never on the frame path.

use std::marker::PhantomData;

use crate::enums::{CellWide, PointSpace};
use crate::error::{Error, Result, check, check_optional};
use crate::sys;
use crate::terminal::Terminal;

/// A row address in one of the terminal's coordinate spaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Point {
    /// Which space `y` is measured in.
    pub space: PointSpace,
    /// The row, zero-indexed within that space.
    pub y: u32,
}

impl Point {
    /// A row of the scrollback, counted from the oldest surviving row.
    #[must_use]
    pub fn history(y: u32) -> Self {
        Self {
            space: PointSpace::History,
            y,
        }
    }

    /// A row of what is currently visible.
    #[must_use]
    pub fn viewport(y: u32) -> Self {
        Self {
            space: PointSpace::Viewport,
            y,
        }
    }

    fn to_sys(self, x: u16) -> sys::GhosttyPoint {
        sys::GhosttyPoint {
            tag: self.space.to_sys(),
            value: sys::GhosttyPointValue {
                coordinate: sys::GhosttyPointCoordinate { x, y: self.y },
            },
        }
    }
}

/// Where the viewport sits in the scrollable area.
///
/// The same row space as [`Scroll::Row`], so a position read here can be
/// scrolled back to exactly (`terminal.h:249`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Scrollbar {
    /// Total scrollable rows.
    pub total: u64,
    /// The first visible row's offset into that total.
    pub offset: u64,
    /// How many rows are visible.
    pub len: u64,
}

/// How to move the viewport.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scroll {
    /// To the oldest row of the scrollback.
    Top,
    /// Back to the active area.
    Bottom,
    /// By a number of rows; negative is up.
    Delta(isize),
    /// To an absolute row offset, clamped so the viewport never passes the top
    /// of the active area.
    Row(usize),
}

impl Scroll {
    fn to_sys(self) -> sys::GhosttyTerminalScrollViewport {
        use sys::GhosttyTerminalScrollViewportTag as Tag;
        let (tag, value) = match self {
            Self::Top => (
                Tag::GHOSTTY_SCROLL_VIEWPORT_TOP,
                sys::GhosttyTerminalScrollViewportValue { delta: 0 },
            ),
            Self::Bottom => (
                Tag::GHOSTTY_SCROLL_VIEWPORT_BOTTOM,
                sys::GhosttyTerminalScrollViewportValue { delta: 0 },
            ),
            Self::Delta(delta) => (
                Tag::GHOSTTY_SCROLL_VIEWPORT_DELTA,
                sys::GhosttyTerminalScrollViewportValue { delta },
            ),
            Self::Row(row) => (
                Tag::GHOSTTY_SCROLL_VIEWPORT_ROW,
                sys::GhosttyTerminalScrollViewportValue { row },
            ),
        };
        sys::GhosttyTerminalScrollViewport { tag, value }
    }
}

/// What a [`Terminal::read_row`] found besides the text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RowRead {
    /// How many cells contributed, spacers excluded.
    pub cells: u16,
    /// Whether the row soft-wraps into the next one. A caller reassembling a
    /// logical line follows this rather than assuming one row is one line.
    pub wrapped: bool,
}

impl Terminal {
    /// Rows in the active screen, scrollback included.
    ///
    /// # Errors
    ///
    /// Propagates a library failure.
    pub fn total_rows(&self) -> Result<usize> {
        // SAFETY: the out-pointer type matches GHOSTTY_TERMINAL_DATA_TOTAL_ROWS.
        unsafe {
            self.get(
                sys::GhosttyTerminalData::GHOSTTY_TERMINAL_DATA_TOTAL_ROWS,
                0usize,
            )
        }
    }

    /// Rows of scrollback: [`Terminal::total_rows`] minus the viewport.
    ///
    /// # Errors
    ///
    /// Propagates a library failure.
    pub fn scrollback_rows(&self) -> Result<usize> {
        // SAFETY: the out-pointer type matches
        // GHOSTTY_TERMINAL_DATA_SCROLLBACK_ROWS.
        unsafe {
            self.get(
                sys::GhosttyTerminalData::GHOSTTY_TERMINAL_DATA_SCROLLBACK_ROWS,
                0usize,
            )
        }
    }

    /// Whether the viewport is following the active area rather than parked in
    /// history.
    ///
    /// # Errors
    ///
    /// Propagates a library failure.
    pub fn viewport_pinned(&self) -> Result<bool> {
        // SAFETY: the out-pointer type matches
        // GHOSTTY_TERMINAL_DATA_VIEWPORT_ACTIVE.
        unsafe {
            self.get(
                sys::GhosttyTerminalData::GHOSTTY_TERMINAL_DATA_VIEWPORT_ACTIVE,
                false,
            )
        }
    }

    /// Where the viewport sits.
    ///
    /// The header notes there is intentionally no change notification for this;
    /// it is polled once per frame or per write batch and diffed.
    ///
    /// # Errors
    ///
    /// Propagates a library failure.
    pub fn scrollbar(&self) -> Result<Scrollbar> {
        // SAFETY: the out-pointer type matches GHOSTTY_TERMINAL_DATA_SCROLLBAR.
        let raw: sys::GhosttyTerminalScrollbar = unsafe {
            self.get(
                sys::GhosttyTerminalData::GHOSTTY_TERMINAL_DATA_SCROLLBAR,
                sys::GhosttyTerminalScrollbar::default(),
            )?
        };
        Ok(Scrollbar {
            total: raw.total,
            offset: raw.offset,
            len: raw.len,
        })
    }

    /// Move the viewport.
    ///
    /// This mutates state the running application can observe through queries
    /// and mouse reporting, so it is the *user's* scroll, not a way to read
    /// history — use [`Terminal::read_row`] for that.
    pub fn scroll_viewport(&mut self, scroll: Scroll) {
        // SAFETY: the terminal is live and exclusively ours; the tagged union
        // is built with the tag and arm the header pairs.
        unsafe { sys::ghostty_terminal_scroll_viewport(self.as_raw(), scroll.to_sys()) };
    }

    /// Append one row's text to `out`, leaving the viewport alone.
    ///
    /// Empty cells become spaces and spacer cells are skipped, so the result is
    /// column-aligned with the grid. Trailing blanks are kept: trimming is a
    /// presentation decision and this is a raw read.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidValue`] if the point is outside the grid — which is also
    /// how a caller finds the end of the scrollback.
    pub fn read_row(&self, point: Point, out: &mut String) -> Result<RowRead> {
        let cols = self.cols()?;
        let mut read = RowRead::default();
        for x in 0..cols {
            let reference = self.grid_ref(point, x)?;
            if x == 0 {
                read.wrapped = row_wrapped(&reference)?;
            }
            match cell_wide(&reference)? {
                // The second half of a wide character carries no text of its
                // own; the character before it already occupies both columns.
                CellWide::SpacerTail => continue,
                CellWide::Narrow | CellWide::Wide | CellWide::SpacerHead => {}
            }
            let before = out.len();
            push_graphemes(&reference, out)?;
            if out.len() == before {
                out.push(' ');
            }
            read.cells = read.cells.saturating_add(1);
        }
        Ok(read)
    }

    /// Anchor a tracked reference on a row, at column zero.
    ///
    /// The library moves the anchor as the page list moves underneath it —
    /// "scrolling, scrollback pruning, resize/reflow, and other terminal
    /// mutations update the tracked reference automatically"
    /// (`grid_ref.h:48`) — which is the one primitive that lets a caller count
    /// what the scrollback discarded. See [`TrackedRow`].
    ///
    /// # Errors
    ///
    /// [`Error::InvalidValue`] if the point is outside its coordinate space,
    /// [`Error::OutOfMemory`] if the library cannot allocate the anchor.
    pub fn track(&mut self, point: Point) -> Result<TrackedRow> {
        let mut raw: sys::GhosttyTrackedGridRef = std::ptr::null_mut();
        // SAFETY: the terminal is live and exclusively ours; `raw` is a live
        // out-pointer.
        check(unsafe {
            sys::ghostty_terminal_grid_ref_track(self.as_raw(), point.to_sys(0), &raw mut raw)
        })?;
        if raw.is_null() {
            return Err(Error::InvalidValue);
        }
        Ok(TrackedRow {
            raw,
            _not_send: PhantomData,
        })
    }

    fn grid_ref(&self, point: Point, x: u16) -> Result<sys::GhosttyGridRef> {
        let mut reference = sys::GhosttyGridRef {
            size: size_of::<sys::GhosttyGridRef>(),
            ..Default::default()
        };
        // SAFETY: the terminal is live and `reference` is a live out-pointer
        // with its `size` set as the sized-struct ABI requires.
        check(unsafe {
            sys::ghostty_terminal_grid_ref(self.as_raw(), point.to_sys(x), &raw mut reference)
        })?;
        Ok(reference)
    }
}

/// A row anchor that survives everything except losing its row.
///
/// The library follows the anchored cell across scrolls, prunes and reflows and
/// reports where it ended up, so asking an anchor of known identity for its
/// current history offset says exactly how many rows the scrollback has thrown
/// away underneath it. That subtraction is the whole mechanism behind the
/// eviction floor; nothing else in the C API distinguishes a row that was
/// committed from a row that was pruned.
///
/// Neither [`Send`] nor [`Sync`]: resolving an anchor reads the terminal's page
/// list, so it may only be used on the thread that owns the terminal.
#[derive(Debug)]
pub struct TrackedRow {
    raw: sys::GhosttyTrackedGridRef,
    /// A raw pointer would do it, but spelling the bound out means adding an
    /// `unsafe impl Send` reads as the mistake it would be.
    _not_send: PhantomData<*const ()>,
}

impl TrackedRow {
    /// Whether the anchored row still exists.
    ///
    /// False once the row is pruned, cleared or otherwise discarded in a way
    /// the library cannot map to a new cell (`grid_ref.h:52`), and false after
    /// the terminal is freed.
    #[must_use]
    pub fn alive(&self) -> bool {
        // SAFETY: the handle is live for as long as this value exists and the
        // call is documented to be valid even after its terminal is freed.
        unsafe { sys::ghostty_tracked_grid_ref_has_value(self.raw) }
    }

    /// Where the anchored row sits now, in `space`.
    ///
    /// `None` when the row is gone or is not representable in that space — a
    /// row still in the active area has no history coordinate, and a row in
    /// history has no active one.
    ///
    /// # Errors
    ///
    /// Propagates a library failure other than "no value".
    pub fn row(&self, space: PointSpace) -> Result<Option<u32>> {
        let mut point = sys::GhosttyPointCoordinate::default();
        // SAFETY: the handle is live and `point` is a live out-pointer.
        let found = check_optional(unsafe {
            sys::ghostty_tracked_grid_ref_point(self.raw, space.to_sys(), &raw mut point)
        })?;
        Ok(found.then_some(point.y))
    }

    /// Re-anchor on another row of the same terminal.
    ///
    /// Clears any "no value" state, so an anchor whose row was pruned is reused
    /// rather than reallocated.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidValue`] if the point is outside its coordinate space.
    pub fn set(&mut self, terminal: &mut Terminal, point: Point) -> Result<()> {
        // SAFETY: the handle is live and `terminal` is the terminal that
        // created it — the only one that could have, since neither this type
        // nor `Terminal` can leave the owning thread.
        check(unsafe {
            sys::ghostty_tracked_grid_ref_set(self.raw, terminal.as_raw(), point.to_sys(0))
        })
    }
}

impl Drop for TrackedRow {
    fn drop(&mut self) {
        // SAFETY: the handle came from a successful `grid_ref_track` and is
        // freed exactly once. `grid_ref_tracked.h:30` allows this after the
        // terminal is already gone.
        unsafe { sys::ghostty_tracked_grid_ref_free(self.raw) };
    }
}

fn row_wrapped(reference: &sys::GhosttyGridRef) -> Result<bool> {
    let mut row: sys::GhosttyRow = 0;
    // SAFETY: the reference was just resolved and no mutating terminal call has
    // happened since, which is the whole of its documented lifetime.
    check(unsafe { sys::ghostty_grid_ref_row(reference, &raw mut row) })?;
    let mut wrapped = false;
    // SAFETY: the out-pointer type matches GHOSTTY_ROW_DATA_WRAP.
    check(unsafe {
        sys::ghostty_row_get(
            row,
            sys::GhosttyRowData::GHOSTTY_ROW_DATA_WRAP,
            (&raw mut wrapped).cast(),
        )
    })?;
    Ok(wrapped)
}

fn cell_wide(reference: &sys::GhosttyGridRef) -> Result<CellWide> {
    let mut cell: sys::GhosttyCell = 0;
    // SAFETY: as `row_wrapped`.
    check(unsafe { sys::ghostty_grid_ref_cell(reference, &raw mut cell) })?;
    let mut wide: sys::GhosttyCellWide::Type = 0;
    // SAFETY: the out-pointer type matches GHOSTTY_CELL_DATA_WIDE.
    check(unsafe {
        sys::ghostty_cell_get(
            cell,
            sys::GhosttyCellData::GHOSTTY_CELL_DATA_WIDE,
            (&raw mut wide).cast(),
        )
    })?;
    CellWide::from_sys(wide).ok_or(Error::InvalidValue)
}

/// Append a cell's grapheme cluster, if it has one.
///
/// Most clusters are one codepoint, so the common case reads into the stack
/// buffer and never allocates.
fn push_graphemes(reference: &sys::GhosttyGridRef, out: &mut String) -> Result<()> {
    let mut inline = [0u32; 16];
    let mut len = 0usize;
    // SAFETY: as `row_wrapped`; the buffer is described honestly.
    let result = unsafe {
        sys::ghostty_grid_ref_graphemes(reference, inline.as_mut_ptr(), inline.len(), &raw mut len)
    };
    match check(result) {
        Ok(()) => push_codepoints(&inline[..len], out),
        Err(Error::OutOfSpace) => {
            let mut spilled = vec![0u32; len];
            // SAFETY: `spilled` is exactly the size the library asked for.
            check(unsafe {
                sys::ghostty_grid_ref_graphemes(
                    reference,
                    spilled.as_mut_ptr(),
                    spilled.len(),
                    &raw mut len,
                )
            })?;
            push_codepoints(&spilled[..len.min(spilled.len())], out);
        }
        Err(other) => return Err(other),
    }
    Ok(())
}

fn push_codepoints(codepoints: &[u32], out: &mut String) {
    for &codepoint in codepoints {
        // A zero codepoint is how the library spells an empty cell; anything
        // that is not a scalar value cannot have come from the parser, and
        // dropping it is better than corrupting the row.
        if let Some(character) = char::from_u32(codepoint).filter(|c| *c != '\0') {
            out.push(character);
        }
    }
}
