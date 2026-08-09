//! What a pane's application asked its terminal to report about the mouse.
//!
//! D9 forwards SGR reports to a pane whose application enabled mouse reporting
//! and to no other, and until M4 nothing answered the question: the accessor
//! that would (`Terminal::mouse_tracking`) had no caller anywhere in the tree
//! and no wire field carried the answer (`docs/11-m4-plan.md` D-M4-1). This is
//! the read, and [`super::parser`] is the only thread allowed to make it.
//!
//! **Polled, because there is no callback.** libghostty-vt reports side effects
//! through `amx_vt::TerminalEvent`, whose whole vocabulary is `Bell`,
//! `TitleChanged`, `PwdChanged` and `ClipboardWrite`
//! (`crates/amx-vt/src/callbacks.rs`) — no variant announces a mode change. So
//! the parser reads the mode after a parse, on the thread already holding the
//! terminal, exactly where a title is read the same way today.
//!
//! **One getter in the case that matters.** `Terminal::mouse_tracking` is
//! documented as "true if any of the mouse tracking modes (X10, normal, button,
//! or any-event) are enabled" (`vendor/libghostty-vt/include/ghostty/vt/terminal.h`,
//! `GHOSTTY_TERMINAL_DATA_MOUSE_TRACKING`), so a pane running a shell — which
//! is nearly every pane, nearly all the time — costs one FFI call per parsed
//! chunk and allocates nothing. The eight mode reads that resolve *which*
//! tracking happen only for a pane that has some.
//!
//! **Two questions, not one.** An application picks an event mode and a report
//! format independently (`vendor/libghostty-vt/src/terminal/mouse.zig:7-13` and
//! `:22-28`), and a pane that enabled `?1000` without `?1006` is expecting the
//! X10 encoding — handing it an SGR report delivers bytes it cannot parse. Both
//! halves travel so a reader can answer "would this pane understand what I am
//! about to send it" (`docs/notes/m4-mouse-path.md` F-2).
//!
//! # What the modes can and cannot say
//!
//! A terminal's effective event mode is *one enum*, overwritten by each set —
//! `flags.mouse_event = .button`
//! (`vendor/libghostty-vt/src/terminal/stream_terminal.zig:618-644`), and
//! `mouse.zig:7-13` says the variants "are all mutually exclusive". The **mode
//! bits are not**: setting `?1002` leaves the `?1000` bit standing, and the
//! only thing exported to C is the four bits — `mouse_tracking` is their `or`
//! (`vendor/libghostty-vt/src/terminal/c/terminal.zig:859-862`) and the
//! resolved enum has no accessor at all.
//!
//! So an application that asked for two event modes without resetting the first
//! reads back here as **the more capable of them**, which is what
//! [`most_capable`] picks and what the ordering of [`EVENTS`] and [`FORMATS`]
//! encodes. For the single-mode case every real application produces, that is
//! exact. For the two-mode case it can name `any` where the terminal would send
//! `button`, and — the one that could matter — it can name a format the pane
//! did not settle on, in which case amx *drops* reports it could have relayed
//! rather than sending an encoding the pane cannot read. Erring toward the drop
//! is deliberate: a wrong `events` is inert (nothing reads it but a human), and
//! a wrong `format` in the other direction would put unparseable bytes in front
//! of a program.

use amx_proto::control::session::{MouseEvents, MouseFormat, MouseMode};
use amx_vt::{Mode, Terminal};

/// The event modes, least capable first. See the module header for why the
/// order is the tie-break.
const EVENTS: &[(u16, MouseEvents)] = &[
    (9, MouseEvents::X10),
    (1000, MouseEvents::Normal),
    (1002, MouseEvents::Button),
    (1003, MouseEvents::Any),
];

/// The report formats, in the same shape. `x10` is what a terminal reports in
/// when no format mode is set, so it has no mode number and is the fallback
/// below.
const FORMATS: &[(u16, MouseFormat)] = &[
    (1005, MouseFormat::Utf8),
    (1006, MouseFormat::Sgr),
    (1015, MouseFormat::Urxvt),
    (1016, MouseFormat::SgrPixels),
];

/// Read `terminal`'s mouse mode, or `None` when its application asked for no
/// reporting at all.
///
/// A mode the library refuses to answer for is treated as unset rather than
/// guessed at, the same rule the handoff manifest's mode capture follows.
pub(super) fn mouse_mode(terminal: &Terminal) -> Option<MouseMode> {
    if !terminal.mouse_tracking().unwrap_or(false) {
        return None;
    }
    // `mouse_tracking` is the `or` of exactly these four, so it answering true
    // and this finding nothing would mean the library disagreed with itself.
    let events = most_capable(terminal, EVENTS)?;
    // The one that can legitimately find nothing: with an event mode set and no
    // format mode, the terminal reports in the original X10 encoding.
    let format = most_capable(terminal, FORMATS).unwrap_or(MouseFormat::X10);
    Some(MouseMode { events, format })
}

/// The most capable of `modes` the terminal has set — the last one in the
/// slice, which is ordered for exactly this.
fn most_capable<T: Copy>(terminal: &Terminal, modes: &[(u16, T)]) -> Option<T> {
    let mut found = None;
    for &(number, value) in modes {
        if terminal.mode(Mode::dec(number)).unwrap_or(false) {
            found = Some(value);
        }
    }
    found
}
