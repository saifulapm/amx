//! Whether the terminal the view is drawn on is light or dark.
//!
//! Asked once at startup so `theme = "auto"` can pick a legible palette.
//! Sources, in order:
//!
//! 1. The terminal's answer to xterm's OSC 10/11 colour queries. Terminals
//!    that do not support them stay silent.
//! 2. `COLORFGBG`, exported by a few terminals; it can be stale over ssh.
//! 3. Otherwise dark.
//!
//! The terminal is asked whatever the theme, because its colours are also kept
//! (see [`remember`]) for the panes amx starts in detached sessions, which no
//! terminal answers for. The shade picks a palette only under
//! [`crate::theme::AUTO`].

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::io::{Read, Write};
use std::os::fd::{AsFd, BorrowedFd};
use std::path::Path;
use std::time::{Duration, Instant};

/// Whether the terminal background is light or dark.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shade {
    Light,
    Dark,
}

/// xterm's `OSC 10 ; ? ST` and `OSC 11 ; ? ST`, asking for the foreground and
/// background colours in one write.
///
/// Both are asked because agent panes need both: codex tints nothing unless
/// both are answered, and tmux answers the foreground only from a
/// `window-style` that sets one.
const ASK: &str = "\x1b]10;?\x1b\\\x1b]11;?\x1b\\";

/// How long to wait for the answer.
///
/// Long enough for a slow ssh link; a terminal that never answers costs this
/// once at startup. The wait ends as soon as the answer is complete, or on the
/// first byte that is not an escape, since that is the person typing.
const PATIENCE: Duration = Duration::from_millis(200);

/// The `VTIME` of each read, in tenths of a second. Shorter than [`PATIENCE`]
/// so the loop reads more than once.
const TICK: u8 = 1;

/// The shade from the terminal's answer to [`asked`], else `COLORFGBG`, else
/// dark.
pub fn of_the_answer(answer: Option<&str>) -> Shade {
    answer
        .and_then(said)
        .or_else(|| std::env::var("COLORFGBG").ok().as_deref().and_then(told))
        .unwrap_or(Shade::Dark)
}

/// Ask the terminal for its colours and return what it sent back.
///
/// Call once, after the terminal is in raw mode and before keys are read (see
/// [`crate::tui::run`]); otherwise the answer mixes with typed keys.
///
/// `None` when stdin is not a terminal, keys are already waiting, or its
/// settings could not be read or restored.
pub fn asked() -> Option<String> {
    let input = std::io::stdin();
    let fd = input.as_fd();
    if !std::io::IsTerminal::is_terminal(&input) {
        return None;
    }
    // Keys typed before the view was up would be read as the answer and lost,
    // so leave them for the view and do not ask.
    if pending(fd) {
        return None;
    }

    // VMIN 0 with a VTIME timeout: raw mode blocks for one byte, and a
    // terminal that never answers would hang the view.
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

/// Whether input is waiting on `fd`, without reading it.
fn pending(fd: BorrowedFd<'_>) -> bool {
    let mut waiting = nix::libc::pollfd {
        fd: std::os::fd::AsRawFd::as_raw_fd(&fd),
        events: nix::libc::POLLIN,
        revents: 0,
    };
    // SAFETY: one pollfd owned here, and a zero timeout.
    let ready = unsafe { nix::libc::poll(&mut waiting, 1, 0) };
    ready > 0 && waiting.revents & nix::libc::POLLIN != 0
}

/// Apply terminal settings now. `None` on failure.
fn set(fd: BorrowedFd<'_>, how: &nix::sys::termios::Termios) -> Option<()> {
    nix::sys::termios::tcsetattr(fd, nix::sys::termios::SetArg::TCSANOW, how).ok()
}

/// Read until two replies have ended or [`PATIENCE`] runs out.
///
/// A reply ends in BEL or ST (`ESC \`); [`crate::ansi`] treats them alike. A
/// reply cut short would read as a darker colour. A terminal that answers only
/// the background is waited out, and that answer is still returned.
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
        // A reply starts with ESC; anything else is the person typing, so stop
        // before eating their keys.
        if heard.first() != Some(&b'\x1b') {
            break;
        }
        let ended = heard.iter().filter(|&&byte| byte == b'\x07').count()
            + heard.windows(2).filter(|pair| pair == b"\x1b\\").count();
        if ended >= 2 {
            break;
        }
    }
    Some(String::from_utf8_lossy(&heard).into_owned())
}

