//! The terminal handle.
//!
//! 04 §3 states the ownership rule as prose: "the parser I/O thread
//! **exclusively owns the libghostty-vt instance**; VT state is never shared or
//! swapped". [`Terminal`] turns that into a type-level fact. It is [`Send`], so
//! ownership can move onto the parser thread once, and it is **not** [`Sync`],
//! so it can never be shared with a second thread — no `&Terminal` crosses a
//! thread boundary, so no `Arc<Terminal>` and no `static` can exist. The rule
//! stops being something reviewers have to remember.
//!
//! The library agrees: `include/ghostty/vt/terminal.h:1374` says compression
//! "is not thread-safe with other operations on the same terminal. The caller
//! must serialize it with writes, rendering, searches, and other terminal
//! access."

use std::ffi::c_void;
use std::marker::PhantomData;

use crate::callbacks::{self, EffectState, Effects, QueryAnswers};
use crate::enums::Screen;
use crate::error::{Error, Result, check, check_optional};
use crate::sys;

/// How a terminal is created.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalOptions {
    /// Width in cells. Must be greater than zero.
    pub cols: u16,
    /// Height in cells. Must be greater than zero.
    pub rows: u16,
    /// How much memory, in bytes, the scrollback may hold.
    ///
    /// `terminal.h:187` calls it a line count; it is not. Measured against the
    /// vendored library, 8192 holds 4269 rows at 20 columns and 387 at 200, and
    /// the row capacity of one pane moves as its pages recycle. Nothing may be
    /// derived from it — see `docs/notes/scrollback-identity.md`.
    pub max_scrollback: usize,
}

impl Default for TerminalOptions {
    fn default() -> Self {
        Self {
            cols: 80,
            rows: 24,
            max_scrollback: 10_000,
        }
    }
}

/// A packed terminal mode identifier.
///
/// `include/ghostty/vt/modes.h:100` defines the packing: bits 0–14 hold the
/// mode number and bit 15 distinguishes an ANSI mode from a DEC private one.
/// The header's `ghostty_mode_new` is a `static inline`, so bindgen emits
/// neither it nor the `GHOSTTY_MODE_*` macros built on top of it — the packing
/// is reproduced here rather than imported, and the constants below are DEC
/// mode numbers from the VT/xterm specifications, not libghostty-vt enum
/// values, so there is no enum to drift away from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Mode(sys::GhosttyMode);

impl Mode {
    /// A DEC private mode — the `?`-prefixed kind.
    #[must_use]
    pub const fn dec(number: u16) -> Self {
        Self(number & 0x7FFF)
    }

    /// A standard ANSI mode.
    #[must_use]
    pub const fn ansi(number: u16) -> Self {
        Self((number & 0x7FFF) | (1 << 15))
    }

    /// The mode number, without the ANSI flag.
    #[must_use]
    pub const fn number(self) -> u16 {
        self.0 & 0x7FFF
    }

    /// Whether this is an ANSI mode rather than a DEC private one.
    #[must_use]
    pub const fn is_ansi(self) -> bool {
        self.0 >> 15 != 0
    }

    /// DEC 2027, grapheme clustering.
    ///
    /// amx renders cells directly, so a cluster that spans cells is a cluster
    /// that renders wrong. The vendored tree patches this on by default (R7);
    /// the constant is here because callers still need to *observe* it.
    pub const GRAPHEME_CLUSTER: Self = Self::dec(2027);
    /// DEC 2026, synchronised output.
    pub const SYNCHRONIZED_OUTPUT: Self = Self::dec(2026);
    /// DEC 2004, bracketed paste.
    pub const BRACKETED_PASTE: Self = Self::dec(2004);
    /// DEC 1049, alternate screen with cursor save and clear.
    pub const ALTERNATE_SCREEN: Self = Self::dec(1049);
    /// DEC 25, cursor visibility (DECTCEM).
    pub const CURSOR_VISIBLE: Self = Self::dec(25);
    /// DEC 1, application cursor keys (DECCKM).
    pub const APPLICATION_CURSOR_KEYS: Self = Self::dec(1);
    /// DEC 2048, in-band size reports.
    pub const IN_BAND_RESIZE: Self = Self::dec(2048);
}

/// Where the cursor is and whether it should be drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CursorPosition {
    /// Column, zero-indexed.
    pub x: u16,
    /// Row within the active area, zero-indexed.
    pub y: u16,
    /// Whether DEC mode 25 has it visible.
    pub visible: bool,
    /// Whether the next printed character will soft-wrap.
    pub pending_wrap: bool,
}

