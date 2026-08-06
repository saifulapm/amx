//! Walking the rows and cells of one frame.
//!
//! The whole module is a lending iterator: each level hands out a handle that
//! borrows the [`RenderState`], because `render.h:151` is explicit that "row
//! data is only valid as long as the underlying render state is not updated".
//! Expressing that as a borrow means an update while a row handle is alive is a
//! compile error rather than a read of freed memory.

use crate::enums::{CellWide, StyleColor, Underline};
use crate::error::{Error, Result, check};
use crate::render::{RenderState, Rgb};
use crate::sys;

/// A walk over the rows of one frame.
///
/// Row data is only valid until the render state is updated again, which the
/// borrow of the [`RenderState`] enforces.
#[derive(Debug)]
pub struct Rows<'a> {
    state: &'a mut RenderState,
}

impl<'a> Rows<'a> {
    pub(super) fn new(state: &'a mut RenderState) -> Self {
        Self { state }
    }

    /// Advance to the next row, or `None` at the end of the frame.
    pub fn next_row(&mut self) -> Option<Row<'_>> {
        // SAFETY: the iterator handle is live and positioned by the library.
        if unsafe { sys::ghostty_render_state_row_iterator_next(self.state.rows) } {
            Some(Row { state: self.state })
        } else {
            None
        }
    }
}

/// One row of a frame.
#[derive(Debug)]
pub struct Row<'a> {
    state: &'a mut RenderState,
}

impl Row<'_> {
    /// Whether this row changed since its dirty flag was last cleared.
    ///
    /// # Errors
    ///
    /// Propagates a library failure.
    pub fn dirty(&self) -> Result<bool> {
        // SAFETY: the out-pointer type matches
        // GHOSTTY_RENDER_STATE_ROW_DATA_DIRTY and the iterator is on a row.
        unsafe {
            self.get(
                sys::GhosttyRenderStateRowData::GHOSTTY_RENDER_STATE_ROW_DATA_DIRTY,
                false,
            )
        }
    }

    /// Set or clear this row's dirty flag.
    ///
    /// # Errors
    ///
    /// Propagates a library failure.
    pub fn set_dirty(&mut self, dirty: bool) -> Result<()> {
        // SAFETY: the value type matches GHOSTTY_RENDER_STATE_ROW_OPTION_DIRTY
        // and the iterator is on a row.
        check(unsafe {
            sys::ghostty_render_state_row_set(
                self.state.rows,
                sys::GhosttyRenderStateRowOption::GHOSTTY_RENDER_STATE_ROW_OPTION_DIRTY,
                std::ptr::from_ref(&dirty).cast(),
            )
        })
    }

    /// Whether this row soft-wraps into the next one.
    ///
    /// # Errors
    ///
    /// Propagates a library failure.
    pub fn wrapped(&self) -> Result<bool> {
        let row = self.raw_row()?;
        let mut value = false;
        // SAFETY: the out-pointer type matches GHOSTTY_ROW_DATA_WRAP.
        check(unsafe {
            sys::ghostty_row_get(
                row,
                sys::GhosttyRowData::GHOSTTY_ROW_DATA_WRAP,
                (&raw mut value).cast(),
            )
        })?;
        Ok(value)
    }

    fn raw_row(&self) -> Result<sys::GhosttyRow> {
        // SAFETY: the out-pointer type matches
        // GHOSTTY_RENDER_STATE_ROW_DATA_RAW and the iterator is on a row.
        unsafe {
            self.get(
                sys::GhosttyRenderStateRowData::GHOSTTY_RENDER_STATE_ROW_DATA_RAW,
                0 as sys::GhosttyRow,
            )
        }
    }

    /// Start walking this row's cells.
    ///
    /// # Errors
    ///
    /// Propagates a library failure.
    pub fn cells(&mut self) -> Result<Cells<'_>> {
        // SAFETY: the out-pointer is the preallocated row-cells handle, which
        // is what GHOSTTY_RENDER_STATE_ROW_DATA_CELLS populates.
        check(unsafe {
            sys::ghostty_render_state_row_get(
                self.state.rows,
                sys::GhosttyRenderStateRowData::GHOSTTY_RENDER_STATE_ROW_DATA_CELLS,
                (&raw mut self.state.cells).cast(),
            )
        })?;
        Ok(Cells { state: self.state })
    }

    /// # Safety
    ///
    /// `T` must be the output type `render.h` documents for `data`.
    unsafe fn get<T>(&self, data: sys::GhosttyRenderStateRowData::Type, initial: T) -> Result<T> {
        let mut value = initial;
        // SAFETY: the caller guarantees the type.
        check(unsafe {
            sys::ghostty_render_state_row_get(self.state.rows, data, (&raw mut value).cast())
        })?;
        Ok(value)
    }
}

