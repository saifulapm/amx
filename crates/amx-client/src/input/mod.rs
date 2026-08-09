//! The modal input layer: terminal, prefix, navigate (04 §7).
//!
//! [`Input`] is a byte-stream state machine, deliberately not a key decoder:
//! it recognises exactly the prefix key, the mode keys of the layer it is in,
//! and the extent of SGR mouse reports — everything else passes through as
//! opaque byte runs, so kitty-protocol sequences (and every encoding this
//! machine has never heard of) reach the pane byte-identical. Routing input
//! through a lossy key-event type is exactly what this design forbids.
//!
//! The machine emits [`Action`]s — positions into the fed slice plus decoded
//! verbs — and `App` turns them into [`InputEvent`]s: bytes forwarded to the
//! focused pane's raw I/O stream, or control calls from the T04 method table.
//! Splitting it this way keeps the decode pure (testable without a terminal
//! or a server) and keeps every layout consequence where 04 §3 puts it: the
//! client's layout mirror is server truth, so navigate's verbs *call* the
//! server and repaint when new state arrives; nothing here mutates the
//! mirror.
//!
//! Mode keys (04 §7):
//!
//! - **terminal** (default): bytes go to the focused pane; the prefix key
//!   (`ctrl+a` as shipped) enters prefix.
//! - **prefix** (one-shot): `w` enters navigate, a second press of the prefix
//!   sends the literal byte to the pane, `x`/`v` split, `z` zooms, `d`
//!   detaches, `p` opens the picker, `a` jumps to the next agent waiting on
//!   the user (04 §7 lists `next-attention` among the prefix one-shots; 03:
//!   "One key jumps to the next blocked agent"); any other key is swallowed
//!   and the mode falls back to terminal. Closing a pane is navigate's `d`,
//!   deliberately not prefix's: the detach verb owns the prefix chord (04 §7
//!   lists detach among the prefix one-shots) and a destroy verb must not sit
//!   one key from it.
//!
//!   The prefix key and this table are **data**, not literals: they are
//!   [`crate::config::Bindings`], resolved out of `[keys]` and handed to the
//!   machine by whoever built it. The list above is what an empty
//!   configuration resolves to.
//! - **navigate** (sticky): `hjkl` move focus, `HJKL` resize, `x`/`v` split,
//!   `s`+direction swaps with the neighbour, `m` moves the pane to another
//!   workspace, `d` closes, digits jump to the n-th pane, `c` enters copy
//!   mode, `Esc` returns to terminal.
//! - **copy** (entered from navigate with `c`): every byte is consumed by the
//!   copy-mode engine, never forwarded. The key table lives in `copy.rs` —
//!   [`crate::copy::mode_after`] is the whole of this machine's dispatch for
//!   the mode, so the byte that ends it (`y`, `Esc`, `q`) is decided in the
//!   same table the engine reads and the two cannot drift.
//!
//! SGR mouse reports are recognised in every mode, and a report's bytes are
//! never mistaken for mode keys (D9). What happens to one then depends on the
//! mode and on the pane:
//!
//! - **terminal mode, pane asked for reports**: relayed to it, with the
//!   coordinates rewritten into the pane's own frame — the `mouse` submodule
//!   has why "unchanged" cannot mean "byte-for-byte" and what the fence is
//!   instead.
//! - **terminal mode, pane did not ask**: D14's wheel exception. A wheel turn
//!   becomes an [`Action::Mouse`] carrying only the [`Wheel`] it decoded;
//!   every other report is dropped whole.
//! - **chrome modes**: [`Input::feed_chrome`] is what the picker and copy mode
//!   read their bytes through, so a report never reaches a surface's key table
//!   as an `Esc` that cancels it. Copy mode acts on the wheel and nothing
//!   else; the picker acts on nothing.
//!
//! No column and no row is read on any path but the relay's, and the relay
//! neither chooses a pane by position nor interprets what it carries.

mod mouse;
mod reports;

use std::collections::HashMap;

use amx_core::{Direction, PaneId};
use amx_proto::control::Call;
use amx_proto::control::pane::{MoveDirection, SplitDirection};
use amx_proto::control::session::MouseMode;