/// An owned libghostty-vt terminal.
///
/// Not `Sync`, on purpose — see the module documentation. A shared reference
/// cannot leave the owning thread:
///
/// ```compile_fail
/// fn shareable<T: Sync>() {}
/// shareable::<amx_vt::Terminal>();
/// ```
#[derive(Debug)]
pub struct Terminal {
    raw: sys::GhosttyTerminal,
    /// Boxed so the address handed to `GHOSTTY_TERMINAL_OPT_USERDATA` outlives
    /// every callback, and so moving the `Terminal` does not move it.
    effects: Box<EffectState>,
    /// `Cell<()>` is `Send` and not `Sync`, which is exactly the bound 04 §3
    /// asks for. It is spelled out rather than left to the raw pointer field so
    /// that adding a manual `Sync` impl reads as the mistake it would be.
    _not_sync: PhantomData<std::cell::Cell<()>>,
}

// SAFETY: libghostty-vt requires that all operations on one terminal be
// serialized (terminal.h:1374); it does not require them to happen on the
// thread that created it. Moving the sole owner to the parser thread is
// therefore sound, and `!Sync` is what keeps "sole owner" true.
unsafe impl Send for Terminal {}

impl Terminal {
    /// Create a terminal.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidValue`] if `cols` or `rows` is zero,
    /// [`Error::OutOfMemory`] if the library cannot allocate.
    pub fn new(options: TerminalOptions) -> Result<Self> {
        Self::with_answers(options, QueryAnswers::default())
    }

    /// Create a terminal that answers identity queries with `answers`.
    ///
    /// # Errors
    ///
    /// As [`Terminal::new`].
    pub fn with_answers(options: TerminalOptions, answers: QueryAnswers) -> Result<Self> {
        let mut raw: sys::GhosttyTerminal = std::ptr::null_mut();
        let raw_options = sys::GhosttyTerminalOptions {
            cols: options.cols,
            rows: options.rows,
            max_scrollback: options.max_scrollback,
        };
        // SAFETY: `raw` is a live out-pointer; a NULL allocator selects the
        // library default, as terminal.h:1206 documents.
        check(unsafe { sys::ghostty_terminal_new(std::ptr::null(), &raw mut raw, raw_options) })?;
        if raw.is_null() {
            return Err(Error::InvalidValue);
        }

        let terminal = Self {
            raw,
            effects: EffectState::new(answers),
            _not_sync: PhantomData,
        };
        terminal.install_effects()?;
        Ok(terminal)
    }

    fn install_effects(&self) -> Result<()> {
        let userdata: *const c_void = std::ptr::from_ref(&*self.effects).cast();
        self.set_option(
            sys::GhosttyTerminalOption::GHOSTTY_TERMINAL_OPT_USERDATA,
            userdata,
        )?;
        for (option, trampoline) in callbacks::installers() {
            self.set_option(option, trampoline)?;
        }
        Ok(())
    }

    fn set_option(
        &self,
        option: sys::GhosttyTerminalOption::Type,
        value: *const c_void,
    ) -> Result<()> {
        // SAFETY: the terminal is live. terminal.h:1272 documents that pointer
        // typed options — userdata and every callback — take the value itself
        // rather than a pointer to it, which is what is passed here.
        check(unsafe { sys::ghostty_terminal_set(self.raw, option, value) })
    }

    /// The raw handle, for the other modules of this crate.
    pub(crate) fn as_raw(&self) -> sys::GhosttyTerminal {
        self.raw
    }

    /// Feed VT bytes to the parser, collecting everything they produce.
    ///
    /// Every effect callback fires synchronously inside this call
    /// (`terminal.h:77`), so when it returns, `effects` holds the complete,
    /// ordered set of replies and events for exactly these bytes. That is the
    /// property T05's PTY actor builds its reply ordering on: it appends to one
    /// buffer per pane and writes the buffer back in order.
    ///
    /// The parser never fails — malformed input is logged inside the library
    /// and cannot corrupt terminal state — so there is nothing to return.
    pub fn write(&mut self, bytes: &[u8], effects: &mut Effects) {
        let bound = self.effects.bind(effects);
        // SAFETY: the terminal is live and exclusively ours; `bytes` outlives
        // the call. The callbacks reached from here only push onto the bound
        // buffer, so the no-reentrancy rule cannot be broken from inside.
        unsafe { sys::ghostty_terminal_vt_write(self.raw, bytes.as_ptr(), bytes.len()) };
        drop(bound);
    }

