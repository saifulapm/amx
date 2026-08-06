//! The render state: where damage comes from.
//!
//! `include/ghostty/vt/render.h:42` documents the two-phase update, and it is
//! the reason this type exists separately from [`Terminal`]:
//!
//! > `ghostty_render_state_begin_update` requires terminal access, while
//! > `ghostty_render_state_end_update` completes any deferred work using only
//! > memory owned by the render state.
//!
//! amx does not share a terminal across threads (04 §3), so it does not need
//! the phases to shorten a lock hold — but it does need the *other* half of the
//! contract: after `end_update` the render state's row and cell data is memory
//! the render state owns, which is what makes copying it into a snapshot a
//! plain memory read rather than a second traversal of the live grid.
//!
//! `render.h:60` is the other load-bearing paragraph: dirty state has two
//! independent layers, a global one and a per-row one, `update` sets but never
//! clears either, and clearing one does not clear the other. Both are cleared
//! explicitly during snapshot publication.
//!
//! The handles for row iteration and cell iteration are created once and
//! reused, exactly as `render.h:613` invites — "You can reuse this value
//! repeatedly with `ghostty_render_state_row_get()` to avoid allocating a new
//! cells container for every row" — which is what keeps the frame path free of
//! allocations.
//!
//! Split in two by the module budget: this file owns the render state itself,
//! [`rows`] owns the walk over its rows and cells.

mod rows;

pub use rows::{CellRef, Cells, Row, Rows, Style};

use crate::enums::{CursorStyle, Dirty};
use crate::error::{Error, Result, check};
use crate::sys;
use crate::terminal::Terminal;

/// An RGB colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct Rgb {
    /// Red.
    pub r: u8,
    /// Green.
    pub g: u8,
    /// Blue.
    pub b: u8,
}

impl From<sys::GhosttyColorRgb> for Rgb {
    fn from(raw: sys::GhosttyColorRgb) -> Self {
        Self {
            r: raw.r,
            g: raw.g,
            b: raw.b,
        }
    }
}

/// The cursor, as the renderer needs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Cursor {
    /// Column in the viewport, valid only when `visible_in_viewport`.
    pub x: u16,
    /// Row in the viewport, valid only when `visible_in_viewport`.
    pub y: u16,
    /// Whether the terminal modes say to draw it at all.
    pub visible: bool,
    /// Whether the cursor position falls inside the current viewport. Scrolled
    /// into history, it does not.
    pub visible_in_viewport: bool,
    /// Whether it should blink.
    pub blinking: bool,
    /// Whether it sits on the tail cell of a wide character.
    pub wide_tail: bool,
    /// The shape to draw.
    pub style: CursorStyle,
}

/// The colours a frame is drawn against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Colors {
    /// Default background.
    pub background: Rgb,
    /// Default foreground.
    pub foreground: Rgb,
    /// Explicit cursor colour, if the terminal set one.
    pub cursor: Option<Rgb>,
}

/// A viewport's worth of render state, updated from a terminal.
#[derive(Debug)]
pub struct RenderState {
    raw: sys::GhosttyRenderState,
    rows: sys::GhosttyRenderStateRowIterator,
    cells: sys::GhosttyRenderStateRowCells,
}

// SAFETY: an owned allocation with no thread affinity; every call below takes
// `&self` or `&mut self`, and the terminal it reads is itself `!Sync`.
unsafe impl Send for RenderState {}

impl RenderState {
    /// An empty render state, with its row and cell iterators preallocated.
    ///
    /// # Errors
    ///
    /// [`Error::OutOfMemory`] if the library cannot allocate.
    pub fn new() -> Result<Self> {
        // Built in place so a failure part way still drops what was created:
        // every one of the three free functions accepts NULL.
        let mut state = Self {
            raw: std::ptr::null_mut(),
            rows: std::ptr::null_mut(),
            cells: std::ptr::null_mut(),
        };
        // SAFETY: each argument is a live out-pointer of the documented type;
        // a NULL allocator selects the library default.
        unsafe {
            check(sys::ghostty_render_state_new(
                std::ptr::null(),
                &raw mut state.raw,
            ))?;
            check(sys::ghostty_render_state_row_iterator_new(
                std::ptr::null(),
                &raw mut state.rows,
            ))?;
            check(sys::ghostty_render_state_row_cells_new(
                std::ptr::null(),
                &raw mut state.cells,
            ))?;
        }
        Ok(state)
    }