use crate::app::Mode;
use crate::config::{Bindings, PrefixAction};

pub use reports::{Chrome, Wheel};

/// The prefix key amx ships, `ctrl+a` (04 §7).
///
/// The *shipped* one: what a client with no `[keys] prefix` runs. The prefix a
/// given machine is running is [`Input::prefix`], because since M4 it is
/// configuration rather than a constant (D-M4-8).
pub const PREFIX: u8 = crate::config::SHIPPED_PREFIX;

/// `Esc`.
const ESC: u8 = 0x1b;

/// How much of the containing split's ratio one `HJKL` press moves.
/// Exactly representable in an f32, so it survives the wire unchanged.
pub const RESIZE_STEP: f32 = 0.0625;

/// One decoded intent from a fed byte slice.
///
/// Byte runs are carried as positions into the slice [`Input::feed`] was
/// given — not copies — so a keystroke's path from `read()` to the pane
/// stays allocation-free.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    /// Forward `bytes[start..end]` to the focused pane, byte-identical.
    Forward {
        /// Start of the run.
        start: usize,
        /// One past its end.
        end: usize,
    },
    /// `bytes[start..end]` is one whole SGR mouse report.
    ///
    /// Relayed to the focused pane if it enabled SGR reporting; otherwise
    /// `wheel` is the whole of what may be acted on, and a report that is not
    /// a wheel turn is dropped.
    Mouse {
        /// Start of the report.
        start: usize,
        /// One past its final byte.
        end: usize,
        /// The wheel turn this report describes, if it is one. Decoded from
        /// the button alone.
        wheel: Option<Wheel>,
    },
    /// A report carried across a read boundary completed; its bytes are in
    /// [`Input::carried`]. Gated like [`Action::Mouse`], and carrying the same
    /// wheel decode.
    CarriedMouse(Option<Wheel>),
    /// Carried bytes turned out not to be a mouse report; [`Input::carried`]
    /// holds them and they forward verbatim.
    CarriedBytes,
    /// `hjkl`: move focus to the geometric neighbour.
    Focus(Direction),
    /// `HJKL`: grow (`L`/`J`) or shrink (`H`/`K`) the focused pane's slot.
    Resize(Direction),
    /// `x`/`v`: split the focused pane.
    Split(SplitDirection),
    /// `s` + direction: swap the focused pane with its neighbour.
    Swap(Direction),
    /// `m`: move the focused pane to another workspace.
    MovePane,
    /// Navigate `d`: close the focused pane.
    Close,
    /// Prefix `z`: toggle zoom on the focused pane.
    Zoom,
    /// Prefix `d`: detach this client, leaving the session running.
    Detach,
    /// Prefix `p`: open the picker.
    Picker,
    /// Prefix `a`: focus the head of the attention queue.
    ///
    /// One key for "handle the next one" is the point of the queue existing at
    /// all (03), so this needs no focused pane and no non-empty workspace: the
    /// server holds the queue and answers an empty one honestly.
    NextAttention,
    /// A digit: jump focus to the n-th pane (1-based, layout order).
    Jump(u8),
}

impl Action {
    const fn forward(start: usize, end: usize) -> Self {
        Self::Forward { start, end }
    }
}

/// What one round of input handling asks the outside world to do.
///
/// `App::handle_input` hands these to a caller-provided sink — the same shape
/// T13 gave `settle_resize` — because neither destination exists inside the
/// client yet: forwarding rides the raw pane I/O stream once its codec lands,
/// and calls ride the control channel. `pane.focus` in particular is sent
/// fire-and-forget: local focus has already moved by the time it is emitted.
#[derive(Clone, PartialEq, Debug)]
pub enum InputEvent<'a> {
    /// Write these bytes into `pane`'s PTY, byte-identical.
    Forward {
        /// The pane the input addresses.
        pane: PaneId,
        /// The bytes, verbatim.
        bytes: &'a [u8],
    },
    /// Send this control call to the server.
    Call(Call),
    /// Detach this client: leave the terminal, keep the session running.
    Detach,
}

