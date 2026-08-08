#![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]
//! The terminal handle: ownership, writes, and effect ordering.

use amx_vt::{ClipboardLocation, Effects, Mode, Screen, Terminal, TerminalEvent, TerminalOptions};

fn terminal(cols: u16, rows: u16) -> Terminal {
    Terminal::new(TerminalOptions {
        cols,
        rows,
        max_scrollback: 100,
    })
    .expect("a terminal")
}

/// 04 §3: "the parser I/O thread **exclusively owns** the libghostty-vt
/// instance". `Terminal` must therefore be `Send` — ownership moves onto that
/// thread — and must not be `Sync`, because a shared reference crossing a
/// thread boundary is exactly the sharing the rule forbids.
///
/// The negative half cannot be written as a runtime assertion, so it is written
/// as a compile-time one: two blanket impls of `AmbiguousIfSync` apply to any
/// `Sync` type and exactly one applies otherwise, so naming the method is
/// ambiguous — and this file stops compiling — the moment `Terminal` gains a
/// `Sync` impl. `crate::terminal::Terminal` carries the same assertion as a
/// `compile_fail` doctest; this one guards the crate's own test build.
#[test]
fn terminal_is_not_sync() {
    trait AmbiguousIfSync<Marker> {
        fn assert_not_sync() {}
    }
    struct Anything;
    struct OnlySync;

    impl<T: ?Sized> AmbiguousIfSync<Anything> for T {}
    impl<T: ?Sized + Sync> AmbiguousIfSync<OnlySync> for T {}

    // Resolves only because exactly one impl applies: `Terminal` is not `Sync`.
    <Terminal as AmbiguousIfSync<_>>::assert_not_sync();

    fn sendable<T: Send>() {}
    sendable::<Terminal>();
}

#[test]
fn vt_write_leaves_the_text_in_the_grid_and_moves_the_cursor() {
    let mut terminal = terminal(10, 3);
    let mut effects = Effects::new();
    terminal.write(b"hi", &mut effects);

    assert!(effects.is_empty(), "plain text produces no effects");
    let cursor = terminal.cursor().expect("a cursor");
    assert_eq!((cursor.x, cursor.y), (2, 0));
    assert!(cursor.visible);
}

/// The DA reply is produced by a callback that runs inside `vt_write`
/// (`terminal.h:77`), so it takes its place in the reply stream at the point
/// the application will expect it — not appended afterwards, and not overtaken
/// by the in-band replies around it. That ordering is what T05's PTY actor
/// writes back to the pty, so it has to hold here first.
#[test]
fn device_attributes_query_reply_is_captured_in_order() {
    let mut terminal = terminal(10, 3);
    let mut effects = Effects::new();

    // DSR cursor position, then DA1, then DSR again from a different column.
    terminal.write(b"\x1b[6n\x1b[c ab\x1b[6n", &mut effects);

    let replies = String::from_utf8(effects.replies().to_vec()).expect("ascii replies");
    let da = replies.find("\x1b[?").expect("a DA1 reply");
    let first = replies.find("\x1b[1;1R").expect("a CPR from column 1");
    let last = replies.rfind("\x1b[1;4R").expect("a CPR from column 4");
    assert!(
        first < da && da < last,
        "replies must keep the order the sequences arrived in: {replies:?}"
    );

    // The DA1 answer is the one the terminal was configured with.
    let attributes = &terminal.query_answers().device_attributes;
    assert!(
        replies.contains(&format!("\x1b[?{}", attributes.conformance_level)),
        "DA1 reports the configured conformance level: {replies:?}"
    );
}

/// Effects accumulate across one write in the order the parser met them, and
/// the buffer is reusable: the caller clears it and hands the same one back.
#[test]
fn effects_accumulate_in_order_and_the_buffer_is_reusable() {
    let mut terminal = terminal(10, 3);
    let mut effects = Effects::new();

    terminal.write(b"\x07\x1b]2;pane\x07\x07", &mut effects);
    assert_eq!(
        effects.events(),
        [
            TerminalEvent::Bell,
            TerminalEvent::TitleChanged,
            TerminalEvent::Bell
        ]
    );
    assert_eq!(terminal.title().expect("a title").as_deref(), Some("pane"));

    effects.clear();
    assert!(effects.is_empty());
    terminal.write(b"\x07", &mut effects);
    assert_eq!(effects.events(), [TerminalEvent::Bell]);
}

