//! Raw mode, alt screen and window size — `rustix::termios` only (D-M0-4).
//!
//! [`TerminalGuard`] is the one seam that owns leaving the terminal the way it
//! found it. Entering saves the caller's `termios` and switches to raw mode
//! plus the alternate screen; restoring is idempotent and runs from three
//! places: the guard's own `Drop` (normal exit, and unwinding through a
//! panic — Rust always runs destructors while unwinding), and an explicit
//! `restore()` call from [`crate::app`]'s `SIGTERM` arm before it returns,
//! since a signal is not unwinding and nothing drops on its own until control
//! actually leaves the scope that owns the guard.
//!
//! Mouse tracking rides the same seam and is off unless asked for
//! ([`TerminalGuard::request_mouse`]). "The way it found it" is meant
//! literally there: the guard resets the two modes it set and no others, so a
//! mode the terminal had already chosen — `?1007`, alternate scroll, set by
//! default in both terminals X01 measured — survives amx rather than being
//! cleared by a client tidying up after itself
//! (`docs/notes/m4-mouse-path.md` F-4).

use std::fmt;
use std::io::{self, Write};
use std::os::fd::AsFd;

use rustix::termios::{self, OptionalActions, Termios};
use thiserror::Error;
use tokio::signal::unix::{Signal, SignalKind};

/// Bytes that enter the alternate screen and hide the cursor.
pub const ALT_SCREEN_ENTER: &[u8] = b"\x1b[?1049h\x1b[?25l";

/// Bytes that show the cursor and leave the alternate screen.
///
/// The exact reverse of [`ALT_SCREEN_ENTER`], written in the opposite order:
/// the cursor is shown before the screen it was hidden on goes away.
pub const ALT_SCREEN_LEAVE: &[u8] = b"\x1b[?25h\x1b[?1049l";

/// Bytes that ask the host terminal to report mouse events (D14, off by
/// default — see [`TerminalGuard::request_mouse`]).
///
/// Exactly these two modes, in this order, and no others. `?1006` selects the
/// SGR encoding `crate::input`'s scanner recognises and `?1000` asks for press
/// and release; that pair, `1006` first, is what every installed terminal's own
/// terminfo entry names as the enable string (`XM=\E[?1006;1000%?…`, X01 §2.1).
///
/// **Never `?1002`.** It is motion-while-pressed, which amx has no use for, and
/// a terminal's mouse event mode is one enum rather than a set of flags — X01
/// asked for `1000` and `1002` together and watched `1000` come back *reset*
/// (§2.2, and `vendor/libghostty-vt/src/terminal/mouse.zig:7-13` spells the
/// same exclusivity out).
///
/// **Never `?1007`.** Alternate scroll is *already set* in both terminals X01
/// measured, before amx asks for anything, and a client that resets every mode
/// it wrote would clear a mode the user's terminal had chosen for itself (X01
/// F-4). This pair is the whole of what amx touches, in both directions, which
/// is what makes "leaving amx leaves the terminal as it was found" true rather
/// than aspirational.
pub const MOUSE_TRACK_ENTER: &[u8] = b"\x1b[?1006h\x1b[?1000h";

/// Bytes that stop mouse reporting.
///
/// The reverse of [`MOUSE_TRACK_ENTER`] in the opposite order: tracking stops
/// before the encoding it was reporting in goes away, so no report can be
/// emitted in an encoding nothing is still asking for.
pub const MOUSE_TRACK_LEAVE: &[u8] = b"\x1b[?1000l\x1b[?1006l";