/// The input state machine's own state: what survives between reads.
#[derive(Debug, Default)]
pub struct Input {
    /// The prefix key and the prefix table this machine runs on.
    ///
    /// [`Default`] is what amx ships, so a machine nobody configured and a
    /// machine whose user has no `[keys]` section behave identically without
    /// anyone writing that rule down twice.
    bindings: Bindings,
    /// `s` was pressed in navigate; the next direction key names the swap.
    pending_swap: bool,
    /// What each pane's application asked its terminal to report about the
    /// mouse, folded from `session.state`'s `PaneState.mouse`.
    ///
    /// A pane with no entry asked for nothing, which is every pane running a
    /// shell — and the two cases a reader cares about are "relay to it" and
    /// "do not", so an absent entry and an entry in an encoding this client
    /// cannot produce both land on the second.
    mouse_modes: HashMap<PaneId, MouseMode>,
    /// An unterminated SGR report candidate carried across a read boundary.
    carry: Vec<u8>,
    /// The last resolved carry, kept readable until the next feed so the
    /// [`Action::CarriedMouse`]/[`Action::CarriedBytes`] it produced can be
    /// forwarded from it.
    done: Vec<u8>,
    /// Reused action buffer, so a keystroke allocates nothing steady-state.
    scratch: Vec<Action>,
    /// [`Self::scratch`]'s counterpart for [`Self::feed_chrome`].
    chrome: Vec<Chrome>,
    /// Where a relayed report is rewritten. Reused for the same reason
    /// [`Self::scratch`] is: 04's performance rule names event dispatch, and a
    /// wheel held down is a report every few milliseconds.
    relay: Vec<u8>,
}

impl Input {
    /// A machine in terminal mode with nothing carried.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Run on `bindings` from here on.
    ///
    /// Called once, before the loop starts, by whoever read the configuration
    /// — `amx attach` in the shipped client. It is deliberately not a
    /// constructor argument: `App` builds its own machine, and threading a
    /// table through every `App` constructor to reach one field would put
    /// configuration in the middle of code that has no business knowing there
    /// is a file.
    pub fn set_bindings(&mut self, bindings: Bindings) {
        self.bindings = bindings;
    }

    /// The bindings this machine is running.
    #[must_use]
    pub const fn bindings(&self) -> &Bindings {
        &self.bindings
    }

    /// The byte that opens the prefix layer on this machine.
    #[must_use]
    pub const fn prefix(&self) -> u8 {
        self.bindings.prefix()
    }

    /// Take the reusable action buffer (empty, capacity kept).
    pub(crate) fn take_scratch(&mut self) -> Vec<Action> {
        std::mem::take(&mut self.scratch)
    }

    /// Return the action buffer for reuse.
    pub(crate) fn put_scratch(&mut self, mut scratch: Vec<Action>) {
        scratch.clear();
        self.scratch = scratch;
    }

    /// Run `bytes` through the machine: push every action onto `out` and
    /// return the mode the machine is left in.
    pub fn feed(&mut self, mut mode: Mode, bytes: &[u8], out: &mut Vec<Action>) -> Mode {
        let mut i = self.resume_carry(mode, bytes, out);
        let mut run = i;
        while i < bytes.len() {
            let b = bytes[i];
            if b == ESC {
                match mouse::scan(&bytes[i..]) {
                    mouse::Scan::Report(len) => {
                        if mode == Mode::Terminal {
                            if run < i {
                                out.push(Action::forward(run, i));
                            }
                            out.push(Action::Mouse {
                                start: i,
                                end: i + len,
                                wheel: mouse::wheel_of(&bytes[i..i + len]),
                            });
                        }
                        i += len;
                        run = i;
                        continue;
                    }
                    mouse::Scan::Partial => {
                        if mode == Mode::Terminal && run < i {
                            out.push(Action::forward(run, i));
                        }
                        self.carry.extend_from_slice(&bytes[i..]);
                        return mode;
                    }
                    mouse::Scan::Not => {}
                }
            }
            let prefix = self.bindings.prefix();
            let consumed = mode != Mode::Terminal || b == prefix;
            mode = match mode {
                Mode::Terminal if b == prefix => {
                    if run < i {
                        out.push(Action::forward(run, i));
                    }
                    Mode::Prefix
                }
                Mode::Terminal => Mode::Terminal,
                Mode::Prefix => self.prefix_key(b, i, out),
                Mode::Navigate => self.navigate_key(b, out),
                Mode::Copy => crate::copy::mode_after(b),
            };
            i += 1;
            if consumed {
                run = i;
            }
        }
        if mode == Mode::Terminal && run < bytes.len() {
            out.push(Action::forward(run, bytes.len()));
        }
        mode
    }