/// A walk over the cells of one row.
#[derive(Debug)]
pub struct Cells<'a> {
    state: &'a mut RenderState,
}

impl Cells<'_> {
    /// Advance to the next cell, or `None` at the end of the row.
    pub fn next_cell(&mut self) -> Option<CellRef<'_>> {
        // SAFETY: the cells handle is live and positioned by the library.
        if unsafe { sys::ghostty_render_state_row_cells_next(self.state.cells) } {
            Some(CellRef { state: self.state })
        } else {
            None
        }
    }
}

/// One cell of one row.
///
/// Nothing here borrows library memory past the call that reads it: text is
/// copied into a caller buffer and everything else is a POD value.
#[derive(Debug)]
pub struct CellRef<'a> {
    state: &'a RenderState,
}

impl CellRef<'_> {
    /// Append this cell's grapheme cluster to `out` as UTF-8, and report how
    /// many bytes it added. An empty cell adds nothing.
    ///
    /// # Errors
    ///
    /// Propagates a library failure other than the too-small-buffer retry.
    pub fn text(&self, out: &mut Vec<u8>) -> Result<usize> {
        loop {
            let start = out.len();
            let spare = out.capacity() - start;
            let mut buffer = sys::GhosttyBuffer {
                ptr: if spare == 0 {
                    std::ptr::null_mut()
                } else {
                    // SAFETY: `start <= capacity`, so this is in bounds.
                    unsafe { out.as_mut_ptr().add(start) }
                },
                cap: spare,
                len: 0,
            };
            // SAFETY: the handle is live and on a cell; the buffer describes
            // the uninitialised tail of `out`, which the library only writes.
            let result = unsafe {
                sys::ghostty_render_state_row_cells_get(
                    self.state.cells,
                    sys::GhosttyRenderStateRowCellsData::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_UTF8,
                    (&raw mut buffer).cast(),
                )
            };
            match check(result) {
                Ok(()) => {
                    // SAFETY: the library wrote `buffer.len` bytes at `start`,
                    // and the call would have failed if that overran capacity.
                    unsafe { out.set_len(start + buffer.len) };
                    return Ok(buffer.len);
                }
                // `buffer.len` is the required capacity in this case.
                Err(Error::OutOfSpace) => out.reserve(buffer.len.max(1)),
                Err(other) => return Err(other),
            }
        }
    }

    /// The cell's resolved background colour, palette indices already looked
    /// up. `None` means "use the frame default".
    ///
    /// # Errors
    ///
    /// Propagates a library failure.
    pub fn background(&self) -> Result<Option<Rgb>> {
        self.color(
            sys::GhosttyRenderStateRowCellsData::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_BG_COLOR,
        )
    }

    /// The cell's resolved foreground colour. Bold-as-bright is deliberately
    /// not applied by the library, and amx does not apply it either — the
    /// client owns presentation (04 §3).
    ///
    /// # Errors
    ///
    /// Propagates a library failure.
    pub fn foreground(&self) -> Result<Option<Rgb>> {
        self.color(
            sys::GhosttyRenderStateRowCellsData::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_FG_COLOR,
        )
    }

    fn color(&self, data: sys::GhosttyRenderStateRowCellsData::Type) -> Result<Option<Rgb>> {
        let mut raw = sys::GhosttyColorRgb::default();
        // SAFETY: the out-pointer type matches both colour data kinds. The
        // header documents "no colour" as GHOSTTY_INVALID_VALUE rather than
        // NO_VALUE for these two, so that is what is folded into `None`.
        let result = unsafe {
            sys::ghostty_render_state_row_cells_get(self.state.cells, data, (&raw mut raw).cast())
        };
        match check(result) {
            Ok(()) => Ok(Some(raw.into())),
            Err(Error::InvalidValue | Error::NoValue) => Ok(None),
            Err(other) => Err(other),
        }
    }

    /// Whether the cell carries any non-default styling.
    ///
    /// Checking this first is cheaper than materialising the whole style, which
    /// is why the header exposes it separately.
    ///
    /// # Errors
    ///
    /// Propagates a library failure.
    pub fn has_styling(&self) -> Result<bool> {
        // SAFETY: the out-pointer type matches
        // GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_HAS_STYLING.
        unsafe {
            self.get(
                sys::GhosttyRenderStateRowCellsData::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_HAS_STYLING,
                false,
            )
        }
    }

    /// The cell's full style.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidValue`] if the library reports an underline style this
    /// build does not know.
    pub fn style(&self) -> Result<Style> {
        let mut raw = sys::GhosttyStyle {
            size: size_of::<sys::GhosttyStyle>(),
            ..Default::default()
        };
        // SAFETY: the out-pointer type matches
        // GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_STYLE and `raw.size` is set as
        // the sized-struct ABI requires.
        check(unsafe {
            sys::ghostty_render_state_row_cells_get(
                self.state.cells,
                sys::GhosttyRenderStateRowCellsData::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_STYLE,
                (&raw mut raw).cast(),
            )
        })?;
        Style::from_sys(&raw)
    }

    /// Whether the cell is narrow, wide, or a spacer for a wide neighbour.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidValue`] if the library reports a width this build does
    /// not know.
    pub fn wide(&self) -> Result<CellWide> {
        // SAFETY: the out-pointer type matches
        // GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_RAW.
        let cell: sys::GhosttyCell = unsafe {
            self.get(
                sys::GhosttyRenderStateRowCellsData::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_RAW,
                0,
            )?
        };
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

    /// # Safety
    ///
    /// `T` must be the output type `render.h` documents for `data`.
    unsafe fn get<T>(
        &self,
        data: sys::GhosttyRenderStateRowCellsData::Type,
        initial: T,
    ) -> Result<T> {
        let mut value = initial;
        // SAFETY: the caller guarantees the type.
        check(unsafe {
            sys::ghostty_render_state_row_cells_get(self.state.cells, data, (&raw mut value).cast())
        })?;
        Ok(value)
    }
}