#[test]
fn osc_52_is_decoded_into_a_clipboard_effect() {
    let mut terminal = terminal(10, 3);
    let mut effects = Effects::new();

    // "amx" base64-encoded, written to the standard clipboard.
    terminal.write(b"\x1b]52;c;YW14\x07", &mut effects);

    match effects.events() {
        [TerminalEvent::ClipboardWrite { location, contents }] => {
            assert_eq!(*location, ClipboardLocation::Standard);
            let payload = contents.first().expect("one representation");
            assert_eq!(payload.data, b"amx");
        }
        other => panic!("expected one clipboard write, got {other:?}"),
    }
}

#[test]
fn osc_7_reports_a_pwd_change_and_the_value_reads_back() {
    let mut terminal = terminal(10, 3);
    let mut effects = Effects::new();

    terminal.write(b"\x1b]7;file:///tmp\x07", &mut effects);

    assert_eq!(effects.events(), [TerminalEvent::PwdChanged]);
    assert_eq!(
        terminal.pwd().expect("a pwd").as_deref(),
        Some("file:///tmp")
    );
}

/// R7: the vendored patch turns grapheme clustering on by default, and the
/// wrapper has to be able to see that rather than assume it.
#[test]
fn grapheme_cluster_mode_is_readable_and_settable() {
    let mut terminal = terminal(10, 3);
    assert!(terminal.mode(Mode::GRAPHEME_CLUSTER).expect("mode 2027"));

    terminal
        .set_mode(Mode::GRAPHEME_CLUSTER, false)
        .expect("2027 is settable");
    assert!(!terminal.mode(Mode::GRAPHEME_CLUSTER).expect("mode 2027"));
}

#[test]
fn an_unknown_mode_is_an_error_rather_than_a_silent_false() {
    let terminal = terminal(10, 3);
    assert!(terminal.mode(Mode::dec(31_337)).is_err());
}

#[test]
fn mode_packing_matches_the_header_layout() {
    // modes.h:100 — bits 0..14 are the number, bit 15 is the ANSI flag.
    assert_eq!(Mode::dec(2027).number(), 2027);
    assert!(!Mode::dec(2027).is_ansi());
    assert!(Mode::ansi(4).is_ansi());
    assert_eq!(Mode::ansi(4).number(), 4);
    assert_ne!(Mode::ansi(4), Mode::dec(4));
}

#[test]
fn the_alternate_screen_is_observable() {
    let mut terminal = terminal(10, 3);
    let mut effects = Effects::new();
    assert_eq!(terminal.active_screen().expect("a screen"), Screen::Primary);

    terminal.write(b"\x1b[?1049h", &mut effects);
    assert_eq!(
        terminal.active_screen().expect("a screen"),
        Screen::Alternate
    );
}

#[test]
fn resize_changes_the_reported_geometry() {
    let mut terminal = terminal(10, 3);
    terminal.resize(20, 5).expect("a resize");

    assert_eq!(terminal.cols().expect("cols"), 20);
    assert_eq!(terminal.rows().expect("rows"), 5);
    assert!(terminal.resize(0, 5).is_err(), "zero columns is rejected");
}

#[test]
fn reset_clears_the_grid_but_keeps_the_geometry() {
    let mut terminal = terminal(10, 3);
    let mut effects = Effects::new();
    terminal.write(b"hi", &mut effects);

    terminal.reset();

    assert_eq!(terminal.cols().expect("cols"), 10);
    let cursor = terminal.cursor().expect("a cursor");
    assert_eq!((cursor.x, cursor.y), (0, 0));
}

/// The ownership rule is only useful if the owner can actually be moved onto
/// the parser thread.
#[test]
fn a_terminal_moves_to_another_thread() {
    let mut terminal = terminal(10, 3);
    let handle = std::thread::spawn(move || {
        let mut effects = Effects::new();
        terminal.write(b"hi", &mut effects);
        terminal.cursor().expect("a cursor").x
    });
    assert_eq!(handle.join().expect("the thread"), 2);
}