    /// Read the terminal into the render state, both phases in one call.
    ///
    /// # Errors
    ///
    /// [`Error::OutOfMemory`] if the update needs to allocate and cannot.
    pub fn update(&mut self, terminal: &mut Terminal) -> Result<()> {
        self.begin_update(terminal)?;
        self.end_update()
    }

    /// The phase that needs the terminal. Consumes the terminal's dirty state.
    ///
    /// The render state must not be read until [`RenderState::end_update`] has
    /// run: the header calls it "incomplete" until then.
    ///
    /// # Errors
    ///
    /// As [`RenderState::update`].
    pub fn begin_update(&mut self, terminal: &mut Terminal) -> Result<()> {
        // SAFETY: both handles are live; this is the only call here that
        // touches the terminal, and it takes `&mut Terminal` because it mutates
        // the terminal's dirty tracking.
        check(unsafe { sys::ghostty_render_state_begin_update(self.raw, terminal.as_raw()) })
    }

    /// The phase that does not. Safe to run while the terminal changes again.
    ///
    /// # Errors
    ///
    /// As [`RenderState::update`].
    pub fn end_update(&mut self) -> Result<()> {
        // SAFETY: the handle is live; the header documents this as touching
        // only render-state-owned memory, and as a no-op without a prior begin.
        check(unsafe { sys::ghostty_render_state_end_update(self.raw) })
    }

    /// How much of the frame changed.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidValue`] if the library reports a state this build does
    /// not know.
    pub fn dirty(&self) -> Result<Dirty> {
        // SAFETY: the out-pointer type matches GHOSTTY_RENDER_STATE_DATA_DIRTY.
        let raw: sys::GhosttyRenderStateDirty::Type = unsafe {
            self.get(
                sys::GhosttyRenderStateData::GHOSTTY_RENDER_STATE_DATA_DIRTY,
                0,
            )?
        };
        Dirty::from_sys(raw).ok_or(Error::InvalidValue)
    }

    /// Set the global dirty state.
    ///
    /// Clearing it does **not** clear the per-row flags; those are cleared row
    /// by row through [`Row::set_dirty`].
    ///
    /// # Errors
    ///
    /// Propagates a library failure.
    pub fn set_dirty(&mut self, dirty: Dirty) -> Result<()> {
        let value = dirty.to_sys();
        // SAFETY: the value type matches GHOSTTY_RENDER_STATE_OPTION_DIRTY.
        check(unsafe {
            sys::ghostty_render_state_set(
                self.raw,
                sys::GhosttyRenderStateOption::GHOSTTY_RENDER_STATE_OPTION_DIRTY,
                std::ptr::from_ref(&value).cast(),
            )
        })
    }

    /// Viewport width in cells.
    ///
    /// # Errors
    ///
    /// Propagates a library failure.
    pub fn cols(&self) -> Result<u16> {
        // SAFETY: the out-pointer type matches GHOSTTY_RENDER_STATE_DATA_COLS.
        unsafe {
            self.get(
                sys::GhosttyRenderStateData::GHOSTTY_RENDER_STATE_DATA_COLS,
                0u16,
            )
        }
    }

    /// Viewport height in cells.
    ///
    /// # Errors
    ///
    /// Propagates a library failure.
    pub fn rows(&self) -> Result<u16> {
        // SAFETY: the out-pointer type matches GHOSTTY_RENDER_STATE_DATA_ROWS.
        unsafe {
            self.get(
                sys::GhosttyRenderStateData::GHOSTTY_RENDER_STATE_DATA_ROWS,
                0u16,
            )
        }
    }

