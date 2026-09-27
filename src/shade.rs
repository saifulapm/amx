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
//! The terminal is asked whatever the theme, because the colour it answers
//! with is kept too — see [`remember`] — for the panes amx starts, which sit in
//! detached tmux sessions no terminal answers. The shade still picks a palette
//! only under `auto`: a theme named by hand is a decision already made, and
//! this is not amx overruling it. See [`crate::theme::AUTO`].

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::io::{Read, Write};
use std::os::fd::{AsFd, BorrowedFd};
use std::path::Path;
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

/// Whether this terminal is a light one, by what it answered [`asked`] with
/// and then by its environment.
pub fn of_the_answer(answer: Option<&str>) -> Shade {
    answer
        .and_then(said)
        .or_else(|| std::env::var("COLORFGBG").ok().as_deref().and_then(told))
        .unwrap_or(Shade::Dark)
}

/// The terminal's own answer, as the bytes it sent back.
///
/// Reads from the terminal, so it is called once, from the one place that has
/// already taken the terminal into raw mode and has not yet started reading
/// keys off it — see [`crate::tui::run`]. Anywhere else, the answer lands in
/// the middle of somebody's typing.
///
/// `None` from a terminal that did not answer, one that is not a terminal at
/// all, and one whose settings could not be read or put back — the last
/// because a shade is not worth a terminal left in a state amx changed.
pub fn asked() -> Option<String> {
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
pub fn said(answer: &str) -> Option<Shade> {
    let (red, green, blue) = background_of(answer)?;
    Some(shade_of(red.into(), green.into(), blue.into()))
}

/// The colour a terminal's answer names, a byte a channel.
///
/// The colour is written `rgb:` and then the three channels in hex, separated
/// by slashes, each of them one to four digits wide — a terminal answering in
/// 16 bits a channel and one answering in 8 are saying the same colour, so
/// what is read is the top eight bits of whatever width arrived.
///
/// Anything else is nothing rather than a guess. An answer amx cannot read is
/// a terminal amx has not measured, and painting a light palette onto a dark
/// screen is worse than painting the one everybody had before.
pub fn background_of(answer: &str) -> Option<(u8, u8, u8)> {
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
    Some((top(red)?, top(green)?, top(blue)?))
}

/// The top eight bits of one channel, however many digits it was written in.
///
/// One digit is the odd one: `f` means the whole of the channel rather than
/// the bottom sixteenth of it, so it is repeated into both nibbles the way X's
/// own parser does, and `f` comes out 255 rather than 240.
fn top(digits: &str) -> Option<u8> {
    let value = u32::from_str_radix(digits, 16).ok()?;
    let top = match digits.len() {
        1 => value * 0x11,
        2 => value,
        3 => value >> 4,
        4 => value >> 8,
        _ => return None,
    };
    u8::try_from(top).ok()
}

/// Keep a colour the terminal answered with, for the panes amx starts.
///
/// Written whole or not at all: a spawn reading it mid-write would paint a
/// pane with half a colour.
pub fn remember(state_root: &Path, colour: (u8, u8, u8)) -> Result<()> {
    let (red, green, blue) = colour;
    crate::store::write_atomic(
        &crate::paths::background_file(state_root),
        format!("#{red:02x}{green:02x}{blue:02x}\n").as_bytes(),
    )
}

/// The colour last kept, as `#rrggbb`, or nothing where the file is missing
/// or holds anything but one colour.
#[allow(dead_code, reason = "read by spawn once panes wear it")]
pub fn remembered(state_root: &Path) -> Option<String> {
    let kept = std::fs::read_to_string(crate::paths::background_file(state_root)).ok()?;
    let kept = kept.trim();
    let hex = kept.strip_prefix('#')?;
    (hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit())).then(|| kept.to_string())
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

/// The terminal's answer arriving after the wait, as the key loop reads it.
///
/// A terminal slower than [`PATIENCE`] still answers, and by then the view is
/// reading keys: the escape that opens the answer comes through as alt and
/// `]`, the rest as a key a character, and `:` and `/` among them would open a
/// line on the list. This holds on to a run of keys for as long as it could
/// still be an answer, drops it whole when it is one, and hands it back in
/// order when it turns out to be somebody typing.
#[derive(Default)]
pub struct Late {
    held: Vec<KeyEvent>,
}

/// What an answer spells between the escape that opens it and its colour.
const SPELT: &str = "11;rgb:";

impl Late {
    /// The keys to act on now that this one has arrived, which is none while
    /// a run is held and none when the run was an answer.
    pub fn hear(&mut self, key: KeyEvent) -> Vec<KeyEvent> {
        if self.held.is_empty() {
            return match opens(&key) {
                true => {
                    self.held.push(key);
                    Vec::new()
                }
                false => vec![key],
            };
        }
        let spelt = self.spelt();
        if closes(&key) {
            // `said` reads the colour the way it reads one from the wait.
            return match said(&format!("\x1b]{spelt}\x07")) {
                Some(_) => {
                    self.held.clear();
                    Vec::new()
                }
                None => self.with(key),
            };
        }
        match plain(&key) {
            Some(c) if could_be(&format!("{spelt}{c}")) => {
                self.held.push(key);
                Vec::new()
            }
            _ => self.with(key),
        }
    }

    /// Whether a run is held, waiting to see whether it is an answer.
    pub fn holding(&self) -> bool {
        !self.held.is_empty()
    }

    /// The keys held, handed back because nothing more is coming: a run that
    /// stops short of a terminator is somebody's typing.
    pub fn let_go(&mut self) -> Vec<KeyEvent> {
        std::mem::take(&mut self.held)
    }

    /// What the held run spells after the escape that opened it.
    fn spelt(&self) -> String {
        self.held[1..].iter().filter_map(plain).collect()
    }

    /// The held run and this key after it, all of it somebody's typing.
    fn with(&mut self, key: KeyEvent) -> Vec<KeyEvent> {
        let mut keys = self.let_go();
        keys.push(key);
        keys
    }
}

/// Alt and `]`, which is how the escape opening an answer is read.
fn opens(key: &KeyEvent) -> bool {
    key.code == KeyCode::Char(']') && key.modifiers == KeyModifiers::ALT
}

/// Control and `g` for a bell, or alt and `\` for the other terminator.
fn closes(key: &KeyEvent) -> bool {
    matches!(
        (key.code, key.modifiers),
        (KeyCode::Char('g'), KeyModifiers::CONTROL) | (KeyCode::Char('\\'), KeyModifiers::ALT)
    )
}

/// The character a key is, when it is one typed with no chord.
fn plain(key: &KeyEvent) -> Option<char> {
    match key.code {
        KeyCode::Char(c) if (key.modifiers - KeyModifiers::SHIFT).is_empty() => Some(c),
        _ => None,
    }
}

/// Whether text could still grow into an answer's body.
fn could_be(text: &str) -> bool {
    match text.strip_prefix(SPELT) {
        Some(colour) => colour.chars().all(|c| c.is_ascii_hexdigit() || c == '/'),
        None => SPELT.starts_with(text),
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
    fn every_answer_shape_yields_its_colour() {
        for answer in [
            "\x1b]11;rgb:ffff/ffff/ffff\x1b\\",
            "\x1b]11;rgb:ff/ff/ff\x07",
            "\x1b]11;rgb:fff/fff/fff\x1b\\",
            "\x1b]11;rgb:f/f/f\x07",
        ] {
            assert_eq!(
                background_of(answer),
                Some((0xff, 0xff, 0xff)),
                "{answer:?}"
            );
        }
        assert_eq!(
            background_of("\x1b]11;rgb:2323/1f1f/1f1f\x1b\\"),
            Some((0x23, 0x1f, 0x1f)),
            "what tmux answered for bg=#231f1f"
        );
        assert_eq!(
            background_of("\x1b]11;rgb:23/1f/1f\x07"),
            Some((0x23, 0x1f, 0x1f))
        );
        assert_eq!(
            background_of("\x1b]11;rgb:a/5/0\x07"),
            Some((0xaa, 0x55, 0x00))
        );
        for answer in ["", "\x1b]11;?\x1b\\", "\x1b]11;rgb:ff/ff\x07", "ok"] {
            assert_eq!(background_of(answer), None, "{answer:?}");
        }
    }

    #[test]
    fn a_remembered_colour_is_read_back_as_written() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path().join("agents");
        assert_eq!(remembered(&root), None, "nothing kept yet");
        remember(&root, (0x23, 0x1f, 0x1f)).unwrap();
        assert_eq!(
            std::fs::read_to_string(crate::paths::background_file(&root)).unwrap(),
            "#231f1f\n"
        );
        assert_eq!(remembered(&root).as_deref(), Some("#231f1f"));
    }

    #[test]
    fn a_file_that_is_not_one_colour_is_no_colour() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path().join("agents");
        let file = crate::paths::background_file(&root);
        for kept in [
            "", "231f1f", "#231f1", "#231f1fa", "#23 f1f", "#gg1f1f", "red",
        ] {
            std::fs::write(&file, kept).unwrap();
            assert_eq!(remembered(&root), None, "{kept:?}");
        }
        std::fs::write(&file, "  #231F1F \n").unwrap();
        assert_eq!(remembered(&root).as_deref(), Some("#231F1F"), "trimmed");
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

    /// A reply as the key loop reads it: alt and `]` for the escape that
    /// opens it, a key a character, and the terminator either way it is spelt.
    fn as_keys(answer: &str, bell: bool) -> Vec<KeyEvent> {
        let mut keys = vec![KeyEvent::new(KeyCode::Char(']'), KeyModifiers::ALT)];
        keys.extend(answer.chars().map(|c| KeyEvent::from(KeyCode::Char(c))));
        keys.push(match bell {
            true => KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL),
            false => KeyEvent::new(KeyCode::Char('\\'), KeyModifiers::ALT),
        });
        keys
    }

    /// What comes out of the recogniser for a run of keys, and then whatever
    /// it was still holding when the keys stopped.
    fn through(keys: &[KeyEvent]) -> Vec<KeyEvent> {
        let mut late = Late::default();
        let mut out: Vec<KeyEvent> = keys.iter().flat_map(|key| late.hear(*key)).collect();
        out.extend(late.let_go());
        out
    }

    #[test]
    fn a_reply_arriving_after_the_wait_is_dropped_whole() {
        // Past the wait, the answer reaches the key loop as keys, and `:` and
        // `/` in it would open a line. None of it gets through.
        for bell in [true, false] {
            let reply = as_keys("11;rgb:ffff/ffff/ffff", bell);
            let mut late = Late::default();
            for key in &reply {
                assert!(late.hear(*key).is_empty(), "{key:?} held or dropped");
            }
            assert!(late.let_go().is_empty(), "and nothing left over");
        }
    }

    #[test]
    fn keys_a_person_types_reach_the_list() {
        let colon = KeyEvent::from(KeyCode::Char(':'));
        let slash = KeyEvent::from(KeyCode::Char('/'));
        assert_eq!(through(&[colon, slash]), vec![colon, slash]);

        // Alt and `]` alone is held only until the keys stop.
        let bracket = KeyEvent::new(KeyCode::Char(']'), KeyModifiers::ALT);
        let mut late = Late::default();
        assert!(late.hear(bracket).is_empty());
        assert!(late.holding());
        assert_eq!(late.let_go(), vec![bracket]);

        // And one followed by something no reply spells gives both back, in
        // the order they were typed.
        let j = KeyEvent::from(KeyCode::Char('j'));
        assert_eq!(through(&[bracket, j, slash]), vec![bracket, j, slash]);
    }

    #[test]
    fn a_terminal_that_says_nothing_leaves_everything_as_it_was() {
        // The wait is what it costs, and the answer is the dark everybody had.
        let mut silence: &[u8] = b"";
        let heard = listen(&mut silence).expect("the wait, and nothing in it");
        assert_eq!(said(&heard), None);
    }
}
