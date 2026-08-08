//! Which terminal modes cross a handoff, and which deliberately do not.
//!
//! The inventory is data — [`CARRIED_MODES`] — because the interesting part is
//! the exclusions, and a list with a reason written beside each omission is the
//! only form in which those reasons survive.

use amx_vt::{Mode, Screen, Terminal};
use serde::{Deserialize, Serialize};

use super::{ManifestError, terminal_error};

/// One terminal mode and whether it was on.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct ModeState {
    /// The mode number, as the VT specifications spell it.
    pub number: u16,
    /// Whether it is an ANSI mode rather than a DEC private one.
    pub ansi: bool,
    /// Whether it was set.
    pub on: bool,
}

/// A pane's modes, keyboard flags and title.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct PaneModes {
    /// Whether the alternate screen was the active one.
    ///
    /// Separate from [`modes`](Self::modes) because it decides *which screen*
    /// the carried grid describes, so it is applied before the paint and not
    /// with the rest.
    pub alternate: bool,
    /// Every carried mode, in [`CARRIED_MODES`] order.
    pub modes: Vec<ModeState>,
    /// The Kitty keyboard flags the application asked for.
    pub kitty_flags: u8,
    /// The title set by OSC 0 or OSC 2, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

impl PaneModes {
    /// Read every carried mode off a terminal.
    ///
    /// A mode this build of the library does not know is left out rather than
    /// guessed at: the successor runs the same library, so a mode neither side
    /// knows is a mode neither side has.
    pub(super) fn capture(terminal: &Terminal) -> Result<Self, ManifestError> {
        let mut modes = Vec::with_capacity(CARRIED_MODES.len());
        for &(number, ansi) in CARRIED_MODES {
            let mode = if ansi {
                Mode::ansi(number)
            } else {
                Mode::dec(number)
            };
            if let Ok(on) = terminal.mode(mode) {
                modes.push(ModeState { number, ansi, on });
            }
        }
        let screen = terminal.active_screen().map_err(terminal_error)?;
        Ok(Self {
            alternate: screen == Screen::Alternate,
            modes,
            kitty_flags: terminal
                .kitty_keyboard_flags()
                .map_err(terminal_error)?
                .bits(),
            title: terminal.title().map_err(terminal_error)?,
        })
    }

    /// Whether a carried mode was set, or `missing` if it was not carried.
    #[must_use]
    pub fn is_set(&self, number: u16, missing: bool) -> bool {
        self.modes
            .iter()
            .find(|mode| mode.number == number && !mode.ansi)
            .map_or(missing, |mode| mode.on)
    }
}

/// The terminal modes a pane carries across a handoff.
///
/// Every mode the vendored library knows (`terminal/modes.zig`) except four
/// classes, each excluded for a reason rather than an oversight:
///
/// - **132-column (3)** and **enable-mode-3 (40)** resize the grid, and the
///   successor's pane is built at the size the manifest already states;
/// - **left-and-right margins (69)** installs a scrolling region, which the C
///   API gives no way to read back — carrying the switch without the region
///   would be worse than carrying neither;
/// - **the screen switches (47, 1047, 1048, 1049)** are the alternate screen
///   itself, which [`PaneModes::alternate`] carries and the paint prologue
///   applies before the grid rather than after it.
const CARRIED_MODES: &[(u16, bool)] = &[
    (2, true),
    (4, true),
    (12, true),
    (20, true),
    (1, false),
    (4, false),
    (5, false),
    (6, false),
    (7, false),
    (8, false),
    (9, false),
    (12, false),
    (25, false),
    (45, false),
    (66, false),
    (67, false),
    (1000, false),
    (1002, false),
    (1003, false),
    (1004, false),
    (1005, false),
    (1006, false),
    (1007, false),
    (1015, false),
    (1016, false),
    (1035, false),
    (1036, false),
    (1039, false),
    (1045, false),
    (2004, false),
    (2026, false),
    (2027, false),
    (2031, false),
    (2048, false),
];