    /// Resize the grid.
    ///
    /// The primary screen reflows if wraparound is on; the alternate screen
    /// does not. A resize also disables synchronised output and emits an
    /// in-band size report when DEC 2048 is set — both are the library's
    /// behaviour, not amx's (`terminal.h:1243`).
    ///
    /// # Errors
    ///
    /// [`Error::InvalidValue`] if either dimension is zero.
    pub fn resize(&mut self, cols: u16, rows: u16) -> Result<()> {
        self.resize_with_cell_size(cols, rows, 0, 0)
    }

    /// Resize, and tell the terminal how big a cell is in pixels.
    ///
    /// The pixel size only feeds size reports and image protocols; amx has no
    /// pixel notion of its own, so [`Terminal::resize`] passes zero.
    ///
    /// # Errors
    ///
    /// As [`Terminal::resize`].
    pub fn resize_with_cell_size(
        &mut self,
        cols: u16,
        rows: u16,
        cell_width_px: u32,
        cell_height_px: u32,
    ) -> Result<()> {
        // SAFETY: the terminal is live and exclusively ours.
        check(unsafe {
            sys::ghostty_terminal_resize(self.raw, cols, rows, cell_width_px, cell_height_px)
        })
    }

    /// Full reset (RIS): modes, scrollback, scrolling region and contents. The
    /// dimensions survive.
    pub fn reset(&mut self) {
        // SAFETY: the terminal is live and exclusively ours.
        unsafe { sys::ghostty_terminal_reset(self.raw) };
    }

    /// Read a terminal mode.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidValue`] if libghostty-vt does not know the mode.
    pub fn mode(&self, mode: Mode) -> Result<bool> {
        let mut value = false;
        // SAFETY: `value` is a live out-pointer of the documented type.
        check(unsafe { sys::ghostty_terminal_mode_get(self.raw, mode.0, &raw mut value) })?;
        Ok(value)
    }

    /// Set a terminal mode.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidValue`] if libghostty-vt does not know the mode.
    pub fn set_mode(&mut self, mode: Mode, value: bool) -> Result<()> {
        // SAFETY: the terminal is live and exclusively ours.
        check(unsafe { sys::ghostty_terminal_mode_set(self.raw, mode.0, value) })
    }

    /// Replace the answers given to identity queries.
    pub fn set_query_answers(&mut self, answers: QueryAnswers) {
        self.effects.set_answers(answers);
    }

    /// The answers currently given to identity queries.
    #[must_use]
    pub fn query_answers(&self) -> &QueryAnswers {
        self.effects.answers()
    }

    /// Width in cells.
    ///
    /// # Errors
    ///
    /// Propagates a library failure; the query itself cannot fail on a live
    /// terminal.
    pub fn cols(&self) -> Result<u16> {
        // SAFETY: the out-pointer type matches GHOSTTY_TERMINAL_DATA_COLS.
        unsafe { self.get(sys::GhosttyTerminalData::GHOSTTY_TERMINAL_DATA_COLS, 0u16) }
    }

    /// Height in cells.
    ///
    /// # Errors
    ///
    /// As [`Terminal::cols`].
    pub fn rows(&self) -> Result<u16> {
        // SAFETY: the out-pointer type matches GHOSTTY_TERMINAL_DATA_ROWS.
        unsafe { self.get(sys::GhosttyTerminalData::GHOSTTY_TERMINAL_DATA_ROWS, 0u16) }
    }

    /// Where the cursor is.
    ///
    /// # Errors
    ///
    /// As [`Terminal::cols`].
    pub fn cursor(&self) -> Result<CursorPosition> {
        // SAFETY: each out-pointer type matches the data kind requested.
        unsafe {
            Ok(CursorPosition {
                x: self.get(
                    sys::GhosttyTerminalData::GHOSTTY_TERMINAL_DATA_CURSOR_X,
                    0u16,
                )?,
                y: self.get(
                    sys::GhosttyTerminalData::GHOSTTY_TERMINAL_DATA_CURSOR_Y,
                    0u16,
                )?,
                visible: self.get(
                    sys::GhosttyTerminalData::GHOSTTY_TERMINAL_DATA_CURSOR_VISIBLE,
                    false,
                )?,
                pending_wrap: self.get(
                    sys::GhosttyTerminalData::GHOSTTY_TERMINAL_DATA_CURSOR_PENDING_WRAP,
                    false,
                )?,
            })
        }
    }