    /// One-shot prefix commands, looked up in the resolved table.
    ///
    /// `at` is the key's own position, so [`PrefixAction::Literal`] — the
    /// binding the prefix key always carries, which is the prefix-twice escape
    /// — can forward the byte in place without copying it.
    ///
    /// A key the table does not name is swallowed with the mode, exactly as an
    /// unknown key always was: a prefix layer that leaked its misses to the
    /// pane would make every typo a keystroke in a shell.
    fn prefix_key(&self, b: u8, at: usize, out: &mut Vec<Action>) -> Mode {
        let Some(action) = self.bindings.action(b) else {
            return Mode::Terminal;
        };
        // Exhaustive rather than a lookup with a fallback: a verb added to
        // `PrefixAction` and not to this match does not compile, which is the
        // only thing standing between "the action is in the table" and "the
        // action happens".
        match action {
            PrefixAction::Navigate => return Mode::Navigate,
            PrefixAction::Literal => out.push(Action::forward(at, at + 1)),
            PrefixAction::SplitHorizontal => out.push(Action::Split(SplitDirection::Horizontal)),
            PrefixAction::SplitVertical => out.push(Action::Split(SplitDirection::Vertical)),
            PrefixAction::Zoom => out.push(Action::Zoom),
            PrefixAction::Detach => out.push(Action::Detach),
            PrefixAction::Picker => out.push(Action::Picker),
            PrefixAction::NextAttention => out.push(Action::NextAttention),
        }
        Mode::Terminal
    }

    /// The sticky navigate layer.
    fn navigate_key(&mut self, b: u8, out: &mut Vec<Action>) -> Mode {
        if self.pending_swap {
            self.pending_swap = false;
            if let Some(dir) = focus_dir(b) {
                out.push(Action::Swap(dir));
                return Mode::Navigate;
            }
            // Anything else cancels the swap and keeps its normal meaning.
        }
        if b == ESC {
            return Mode::Terminal;
        }
        if let Some(dir) = focus_dir(b) {
            out.push(Action::Focus(dir));
        } else if let Some(dir) = resize_dir(b) {
            out.push(Action::Resize(dir));
        } else {
            match b {
                b'x' => out.push(Action::Split(SplitDirection::Horizontal)),
                b'v' => out.push(Action::Split(SplitDirection::Vertical)),
                crate::copy::ENTER => return Mode::Copy,
                b's' => self.pending_swap = true,
                b'm' => out.push(Action::MovePane),
                b'd' => out.push(Action::Close),
                b'1'..=b'9' => out.push(Action::Jump(b - b'0')),
                _ => {}
            }
        }
        Mode::Navigate
    }
}

/// A core direction as its wire form.
#[must_use]
pub fn wire_direction(dir: Direction) -> MoveDirection {
    match dir {
        Direction::Left => MoveDirection::Left,
        Direction::Down => MoveDirection::Down,
        Direction::Up => MoveDirection::Up,
        Direction::Right => MoveDirection::Right,
    }
}

fn focus_dir(b: u8) -> Option<Direction> {
    match b {
        b'h' => Some(Direction::Left),
        b'j' => Some(Direction::Down),
        b'k' => Some(Direction::Up),
        b'l' => Some(Direction::Right),
        _ => None,
    }
}

fn resize_dir(b: u8) -> Option<Direction> {
    match b {
        b'H' => Some(Direction::Left),
        b'J' => Some(Direction::Down),
        b'K' => Some(Direction::Up),
        b'L' => Some(Direction::Right),
        _ => None,
    }
}