    /// The cursor.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidValue`] if the library reports a cursor style this build
    /// does not know.
    pub fn cursor(&self) -> Result<Cursor> {
        use sys::GhosttyRenderStateData as Data;
        // SAFETY: each out-pointer type matches the data kind requested.
        unsafe {
            let style: sys::GhosttyRenderStateCursorVisualStyle::Type =
                self.get(Data::GHOSTTY_RENDER_STATE_DATA_CURSOR_VISUAL_STYLE, 0)?;
            let in_viewport: bool = self.get(
                Data::GHOSTTY_RENDER_STATE_DATA_CURSOR_VIEWPORT_HAS_VALUE,
                false,
            )?;
            // The three viewport fields are documented as undefined unless
            // CURSOR_VIEWPORT_HAS_VALUE is true, so they are not read at all
            // when it is false.
            let (x, y, wide_tail) = if in_viewport {
                (
                    self.get(Data::GHOSTTY_RENDER_STATE_DATA_CURSOR_VIEWPORT_X, 0u16)?,
                    self.get(Data::GHOSTTY_RENDER_STATE_DATA_CURSOR_VIEWPORT_Y, 0u16)?,
                    self.get(
                        Data::GHOSTTY_RENDER_STATE_DATA_CURSOR_VIEWPORT_WIDE_TAIL,
                        false,
                    )?,
                )
            } else {
                (0, 0, false)
            };
            Ok(Cursor {
                x,
                y,
                visible: self.get(Data::GHOSTTY_RENDER_STATE_DATA_CURSOR_VISIBLE, false)?,
                visible_in_viewport: in_viewport,
                blinking: self.get(Data::GHOSTTY_RENDER_STATE_DATA_CURSOR_BLINKING, false)?,
                wide_tail,
                style: CursorStyle::from_sys(style).ok_or(Error::InvalidValue)?,
            })
        }
    }

    /// The frame's default colours.
    ///
    /// # Errors
    ///
    /// Propagates a library failure.
    pub fn colors(&self) -> Result<Colors> {
        let mut raw = sys::GhosttyRenderStateColors {
            size: size_of::<sys::GhosttyRenderStateColors>(),
            ..Default::default()
        };
        // SAFETY: the handle is live and `raw.size` is set as the sized-struct
        // ABI requires (types.h:280).
        check(unsafe { sys::ghostty_render_state_colors_get(self.raw, &raw mut raw) })?;
        Ok(Colors {
            background: raw.background.into(),
            foreground: raw.foreground.into(),
            cursor: raw.cursor_has_value.then(|| raw.cursor.into()),
        })
    }

    /// Start walking the rows of the current frame.
    ///
    /// # Errors
    ///
    /// Propagates a library failure.
    pub fn rows_iter(&mut self) -> Result<Rows<'_>> {
        // SAFETY: the out-pointer is the preallocated row iterator handle,
        // which is what GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR populates.
        check(unsafe {
            sys::ghostty_render_state_get(
                self.raw,
                sys::GhosttyRenderStateData::GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR,
                (&raw mut self.rows).cast(),
            )
        })?;
        Ok(Rows::new(self))
    }

    /// # Safety
    ///
    /// `T` must be the output type `render.h` documents for `data`.
    unsafe fn get<T>(&self, data: sys::GhosttyRenderStateData::Type, initial: T) -> Result<T> {
        let mut value = initial;
        // SAFETY: the caller guarantees the type; `value` is a live
        // out-pointer for the whole call.
        check(unsafe { sys::ghostty_render_state_get(self.raw, data, (&raw mut value).cast()) })?;
        Ok(value)
    }
}

impl Drop for RenderState {
    fn drop(&mut self) {
        // SAFETY: each handle came from its own successful constructor and is
        // freed exactly once; all three accept NULL, which is what they hold if
        // construction failed part way.
        unsafe {
            sys::ghostty_render_state_row_cells_free(self.cells);
            sys::ghostty_render_state_row_iterator_free(self.rows);
            sys::ghostty_render_state_free(self.raw);
        }
    }
}