/// The shade of the background in a terminal's answer.
pub fn said(answer: &str) -> Option<Shade> {
    let (red, green, blue) = colours_of(answer).1?;
    Some(shade_of(red.into(), green.into(), blue.into()))
}

/// An RGB colour, one byte per channel.
pub type Rgb = (u8, u8, u8);

/// The foreground (reply to `10`) and background (reply to `11`) in a
/// terminal's answer, in either order. `None` for a reply that is missing.
pub fn colours_of(answer: &str) -> (Option<Rgb>, Option<Rgb>) {
    let (mut foreground, mut background) = (None, None);
    for reply in answer.split("\x1b]") {
        if let Some(body) = reply.strip_prefix("10;") {
            foreground = foreground.or_else(|| colour_of(body));
        } else if let Some(body) = reply.strip_prefix("11;") {
            background = background.or_else(|| colour_of(body));
        }
    }
    (foreground, background)
}

/// The colour in one reply: `rgb:R/G/B`, each channel one to four hex digits,
/// read as its top eight bits.
///
/// Any other format is `None`, so an unknown terminal falls back to dark.
fn colour_of(reply: &str) -> Option<Rgb> {
    let channels: Vec<&str> = reply
        .strip_prefix("rgb:")?
        .split(|c: char| !c.is_ascii_hexdigit() && c != '/')
        .next()?
        .split('/')
        .collect();
    let [red, green, blue] = channels.as_slice() else {
        return None;
    };
    Some((top(red)?, top(green)?, top(blue)?))
}

/// The top eight bits of a channel written in one to four hex digits.
///
/// A single digit is repeated into both nibbles, as X parses it, so `f` is 255.
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

/// Save the terminal's colours as the tmux style for agent panes:
/// `fg=#rrggbb,bg=#rrggbb`, or `bg=#rrggbb` when only the background was
/// answered. Written atomically, since spawns read it.
pub fn remember(state_root: &Path, foreground: Option<Rgb>, background: Rgb) -> Result<()> {
    let style = match foreground {
        Some(foreground) => format!("fg={},bg={}", hex(foreground), hex(background)),
        None => format!("bg={}", hex(background)),
    };
    crate::store::write_atomic(
        &crate::paths::background_file(state_root),
        format!("{style}\n").as_bytes(),
    )
}

/// A colour as `#rrggbb`.
fn hex((red, green, blue): Rgb) -> String {
    format!("#{red:02x}{green:02x}{blue:02x}")
}

/// The saved tmux style, `bg=#rrggbb` or `fg=#rrggbb,bg=#rrggbb`, or `None`
/// when the file is missing or malformed.
///
/// A bare `#rrggbb` from older versions is read as the background.
pub fn remembered(state_root: &Path) -> Option<String> {
    let kept = std::fs::read_to_string(crate::paths::background_file(state_root)).ok()?;
    let kept = kept.trim();
    if is_hex(kept) {
        return Some(format!("bg={kept}"));
    }
    let background = match kept.split_once(',') {
        Some((foreground, background)) => {
            is_hex(foreground.strip_prefix("fg=")?).then_some(background)?
        }
        None => kept,
    };
    is_hex(background.strip_prefix("bg=")?).then(|| kept.to_string())
}

/// Whether `text` is `#` and six hex digits.
fn is_hex(text: &str) -> bool {
    text.strip_prefix('#')
        .is_some_and(|hex| hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit()))
}

/// The background shade `COLORFGBG` gives.
///
/// The background is the last field: `0;15` (light), `15;0` (dark), or rxvt's
/// `0;default;15`. It is an ANSI index; 7 and 9 to 15 are light, and 8 (bright
/// black) counts as dark.
pub fn told(said: &str) -> Option<Shade> {
    let background: u8 = said.rsplit(';').next()?.trim().parse().ok()?;
    match background {
        7 | 9..=15 => Some(Shade::Light),
        0..=6 | 8 => Some(Shade::Dark),
        _ => None,
    }
}

