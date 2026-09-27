//! Whether the terminal amx is drawn on is a light one or a dark one.
//!
//! One question, asked once, so that `theme = "auto"` can name the palette
//! that will be legible rather than the one somebody happened to configure on
//! another machine. It exists because of a real screen: amx over ssh from a
//! phone in light mode, painted in the dark palette, with the row under the
//! cursor unreadable — an off-black bar under the terminal's own near-black
//! text.
//!
//! Two answers are asked for, in the order of how much they know:
//!
//! 1. **The terminal itself**, through the escape xterm answers its background
//!    colour with. It is the only source that is about the terminal in front of
//!    the person rather than about something they once set, and every terminal
//!    that does not know the escape says nothing rather than something wrong.
//! 2. **`COLORFGBG`**, which a handful of terminals export and the rest do not.
//!    It is a fact about the shell's environment and may be a machine or two
//!    stale over ssh, so it answers only where the terminal would not.
//!
//! And where neither says anything, dark — which is what amx painted for
//! everybody before this file, and the background nearly every terminal opens
//! on.
//!
//! Nothing here runs unless somebody asked for it. A theme named by hand is a
//! decision already made, and this is not amx overruling it: see
//! [`crate::theme::AUTO`].

use std::io::{Read, Write};
use std::os::fd::{AsFd, BorrowedFd};
use std::time::{Duration, Instant};

/// What the terminal's background is, as far as anything could tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shade {
    Light,
    Dark,
}

/// The escape that asks a terminal what its background colour is: xterm's
/// `OSC 11 ; ? ST`, which every terminal amx has been drawn on either answers
/// or ignores.
const ASK: &str = "\x1b]11;?\x1b\\";

/// How long an answer is waited for.
///
/// Long enough for a terminal at the far end of an ssh session on a phone, and
/// short enough that a terminal which will never answer costs a fifth of a
/// second once, at the moment the view is already clearing the screen. The
/// wait ends the instant an answer lands, so the terminals that do answer pay
/// none of it.
///
/// It is also how long somebody's own typing would be read by this rather than
/// by the view, which is why the wait ends on the first byte that cannot be an
/// answer as well: whatever a terminal sends back opens with an escape, and a
/// letter arriving instead is a person at the keyboard and not a reply.
const PATIENCE: Duration = Duration::from_millis(200);

/// What one read waits, in tenths of a second, which is the unit a terminal
/// measures `VTIME` in. Shorter than [`PATIENCE`] so the loop gets more than
/// one look before it gives up.
const TICK: u8 = 1;

/// Whether this terminal is a light one, asking it and then its environment.
///
/// Reads from the terminal, so it is called once, from the one place that has
/// already taken the terminal into raw mode and has not yet started reading
/// keys off it — see [`crate::tui::run`]. Anywhere else, the answer lands in
/// the middle of somebody's typing.
pub fn of_the_terminal() -> Shade {
    asked()
        .as_deref()
        .and_then(said)
        .or_else(|| std::env::var("COLORFGBG").ok().as_deref().and_then(told))
        .unwrap_or(Shade::Dark)
}

/// The terminal's own answer, as the bytes it sent back.
///
/// `None` from a terminal that did not answer, one that is not a terminal at
/// all, and one whose settings could not be read or put back — the last
/// because a shade is not worth a terminal left in a state amx changed.
fn asked() -> Option<String> {
    let input = std::io::stdin();
    let fd = input.as_fd();
    if !std::io::IsTerminal::is_terminal(&input) {
        return None;
    }
    // Keys already waiting are somebody who started typing before the view
    // was up. Asking now would read them as the answer and lose them, so the
    // terminal goes unasked and the keys are left for the view.
    if pending(fd) {
        return None;
    }

    // A read that comes back empty rather than waiting for a key. Raw mode
    // leaves stdin blocking on one byte, and a terminal that never answers
    // would hold the view at a blank screen until somebody pressed something.
    let settled = nix::sys::termios::tcgetattr(fd).ok()?;
    let mut timed = settled.clone();
    timed.control_chars[nix::sys::termios::SpecialCharacterIndices::VMIN as usize] = 0;
    timed.control_chars[nix::sys::termios::SpecialCharacterIndices::VTIME as usize] = TICK;
    set(fd, &timed)?;

    print!("{ASK}");
    let answer = std::io::stdout()
        .flush()
        .ok()
        .and_then(|()| listen(&mut std::io::stdin().lock()));
    set(fd, &settled)?;
    answer
}

