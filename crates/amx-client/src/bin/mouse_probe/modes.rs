//! Asking: the DEC private-mode sequences the probe writes, and the terminal
//! guard that puts them back.
//!
//! Split from `main.rs` because "what amx would write to the host terminal" is
//! the part of this probe X13 copies most directly, and it should be readable
//! on its own with its ordering rationale attached.

use std::fmt::Write as _;
use std::io::Write as _;
use std::os::fd::AsFd as _;

/// The bytes that turn `modes` on, in the order given.
pub fn enable_bytes(modes: &[u16]) -> Vec<u8> {
    let mut out = Vec::new();
    for mode in modes {
        write!(&mut ByteWriter(&mut out), "\x1b[?{mode}h").expect("write to a Vec");
    }
    out
}

/// The bytes that turn `modes` off, in reverse order.
///
/// Reverse on purpose, and it matters: `1006` selects the *encoding* the
/// tracking modes report in, so a terminal is left in a coherent state if the
/// tracking goes off before the encoding it was reporting in does.
pub fn disable_bytes(modes: &[u16]) -> Vec<u8> {
    let mut out = Vec::new();
    for mode in modes.iter().rev() {
        write!(&mut ByteWriter(&mut out), "\x1b[?{mode}l").expect("write to a Vec");
    }
    out
}

/// The bytes that ask the terminal to report each mode's state (DECRQM).
pub fn query_bytes(modes: &[u16]) -> Vec<u8> {
    let mut out = Vec::new();
    for mode in modes {
        write!(&mut ByteWriter(&mut out), "\x1b[?{mode}$p").expect("write to a Vec");
    }
    out
}

/// `fmt::Write` over a byte vector, so the sequences above are built with
/// `write!` and no intermediate `String`.
struct ByteWriter<'a>(&'a mut Vec<u8>);

impl std::fmt::Write for ByteWriter<'_> {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        self.0.extend_from_slice(s.as_bytes());
        Ok(())
    }
}

/// Raw mode, and optionally the alternate screen, restored on every path.
///
/// `TerminalGuard` couples raw mode to the alternate screen, and a probe whose
/// transcript disappears when the alt screen is left is a probe nobody can
/// read. This is the same save-and-restore over `rustix::termios` with the
/// screen switch made optional.
pub struct Guard<'a> {
    fd: &'a std::io::Stdin,
    saved: rustix::termios::Termios,
    alt: bool,
    active: bool,
}

impl<'a> Guard<'a> {
    /// Save the terminal's attributes and switch it to raw mode.
    pub fn enter(fd: &'a std::io::Stdin, alt: bool) -> Self {
        let saved = rustix::termios::tcgetattr(fd.as_fd()).expect("read terminal attributes");
        let mut raw = saved.clone();
        raw.make_raw();
        rustix::termios::tcsetattr(fd.as_fd(), rustix::termios::OptionalActions::Flush, &raw)
            .expect("enter raw mode");
        if alt {
            let mut out = std::io::stdout();
            let _ = out.write_all(amx_client::term::ALT_SCREEN_ENTER);
            let _ = out.flush();
        }
        Self {
            fd,
            saved,
            alt,
            active: true,
        }
    }

    /// Put the terminal back. Idempotent, like the guard it mirrors.
    pub fn restore(&mut self) {
        if !self.active {
            return;
        }
        if self.alt {
            let mut out = std::io::stdout();
            let _ = out.write_all(amx_client::term::ALT_SCREEN_LEAVE);
            let _ = out.flush();
        }
        let _ = rustix::termios::tcsetattr(
            self.fd.as_fd(),
            rustix::termios::OptionalActions::Flush,
            &self.saved,
        );
        self.active = false;
    }
}

impl Drop for Guard<'_> {
    fn drop(&mut self) {
        self.restore();
    }
}

#[cfg(test)]
mod tests {
    use super::{disable_bytes, enable_bytes, query_bytes};

    #[test]
    fn modes_are_set_in_order_and_reset_in_reverse() {
        assert_eq!(enable_bytes(&[1000, 1006]), b"\x1b[?1000h\x1b[?1006h");
        assert_eq!(disable_bytes(&[1000, 1006]), b"\x1b[?1006l\x1b[?1000l");
        assert_eq!(query_bytes(&[1006]), b"\x1b[?1006$p");
    }

    #[test]
    fn an_empty_mode_list_writes_nothing_at_all() {
        // The baseline run: what `amx attach` asks its host terminal for
        // today, which is nothing.
        assert!(enable_bytes(&[]).is_empty());
        assert!(disable_bytes(&[]).is_empty());
        assert!(query_bytes(&[]).is_empty());
    }
}