/// Filters a terminal answer that arrives after [`PATIENCE`] out of the key
/// stream.
///
/// A late answer reaches the key loop as alt-`]` followed by one key per
/// character, and its `:` and `/` would trigger view commands. Keys are held
/// while they could still be an answer, dropped when they are one, and handed
/// back in order otherwise.
#[derive(Default)]
pub struct Late {
    held: Vec<KeyEvent>,
}

/// The text between the opening escape and the colour, for each reply.
const SPELT: [&str; 2] = ["10;rgb:", "11;rgb:"];

impl Late {
    /// Feed one key and return the keys to act on now: none while a run is
    /// held or when it turned out to be an answer.
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
            // Parsed the same way as an answer read during the wait.
            return match colours_of(&format!("\x1b]{spelt}\x07")) {
                (Some(_), _) | (_, Some(_)) => {
                    self.held.clear();
                    Vec::new()
                }
                (None, None) => self.with(key),
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

    /// Whether keys are being held.
    pub fn holding(&self) -> bool {
        !self.held.is_empty()
    }

    /// Release the held keys when no more input is coming; a run without a
    /// terminator was typed.
    pub fn let_go(&mut self) -> Vec<KeyEvent> {
        std::mem::take(&mut self.held)
    }

    /// The characters held after the opening escape.
    fn spelt(&self) -> String {
        self.held[1..].iter().filter_map(plain).collect()
    }

    /// Release the held keys followed by `key`.
    fn with(&mut self, key: KeyEvent) -> Vec<KeyEvent> {
        let mut keys = self.let_go();
        keys.push(key);
        keys
    }
}

/// Alt-`]`, the key the opening `ESC ]` of an answer reads as.
fn opens(key: &KeyEvent) -> bool {
    key.code == KeyCode::Char(']') && key.modifiers == KeyModifiers::ALT
}

/// Ctrl-`g` (BEL) or alt-`\` (ST), the keys a terminator reads as.
fn closes(key: &KeyEvent) -> bool {
    matches!(
        (key.code, key.modifiers),
        (KeyCode::Char('g'), KeyModifiers::CONTROL) | (KeyCode::Char('\\'), KeyModifiers::ALT)
    )
}

/// The character of a key pressed with no modifier other than shift.
fn plain(key: &KeyEvent) -> Option<char> {
    match key.code {
        KeyCode::Char(c) if (key.modifiers - KeyModifiers::SHIFT).is_empty() => Some(c),
        _ => None,
    }
}

/// Whether `text` could still grow into a reply.
fn could_be(text: &str) -> bool {
    SPELT.iter().any(|spelt| match text.strip_prefix(spelt) {
        Some(colour) => colour.chars().all(|c| c.is_ascii_hexdigit() || c == '/'),
        None => spelt.starts_with(text),
    })
}

/// Light when the colour's Rec. 709 luminance is at least half.
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
        // White in every channel width reads the same: the top eight bits.
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
                colours_of(answer),
                (None, Some((0xff, 0xff, 0xff))),
                "{answer:?}"
            );
        }
        assert_eq!(
            colours_of("\x1b]11;rgb:2323/1f1f/1f1f\x1b\\").1,
            Some((0x23, 0x1f, 0x1f)),
            "what tmux answered for bg=#231f1f"
        );
        assert_eq!(
            colours_of("\x1b]11;rgb:23/1f/1f\x07").1,
            Some((0x23, 0x1f, 0x1f))
        );
        assert_eq!(
            colours_of("\x1b]11;rgb:a/5/0\x07").1,
            Some((0xaa, 0x55, 0x00))
        );
        for answer in ["", "\x1b]11;?\x1b\\", "\x1b]11;rgb:ff/ff\x07", "ok"] {
            assert_eq!(colours_of(answer), (None, None), "{answer:?}");
        }
    }

    #[test]
    fn both_replies_are_read_out_of_one_answer_in_either_order() {
        let tmux = "\x1b]10;rgb:e5e5/e0e0/dcdc\x1b\\\x1b]11;rgb:2121/1b1b/1b1b\x1b\\";
        let both = (Some((0xe5, 0xe0, 0xdc)), Some((0x21, 0x1b, 0x1b)));
        assert_eq!(colours_of(tmux), both, "what tmux answered for the pair");
        assert_eq!(
            colours_of("\x1b]11;rgb:21/1b/1b\x07\x1b]10;rgb:e5/e0/dc\x07"),
            both,
            "backwards"
        );
        assert_eq!(
            colours_of("\x1b]10;rgb:e5/e0/dc\x07"),
            (Some((0xe5, 0xe0, 0xdc)), None)
        );
        assert_eq!(
            said(tmux),
            Some(Shade::Dark),
            "the shade is the background's"
        );
        assert_eq!(
            said("\x1b]10;rgb:ffff/ffff/ffff\x07"),
            None,
            "not the foreground's"
        );
    }

    #[test]
    fn a_remembered_colour_is_read_back_as_written() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path().join("agents");
        let file = crate::paths::background_file(&root);
        assert_eq!(remembered(&root), None, "nothing kept yet");
        remember(&root, Some((0xe5, 0xe0, 0xdc)), (0x21, 0x1b, 0x1b)).unwrap();
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "fg=#e5e0dc,bg=#211b1b\n"
        );
        assert_eq!(remembered(&root).as_deref(), Some("fg=#e5e0dc,bg=#211b1b"));
        remember(&root, None, (0x23, 0x1f, 0x1f)).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "bg=#231f1f\n");
        assert_eq!(remembered(&root).as_deref(), Some("bg=#231f1f"));
    }

    #[test]
    fn a_colour_kept_before_the_foreground_was_is_the_background_alone() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path().join("agents");
        std::fs::write(crate::paths::background_file(&root), "#231f1f\n").unwrap();
        assert_eq!(remembered(&root).as_deref(), Some("bg=#231f1f"));
    }

    #[test]
    fn a_file_that_is_not_one_colour_is_no_colour() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path().join("agents");
        let file = crate::paths::background_file(&root);
        for kept in [
            "",
            "231f1f",
            "#231f1",
            "#231f1fa",
            "#23 f1f",
            "#gg1f1f",
            "red",
            "bg=",
            "bg=#231f1",
            "fg=#e5e0dc",
            "bg=#211b1b,fg=#e5e0dc",
            "fg=#e5e0dc,bg=#211b1b,",
            "fg=#e5e0dc, bg=#211b1b",
            "fg=#e5e0dc,bg=#211b1b,bold",
            "fg=red,bg=#211b1b",
            "bg=#211b1b;set -g x",
        ] {
            std::fs::write(&file, kept).unwrap();
            assert_eq!(remembered(&root), None, "{kept:?}");
        }
        std::fs::write(&file, "  #231F1F \n").unwrap();
        assert_eq!(remembered(&root).as_deref(), Some("bg=#231F1F"), "trimmed");
    }

    #[test]
    fn a_green_terminal_is_lighter_than_a_blue_one_of_the_same_numbers() {
        // Rec. 709 weights green far above blue.
        assert_eq!(said("\x1b]11;rgb:00/c0/00\x07"), Some(Shade::Light));
        assert_eq!(said("\x1b]11;rgb:00/00/c0\x07"), Some(Shade::Dark));
    }

    #[test]
    fn an_answer_nothing_can_read_is_nothing_rather_than_a_guess() {
        // An answer that cannot be parsed gives no shade.
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
    fn the_answer_is_read_to_both_terminators_and_no_further() {
        // The read stops at the second terminator; a truncated answer would
        // read darker.
        let mut sent: &[u8] = b"\x1b]10;rgb:0000/0000/0000\x1b\\\x1b]11;rgb:ffff/ffff/ffff\x07";
        let began = Instant::now();
        let heard = listen(&mut sent).expect("the answer");
        assert!(began.elapsed() < PATIENCE, "and does not wait out the rest");
        assert_eq!(
            colours_of(&heard),
            (Some((0, 0, 0)), Some((0xff, 0xff, 0xff)))
        );
    }

    #[test]
    fn a_terminal_answering_the_background_alone_is_waited_out_and_heard() {
        let mut sent: &[u8] = b"\x1b]11;rgb:ffff/ffff/ffff\x07";
        let heard = listen(&mut sent).expect("the answer");
        assert_eq!(colours_of(&heard), (None, Some((0xff, 0xff, 0xff))));
        assert_eq!(said(&heard), Some(Shade::Light));
    }

    #[test]
    fn the_view_asks_for_both_colours_in_one_write() {
        assert_eq!(ASK, "\x1b]10;?\x1b\\\x1b]11;?\x1b\\");
    }

    #[test]
    fn a_key_pressed_while_the_view_opens_ends_the_wait_rather_than_feeding_it() {
        // A typed letter ends the wait at once.
        let mut typing: &[u8] = b"j";
        let began = Instant::now();
        let heard = listen(&mut typing).expect("the letter");
        assert!(began.elapsed() < PATIENCE, "and does not wait out the rest");
        assert_eq!(said(&heard), None);
    }

    #[test]
    fn a_key_already_waiting_is_left_for_the_view() {
        // Keys typed before the view is up must stay unread, so the terminal
        // is not asked.
        let pty = nix::pty::openpty(None, None).expect("a pty");
        // Raw mode, as the view has it when it asks.
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

    /// A reply as the key loop sees it: alt-`]`, one key per character, then
    /// the terminator as BEL or ST.
    fn as_keys(answer: &str, bell: bool) -> Vec<KeyEvent> {
        let mut keys = vec![KeyEvent::new(KeyCode::Char(']'), KeyModifiers::ALT)];
        keys.extend(answer.chars().map(|c| KeyEvent::from(KeyCode::Char(c))));
        keys.push(match bell {
            true => KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL),
            false => KeyEvent::new(KeyCode::Char('\\'), KeyModifiers::ALT),
        });
        keys
    }

    /// The keys [`Late`] passes through for `keys`, plus what it still held
    /// at the end.
    fn through(keys: &[KeyEvent]) -> Vec<KeyEvent> {
        let mut late = Late::default();
        let mut out: Vec<KeyEvent> = keys.iter().flat_map(|key| late.hear(*key)).collect();
        out.extend(late.let_go());
        out
    }

    #[test]
    fn a_reply_arriving_after_the_wait_is_dropped_whole() {
        // None of a late answer reaches the key loop.
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
    fn a_late_foreground_reply_is_dropped_whole_too() {
        for bell in [true, false] {
            let reply = as_keys("10;rgb:e5e5/e0e0/dcdc", bell);
            let mut late = Late::default();
            for key in &reply {
                assert!(late.hear(*key).is_empty(), "{key:?} held or dropped");
            }
            assert!(late.let_go().is_empty(), "and nothing left over");
        }
    }

    #[test]
    fn both_late_replies_back_to_back_are_dropped_whole() {
        for bell in [true, false] {
            let mut keys = as_keys("10;rgb:e5e5/e0e0/dcdc", bell);
            keys.extend(as_keys("11;rgb:2121/1b1b/1b1b", bell));
            assert!(through(&keys).is_empty(), "bell {bell}");
        }
    }

    #[test]
    fn a_run_that_is_neither_reply_is_handed_back() {
        let keys = as_keys("12;rgb:ffff/ffff/ffff", true);
        assert_eq!(through(&keys), keys);
    }

    #[test]
    fn keys_a_person_types_reach_the_list() {
        let colon = KeyEvent::from(KeyCode::Char(':'));
        let slash = KeyEvent::from(KeyCode::Char('/'));
        assert_eq!(through(&[colon, slash]), vec![colon, slash]);

        // Alt-`]` alone is held only until the keys stop.
        let bracket = KeyEvent::new(KeyCode::Char(']'), KeyModifiers::ALT);
        let mut late = Late::default();
        assert!(late.hear(bracket).is_empty());
        assert!(late.holding());
        assert_eq!(late.let_go(), vec![bracket]);

        // Followed by something no reply starts with, both are handed back in
        // order.
        let j = KeyEvent::from(KeyCode::Char('j'));
        assert_eq!(through(&[bracket, j, slash]), vec![bracket, j, slash]);
    }

    #[test]
    fn a_terminal_that_says_nothing_leaves_everything_as_it_was() {
        // Silence costs the wait and gives no shade.
        let mut silence: &[u8] = b"";
        let heard = listen(&mut silence).expect("the wait, and nothing in it");
        assert_eq!(said(&heard), None);
    }
}