    /// Which screen buffer is active.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidValue`] if the library reports a screen this build does
    /// not know.
    pub fn active_screen(&self) -> Result<Screen> {
        // SAFETY: the out-pointer type matches GHOSTTY_TERMINAL_DATA_ACTIVE_SCREEN.
        let raw: sys::GhosttyTerminalScreen::Type = unsafe {
            self.get(
                sys::GhosttyTerminalData::GHOSTTY_TERMINAL_DATA_ACTIVE_SCREEN,
                0,
            )?
        };
        Screen::from_sys(raw).ok_or(Error::InvalidValue)
    }

    /// Whether any mouse tracking mode is on.
    ///
    /// # Errors
    ///
    /// As [`Terminal::cols`].
    pub fn mouse_tracking(&self) -> Result<bool> {
        // SAFETY: the out-pointer type matches GHOSTTY_TERMINAL_DATA_MOUSE_TRACKING.
        unsafe {
            self.get(
                sys::GhosttyTerminalData::GHOSTTY_TERMINAL_DATA_MOUSE_TRACKING,
                false,
            )
        }
    }

    /// The Kitty keyboard protocol flags the application has asked for.
    ///
    /// # Errors
    ///
    /// As [`Terminal::cols`].
    pub fn kitty_keyboard_flags(&self) -> Result<crate::key::KittyKeyFlags> {
        // SAFETY: the out-pointer type matches
        // GHOSTTY_TERMINAL_DATA_KITTY_KEYBOARD_FLAGS, documented as uint8_t*.
        let bits: sys::GhosttyKittyKeyFlags = unsafe {
            self.get(
                sys::GhosttyTerminalData::GHOSTTY_TERMINAL_DATA_KITTY_KEYBOARD_FLAGS,
                0,
            )?
        };
        Ok(crate::key::KittyKeyFlags::from_bits(bits))
    }

    /// The title set by OSC 0 or OSC 2, if any.
    ///
    /// Copied out: the library's pointer is only valid until the next write or
    /// reset (`terminal.h:1002`), and a caller that has to remember that is a
    /// caller that will eventually forget.
    ///
    /// # Errors
    ///
    /// As [`Terminal::cols`].
    pub fn title(&self) -> Result<Option<String>> {
        self.string(sys::GhosttyTerminalData::GHOSTTY_TERMINAL_DATA_TITLE)
    }

    /// The working directory reported by OSC 7, OSC 9 or OSC 1337, if any.
    ///
    /// The bytes are whatever the shell emitted, unparsed — a `file://` URI for
    /// OSC 7, usually a bare path for the others.
    ///
    /// # Errors
    ///
    /// As [`Terminal::cols`].
    pub fn pwd(&self) -> Result<Option<String>> {
        self.string(sys::GhosttyTerminalData::GHOSTTY_TERMINAL_DATA_PWD)
    }

    fn string(&self, data: sys::GhosttyTerminalData::Type) -> Result<Option<String>> {
        let mut string = sys::GhosttyString {
            ptr: std::ptr::null(),
            len: 0,
        };
        // SAFETY: the out-pointer type matches every GhosttyString data kind.
        let present = check_optional(unsafe {
            sys::ghostty_terminal_get(self.raw, data, (&raw mut string).cast())
        })?;
        if !present || string.ptr.is_null() || string.len == 0 {
            return Ok(None);
        }
        // SAFETY: the header documents the pointer as valid until the next
        // `vt_write` or `reset`; it is copied before this function returns and
        // `&self` means neither can have happened in between.
        let bytes = unsafe { std::slice::from_raw_parts(string.ptr, string.len) };
        Ok(Some(String::from_utf8_lossy(bytes).into_owned()))
    }

    /// Read one POD value out of the terminal.
    ///
    /// # Safety
    ///
    /// `T` must be the output type `terminal.h` documents for `data`.
    pub(crate) unsafe fn get<T>(
        &self,
        data: sys::GhosttyTerminalData::Type,
        initial: T,
    ) -> Result<T> {
        let mut value = initial;
        // SAFETY: the caller guarantees the type; `value` is a live
        // out-pointer for the whole call.
        check(unsafe { sys::ghostty_terminal_get(self.raw, data, (&raw mut value).cast()) })?;
        Ok(value)
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        // SAFETY: the handle came from a successful `ghostty_terminal_new` and
        // is freed exactly once. The boxed effect state outlives this call and
        // is dropped after it, so no callback can be reached with a dangling
        // userdata pointer.
        unsafe { sys::ghostty_terminal_free(self.raw) };
    }
}