/// Whether anything is waiting to be read, without reading it.
fn pending(fd: BorrowedFd<'_>) -> bool {
    let mut waiting = nix::libc::pollfd {
        fd: std::os::fd::AsRawFd::as_raw_fd(&fd),
        events: nix::libc::POLLIN,
        revents: 0,
    };
    // SAFETY: one pollfd, owned here, and a timeout of nothing.
    let ready = unsafe { nix::libc::poll(&mut waiting, 1, 0) };
    ready > 0 && waiting.revents & nix::libc::POLLIN != 0
}

/// Put the terminal's settings where they are asked for, saying whether it
/// took.
fn set(fd: BorrowedFd<'_>, how: &nix::sys::termios::Termios) -> Option<()> {
    nix::sys::termios::tcsetattr(fd, nix::sys::termios::SetArg::TCSANOW, how).ok()
}

/// Read until the answer is whole or the patience runs out.
///
/// Whole is the terminator, which is the one thing that says the rest of the
/// answer is not still arriving: a colour is sent in one write by every
/// terminal measured, and an answer cut in half reads as a darker colour than
/// it is. Either terminator, because the two spellings are the same escape —
/// see [`crate::ansi`], which reads them the same way.
fn listen(input: &mut impl Read) -> Option<String> {
    let deadline = Instant::now() + PATIENCE;
    let mut heard = Vec::new();
    while Instant::now() < deadline {
        let mut bytes = [0u8; 64];
        match input.read(&mut bytes) {
            Ok(0) => continue,
            Ok(n) => heard.extend_from_slice(&bytes[..n]),
            Err(_) => return None,
        }
        // Whatever a terminal sends back opens with an escape. Anything else
        // is somebody at the keyboard, and going on reading would be this
        // eating the keys they pressed while the view was still opening.
        if heard.first() != Some(&b'\x1b') {
            break;
        }
        if heard.contains(&b'\x07') || heard.windows(2).any(|pair| pair == b"\x1b\\") {
            break;
        }
    }
    Some(String::from_utf8_lossy(&heard).into_owned())
}

/// What a terminal's answer says its background is.
///
/// The colour is written `rgb:` and then the three channels in hex, separated
/// by slashes, each of them one to four digits wide — a terminal answering in
/// 16 bits a channel and one answering in 8 are saying the same colour, so
/// what is read is the top eight bits of whatever width arrived.
///
/// Anything else is nothing rather than a guess. An answer amx cannot read is
/// a terminal amx has not measured, and painting a light palette onto a dark
/// screen is worse than painting the one everybody had before.
pub fn said(answer: &str) -> Option<Shade> {
    let channels: Vec<&str> = answer
        .split_once("rgb:")?
        .1
        .split(|c: char| !c.is_ascii_hexdigit() && c != '/')
        .next()?
        .split('/')
        .collect();
    let [red, green, blue] = channels.as_slice() else {
        return None;
    };
    Some(shade_of(top(red)?, top(green)?, top(blue)?))
}

/// The top eight bits of one channel, however many digits it was written in.
///
/// One digit is the odd one: `f` means the whole of the channel rather than
/// the bottom sixteenth of it, so it is repeated into both nibbles the way X's
/// own parser does, and `f` comes out 255 rather than 240.
fn top(digits: &str) -> Option<u32> {
    let value = u32::from_str_radix(digits, 16).ok()?;
    match digits.len() {
        1 => Some(value * 0x11),
        2 => Some(value),
        3 => Some(value >> 4),
        4 => Some(value >> 8),
        _ => None,
    }
}

/// What `COLORFGBG` says the background is.
///
/// Two or three fields, the background last: `0;15` is white behind black and
/// `15;0` the other way about, and rxvt writes `0;default;15` with the cursor
/// between them. The value is an ANSI index, and the light half of the sixteen
/// is 7 and 9 through 15 — 8 is the bright black a dark scheme is built on and
/// belongs with the dark ones.
pub fn told(said: &str) -> Option<Shade> {
    let background: u8 = said.rsplit(';').next()?.trim().parse().ok()?;
    match background {
        7 | 9..=15 => Some(Shade::Light),
        0..=6 | 8 => Some(Shade::Dark),
        _ => None,
    }
}

