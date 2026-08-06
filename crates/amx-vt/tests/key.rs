#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]
//! Key encoding, and the terminal state it depends on.

use amx_vt::{
    Effects, Key, KeyAction, KeyEncoder, KeyEvent, KittyKeyFlags, Mods, Terminal, TerminalOptions,
};

fn terminal() -> Terminal {
    Terminal::new(TerminalOptions {
        cols: 80,
        rows: 24,
        max_scrollback: 0,
    })
    .expect("a terminal")
}

fn press(key: Key, mods: Mods) -> KeyEvent {
    let mut event = KeyEvent::new().expect("an event");
    event.set_action(KeyAction::Press);
    event.set_key(key);
    event.set_mods(mods);
    event
}

fn encoded(encoder: &KeyEncoder, event: &KeyEvent) -> Vec<u8> {
    let mut out = Vec::new();
    encoder.encode(event, &mut out).expect("an encoding");
    out
}

#[test]
fn a_plain_key_encodes_to_its_legacy_sequence() {
    let encoder = KeyEncoder::new().expect("an encoder");
    let mut event = press(Key::A, Mods::NONE);
    event.set_text(b"a");

    assert_eq!(encoded(&encoder, &event), b"a");
}

#[test]
fn control_modifies_the_encoding() {
    let encoder = KeyEncoder::new().expect("an encoder");
    let mut event = press(Key::C, Mods::CTRL);
    event.set_text(b"c");

    assert_eq!(encoded(&encoder, &event), b"\x03");
}

/// DECCKM changes what an arrow key emits, which is why the encoder has to
/// track terminal state rather than a static table.
#[test]
fn application_cursor_keys_change_the_arrow_encoding() {
    let mut encoder = KeyEncoder::new().expect("an encoder");
    let event = press(Key::ArrowUp, Mods::NONE);
    assert_eq!(encoded(&encoder, &event), b"\x1b[A");

    encoder.set_cursor_key_application(true);
    assert_eq!(encoded(&encoder, &event), b"\x1bOA");
}

/// The state the encoder needs lives in the terminal, and one call copies all
/// of it across. This is the path the server actually uses.
#[test]
fn syncing_from_a_terminal_picks_up_the_mode_the_application_set() {
    let mut terminal = terminal();
    let mut effects = Effects::new();
    let mut encoder = KeyEncoder::new().expect("an encoder");
    let event = press(Key::ArrowUp, Mods::NONE);
    assert_eq!(encoded(&encoder, &event), b"\x1b[A");

    // The application turns on application cursor keys.
    terminal.write(b"\x1b[?1h", &mut effects);
    encoder.sync_from(&terminal);

    assert_eq!(encoded(&encoder, &event), b"\x1bOA");
}

/// Kitty flags are the application's choice, and the terminal is where they
/// are recorded; the client passes the protocol through rather than deciding
/// it (D-M0-4).
#[test]
fn kitty_keyboard_flags_are_readable_from_the_terminal() {
    let mut terminal = terminal();
    let mut effects = Effects::new();
    assert_eq!(
        terminal.kitty_keyboard_flags().expect("flags"),
        KittyKeyFlags::NONE
    );

    // CSI > 1 u — push disambiguation onto the Kitty flag stack.
    terminal.write(b"\x1b[>1u", &mut effects);

    let flags = terminal.kitty_keyboard_flags().expect("flags");
    assert!(flags.contains(KittyKeyFlags::DISAMBIGUATE));
}

#[test]
fn kitty_disambiguation_changes_the_encoding() {
    let mut encoder = KeyEncoder::new().expect("an encoder");
    let event = press(Key::Escape, Mods::NONE);
    assert_eq!(encoded(&encoder, &event), b"\x1b");

    encoder.set_kitty_flags(KittyKeyFlags::DISAMBIGUATE);
    assert_eq!(encoded(&encoder, &event), b"\x1b[27u");
}

/// An unmodified modifier key produces nothing, which the API has to express
/// as a successful encoding of zero bytes rather than as an error.
#[test]
fn a_bare_modifier_encodes_to_nothing() {
    let encoder = KeyEncoder::new().expect("an encoder");
    let event = press(Key::ShiftLeft, Mods::SHIFT);

    let mut out = Vec::new();
    assert_eq!(encoder.encode(&event, &mut out).expect("an encoding"), 0);
    assert!(out.is_empty());
}

/// The buffer is grown by the library's own required-size report and then
/// reused, so a hot loop stops allocating.
#[test]
fn encoding_appends_to_a_reused_buffer() {
    let encoder = KeyEncoder::new().expect("an encoder");
    let mut event = press(Key::A, Mods::NONE);
    event.set_text(b"a");

    let mut out = Vec::new();
    for _ in 0..3 {
        encoder.encode(&event, &mut out).expect("an encoding");
    }

    assert_eq!(out, b"aaa");
}

#[test]
fn an_event_reports_back_what_was_set_on_it() {
    let mut event = press(Key::F5, Mods::CTRL | Mods::ALT);

    assert_eq!(event.key().expect("a key"), Key::F5);
    assert!(event.mods().contains(Mods::CTRL));
    assert!(event.mods().contains(Mods::ALT));

    // The event borrows the text, so replacing it must not leave a dangling
    // pointer behind; encoding after the swap is what would notice.
    event.set_text(b"first");
    event.set_text(b"second");
    let encoder = KeyEncoder::new().expect("an encoder");
    let _ = encoded(&encoder, &event);
}
