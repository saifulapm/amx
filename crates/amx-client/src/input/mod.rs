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
//! - **terminal** (default): bytes go to the focused pane; `ctrl+a` enters
//!   prefix.
//! - **prefix** (one-shot): `w` enters navigate, a second `ctrl+a` sends the
//!   literal byte to the pane, `x`/`v` split, `z` zooms, `d` detaches, `p`
//!   opens the picker; any other key is swallowed and the mode falls back to
//!   terminal. Closing a pane is navigate's `d`, deliberately not prefix's:
//!   the detach verb owns the prefix chord (04 §7 lists detach among the
//!   prefix one-shots) and a destroy verb must not sit one key from it.
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
//! SGR mouse reports are recognised in every mode and never interpreted:
//! in terminal mode they are forwarded verbatim iff the focused pane enabled
//! mouse reporting, in every other mode they are dropped whole — chrome
//! neither acts on a click nor mistakes a report's bytes for mode keys (D9).

mod mouse;

use std::collections::HashSet;

use amx_core::{Direction, PaneId};
use amx_proto::control::Call;
use amx_proto::control::pane::{MoveDirection, SplitDirection};

use crate::app::Mode;

/// The prefix key, `ctrl+a` (04 §7; configurable once config lands in M1).
pub const PREFIX: u8 = 0x01;

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
    /// `bytes[start..end]` is one whole SGR mouse report: forward it only if
    /// the focused pane enabled mouse reporting, otherwise drop it.
    Mouse {
        /// Start of the report.
        start: usize,
        /// One past its final byte.
        end: usize,
    },
    /// A report carried across a read boundary completed; its bytes are in
    /// [`Input::carried`]. Gated like [`Action::Mouse`].
    CarriedMouse,
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
    /// `s` was pressed in navigate; the next direction key names the swap.
    pending_swap: bool,
    /// Panes that enabled mouse reporting, per the server's pane state.
    mouse_panes: HashSet<PaneId>,
    /// An unterminated SGR report candidate carried across a read boundary.
    carry: Vec<u8>,
    /// The last resolved carry, kept readable until the next feed so the
    /// [`Action::CarriedMouse`]/[`Action::CarriedBytes`] it produced can be
    /// forwarded from it.
    done: Vec<u8>,
    /// Reused action buffer, so a keystroke allocates nothing steady-state.
    scratch: Vec<Action>,
}

impl Input {
    /// A machine in terminal mode with nothing carried.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record whether `pane` has mouse reporting enabled.
    ///
    /// Fed from pane state updates; until the event subscription that would
    /// deliver those exists, tests (and the future state path) call it
    /// directly.
    pub fn set_mouse_reporting(&mut self, pane: PaneId, enabled: bool) {
        if enabled {
            self.mouse_panes.insert(pane);
        } else {
            self.mouse_panes.remove(&pane);
        }
    }

    /// Whether `pane` has mouse reporting enabled.
    #[must_use]
    pub fn mouse_enabled(&self, pane: PaneId) -> bool {
        self.mouse_panes.contains(&pane)
    }

    /// The bytes behind the latest [`Action::CarriedMouse`] or
    /// [`Action::CarriedBytes`]. Valid until the next [`Input::feed`].
    #[must_use]
    pub fn carried(&self) -> &[u8] {
        &self.done
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
            let consumed = mode != Mode::Terminal || b == PREFIX;
            mode = match mode {
                Mode::Terminal if b == PREFIX => {
                    if run < i {
                        out.push(Action::forward(run, i));
                    }
                    Mode::Prefix
                }
                Mode::Terminal => Mode::Terminal,
                Mode::Prefix => Self::prefix_key(b, i, out),
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

    /// Continue an SGR report candidate left over from the previous read.
    /// Returns how many of `bytes` it consumed.
    fn resume_carry(&mut self, mode: Mode, bytes: &[u8], out: &mut Vec<Action>) -> usize {
        if self.carry.is_empty() {
            return 0;
        }
        for (at, &b) in bytes.iter().enumerate() {
            if mouse::param_byte(b) && self.carry.len() < mouse::MAX_REPORT {
                self.carry.push(b);
                continue;
            }
            return if b == b'M' || b == b'm' {
                self.carry.push(b);
                self.resolve_carry(mode, Action::CarriedMouse, out);
                at + 1
            } else {
                // Not a report after all: release what was held, and let the
                // byte that broke the pattern keep its normal meaning.
                self.resolve_carry(mode, Action::CarriedBytes, out);
                at
            };
        }
        bytes.len()
    }

    /// Move a finished carry into [`Self::carried`] and, in terminal mode,
    /// emit the action that forwards it; chrome modes swallow it whole.
    fn resolve_carry(&mut self, mode: Mode, action: Action, out: &mut Vec<Action>) {
        std::mem::swap(&mut self.done, &mut self.carry);
        self.carry.clear();
        if mode == Mode::Terminal {
            out.push(action);
        }
    }

    /// One-shot prefix commands. `at` is the key's own position, so the
    /// literal-prefix escape (`ctrl+a` twice) can forward the byte in place.
    fn prefix_key(b: u8, at: usize, out: &mut Vec<Action>) -> Mode {
        match b {
            PREFIX => {
                out.push(Action::forward(at, at + 1));
                Mode::Terminal
            }
            b'w' => Mode::Navigate,
            b'x' => {
                out.push(Action::Split(SplitDirection::Horizontal));
                Mode::Terminal
            }
            b'v' => {
                out.push(Action::Split(SplitDirection::Vertical));
                Mode::Terminal
            }
            b'z' => {
                out.push(Action::Zoom);
                Mode::Terminal
            }
            b'd' => {
                out.push(Action::Detach);
                Mode::Terminal
            }
            b'p' => {
                out.push(Action::Picker);
                Mode::Terminal
            }
            _ => Mode::Terminal,
        }
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