/// Whether a colour is one to paint dark words on.
///
/// Rec. 709 luminance, which is the weighting that says green carries most of
/// what an eye reads as brightness and blue almost none of it. Halfway is the
/// cut: there is no third answer, and a terminal sitting exactly on the line is
/// one either palette reads on.
fn shade_of(red: u32, green: u32, blue: u32) -> Shade {
    let light = 2126 * red + 7152 * green + 722 * blue;
    match light >= 10_000 * 128 {
        true => Shade::Light,
        false => Shade::Dark,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_terminal_answers_its_background_in_whatever_width_it_keeps_it() {
        // The same white in the three widths terminals answer in, and the same
        // answer from each: what is read is the top eight bits.
        for answer in [
            "\x1b]11;rgb:ffff/ffff/ffff\x1b\\",
            "\x1b]11;rgb:ff/ff/ff\x07",
            "\x1b]11;rgb:fff/fff/fff\x1b\\",
            "\x1b]11;rgb:f/f/f\x07",
        ] {
            assert_eq!(said(answer), Some(Shade::Light), "{answer:?}");
        }
        for answer in [
            "\x1b]11;rgb:1e1e/1e1e/1e1e\x1b\\",
            "\x1b]11;rgb:00/00/00\x07",
        ] {
            assert_eq!(said(answer), Some(Shade::Dark), "{answer:?}");
        }
    }

    #[test]
    fn a_green_terminal_is_lighter_than_a_blue_one_of_the_same_numbers() {
        // Rec. 709, which is the whole reason the three channels are not
        // averaged: green carries most of what an eye reads as brightness.
        assert_eq!(said("\x1b]11;rgb:00/c0/00\x07"), Some(Shade::Light));
        assert_eq!(said("\x1b]11;rgb:00/00/c0\x07"), Some(Shade::Dark));
    }

    #[test]
    fn an_answer_nothing_can_read_is_nothing_rather_than_a_guess() {
        // Painting a light palette onto a dark screen is worse than painting
        // the one everybody had before this file.
        for answer in ["", "\x1b]11;?\x1b\\", "\x1b]11;rgb:ff/ff\x07", "ok"] {
            assert_eq!(said(answer), None, "{answer:?}");
        }
    }

    #[test]
    fn colorfgbg_is_read_off_its_last_field_whichever_shape_it_is_in() {
        assert_eq!(told("0;15"), Some(Shade::Light));
        assert_eq!(told("15;0"), Some(Shade::Dark));
        assert_eq!(told("0;default;15"), Some(Shade::Light), "rxvt's three");
        assert_eq!(
            told("15;8"),
            Some(Shade::Dark),
            "8 is the bright black a dark scheme is built on"
        );
        assert_eq!(told("0;7"), Some(Shade::Light));
        for nothing in ["", "default", "0;", "0;256"] {
            assert_eq!(told(nothing), None, "{nothing:?}");
        }
    }

    #[test]
    fn the_answer_is_read_to_its_terminator_and_no_further() {
        // A colour arrives in one write from every terminal measured, but the
        // read stops on the terminator either way: an answer cut in half reads
        // as a darker colour than it is.
        let mut sent: &[u8] = b"\x1b]11;rgb:ffff/ffff/ffff\x07";
        let heard = listen(&mut sent).expect("the answer");
        assert_eq!(said(&heard), Some(Shade::Light));
    }

    #[test]
    fn a_key_pressed_while_the_view_opens_ends_the_wait_rather_than_feeding_it() {
        // The whole of what this costs somebody is the window between the ask
        // and the answer, and a letter arriving in it is not a terminal
        // replying. It ends the wait on the spot.
        let mut typing: &[u8] = b"j";
        let began = Instant::now();
        let heard = listen(&mut typing).expect("the letter");
        assert!(began.elapsed() < PATIENCE, "and does not wait out the rest");
        assert_eq!(said(&heard), None);
    }

    #[test]
    fn a_key_already_waiting_is_left_for_the_view() {
        // Somebody typing before the view is up: asking then would read their
        // keys as the answer, so the terminal is not asked and they stay put.
        let pty = nix::pty::openpty(None, None).expect("a pty");
        // Raw, as the view has it by the time it asks.
        let mut raw = nix::sys::termios::tcgetattr(&pty.slave).expect("its settings");
        nix::sys::termios::cfmakeraw(&mut raw);
        set(pty.slave.as_fd(), &raw).expect("raw");
        assert!(!pending(pty.slave.as_fd()), "nothing typed yet");
        nix::unistd::write(&pty.master, b"j").expect("a key");
        let began = Instant::now();
        while !pending(pty.slave.as_fd()) {
            assert!(began.elapsed() < Duration::from_secs(1), "the key arrives");
            std::thread::sleep(Duration::from_millis(5));
        }
        let mut kept = [0u8; 1];
        nix::unistd::read(&pty.slave, &mut kept).expect("the key, still there");
        assert_eq!(&kept, b"j");
    }

    #[test]
    fn a_terminal_that_says_nothing_leaves_everything_as_it_was() {
        // The wait is what it costs, and the answer is the dark everybody had.
        let mut silence: &[u8] = b"";
        let heard = listen(&mut silence).expect("the wait, and nothing in it");
        assert_eq!(said(&heard), None);
    }
}