/// A terminal operation on the controlling tty failed.
#[derive(Debug, Error)]
pub enum TermError {
    /// Reading the current terminal attributes failed.
    #[error("read terminal attributes: {0}")]
    Get(#[source] io::Error),
    /// Writing new terminal attributes failed.
    #[error("set terminal attributes: {0}")]
    Set(#[source] io::Error),
    /// Reading the terminal's window size failed.
    #[error("read terminal size: {0}")]
    Size(#[source] io::Error),
    /// Installing the `SIGWINCH` handler failed.
    #[error("install SIGWINCH handler: {0}")]
    Signal(#[source] io::Error),
}

/// A terminal size in cells.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TermSize {
    /// Rows.
    pub rows: u16,
    /// Columns.
    pub cols: u16,
}

impl From<termios::Winsize> for TermSize {
    fn from(ws: termios::Winsize) -> Self {
        Self {
            rows: ws.ws_row,
            cols: ws.ws_col,
        }
    }
}

/// The current window size of `fd`.
pub fn window_size<Fd: AsFd>(fd: Fd) -> Result<TermSize, TermError> {
    termios::tcgetwinsize(fd.as_fd())
        .map(TermSize::from)
        .map_err(|e| TermError::Size(e.into()))
}

/// A stream of `SIGWINCH` notifications.
///
/// Thin wrapper so callers depend on this module rather than reaching for
/// `tokio::signal::unix` directly — the D-M0-4 rationale for using it at all
/// (no crossterm) lives here, next to the one place it is installed.
pub struct Sigwinch(Signal);

impl Sigwinch {
    /// Install the handler. Must run inside a tokio runtime.
    pub fn install() -> Result<Self, TermError> {
        tokio::signal::unix::signal(SignalKind::window_change())
            .map(Self)
            .map_err(TermError::Signal)
    }

    /// Wait for the next `SIGWINCH`.
    ///
    /// `None` only if the signal stream itself closed, which does not happen
    /// in practice on Unix: callers may treat it as "wait forever" without a
    /// retry loop of their own.
    pub async fn recv(&mut self) -> Option<()> {
        self.0.recv().await
    }
}

impl fmt::Debug for Sigwinch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Sigwinch").finish_non_exhaustive()
    }
}

/// Owns the terminal's raw mode and alternate-screen state.
///
/// `Fd` is the controlling terminal (normally the process's own stdin) and
/// `W` is where the alt-screen control sequences are written (normally
/// stdout); both are generic so a test can substitute a real pty and a
/// `Vec<u8>` instead of the process's own streams.
pub struct TerminalGuard<Fd: AsFd, W: Write> {
    fd: Fd,
    out: W,
    saved: Termios,
    active: bool,
    /// Whether this guard asked for mouse tracking, and therefore owes the
    /// terminal the reset. Nothing else is tracked because nothing else is
    /// touched: restoring only what was actually set is the whole of X01's F-4.
    mouse: bool,
}

impl<Fd: AsFd, W: Write> TerminalGuard<Fd, W> {
    /// Save `fd`'s current attributes, switch it to raw mode, and write the
    /// alt-screen-enter sequence to `out`.
    pub fn enter(fd: Fd, mut out: W) -> Result<Self, TermError> {
        let saved = termios::tcgetattr(fd.as_fd()).map_err(|e| TermError::Get(e.into()))?;
        let mut raw = saved.clone();
        raw.make_raw();
        termios::tcsetattr(fd.as_fd(), OptionalActions::Flush, &raw)
            .map_err(|e| TermError::Set(e.into()))?;
        let _ = out.write_all(ALT_SCREEN_ENTER);
        let _ = out.flush();
        Ok(Self {
            fd,
            out,
            saved,
            active: true,
            mouse: false,
        })
    }

    /// The terminal's current size.
    pub fn size(&self) -> Result<TermSize, TermError> {
        window_size(self.fd.as_fd())
    }

    /// Ask the host terminal for SGR mouse reporting (D14's wheel exception,
    /// and D9's forwarding).
    ///
    /// **Not called unless the user turned it on.** X01's outcome (b): asking
    /// costs the user their terminal's own selection — with an application
    /// holding the mouse, an ordinary drag-select becomes shift-drag in both
    /// terminals the spike measured, documented in each one's manual
    /// (foot's `selection-override-modifiers`, alacritty's `Shift`
    /// suppression) — and the cost is paid all the time rather than where it
    /// buys something, because the wheel exception exists precisely for panes
    /// that did *not* ask for the mouse and so cannot be scoped to the panes
    /// that did. `[client] mouse` therefore defaults off and the phone profile
    /// is where it is turned on: the people who need touch-scroll are the
    /// people not selecting text with a mouse.
    ///
    /// Idempotent, and paired with [`Self::restore`] on all three paths the
    /// guard already covers — `Drop`, a panic unwind, and the explicit
    /// `restore()` the `SIGTERM` arm makes — which is what keeps the release
    /// a property of one seam instead of four call sites.
    pub fn request_mouse(&mut self) {
        if self.mouse || !self.active {
            return;
        }
        let _ = self.out.write_all(MOUSE_TRACK_ENTER);
        let _ = self.out.flush();
        self.mouse = true;
    }

    /// Whether this guard is currently holding the terminal's mouse.
    #[must_use]
    pub const fn mouse_requested(&self) -> bool {
        self.mouse
    }

    /// Leave the alternate screen and restore the saved attributes.
    ///
    /// Idempotent: a second call, including the one every `Drop` makes, is a
    /// no-op. Errors are swallowed here on purpose — a restore that runs
    /// during unwinding or from a signal arm has nowhere left to report a
    /// failure to, and leaving the terminal half-restored would be worse than
    /// leaving it unrestored and silent.
    pub fn restore(&mut self) {
        if !self.active {
            return;
        }
        // Before the alt screen goes, and only if this guard asked: the reverse
        // of the order they were written in, and nothing the guard did not set.
        if self.mouse {
            let _ = self.out.write_all(MOUSE_TRACK_LEAVE);
            self.mouse = false;
        }
        let _ = self.out.write_all(ALT_SCREEN_LEAVE);
        let _ = self.out.flush();
        let _ = termios::tcsetattr(self.fd.as_fd(), OptionalActions::Flush, &self.saved);
        self.active = false;
    }
}

impl<Fd: AsFd, W: Write> Drop for TerminalGuard<Fd, W> {
    fn drop(&mut self) {
        self.restore();
    }
}

impl<Fd: AsFd, W: Write> fmt::Debug for TerminalGuard<Fd, W> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TerminalGuard")
            .field("active", &self.active)
            .field("mouse", &self.mouse)
            .finish_non_exhaustive()
    }
}