/// A cell's SGR attributes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Style {
    /// Bold.
    pub bold: bool,
    /// Italic.
    pub italic: bool,
    /// Faint.
    pub faint: bool,
    /// Blinking.
    pub blink: bool,
    /// Inverse video.
    pub inverse: bool,
    /// Invisible.
    pub invisible: bool,
    /// Struck through.
    pub strikethrough: bool,
    /// Overlined.
    pub overline: bool,
    /// Underline style.
    pub underline: Underline,
    /// Underline colour, if it differs from the foreground.
    pub underline_color: Option<Rgb>,
}

impl Style {
    fn from_sys(raw: &sys::GhosttyStyle) -> Result<Self> {
        // The struct field is a plain `int` holding a GHOSTTY_SGR_UNDERLINE_*
        // value, which the bindings type as unsigned; the cast is the only
        // place the two spellings meet.
        #[allow(
            clippy::cast_sign_loss,
            reason = "style.h:107 documents this int as a GhosttySgrUnderline"
        )]
        let underline = Underline::from_sys(raw.underline as sys::GhosttySgrUnderline::Type)
            .ok_or(Error::InvalidValue)?;
        Ok(Self {
            bold: raw.bold,
            italic: raw.italic,
            faint: raw.faint,
            blink: raw.blink,
            inverse: raw.inverse,
            invisible: raw.invisible,
            strikethrough: raw.strikethrough,
            overline: raw.overline,
            underline,
            underline_color: style_color(raw.underline_color),
        })
    }
}

/// Flatten a style colour, dropping palette indices.
///
/// Cell colours arrive already resolved through the palette; the underline
/// colour does not, and amx has no palette of its own to resolve it with, so a
/// palette-indexed underline colour reads as "no explicit colour" and the
/// client falls back to the foreground. This is a real limitation, recorded
/// here rather than hidden.
fn style_color(color: sys::GhosttyStyleColor) -> Option<Rgb> {
    match StyleColor::from_sys(color.tag) {
        // SAFETY: the tag says the RGB arm of the union is the live one.
        Some(StyleColor::Rgb) => Some(unsafe { color.value.rgb }.into()),
        _ => None,
    }
}
