//! `amx` on its own — the front door.
//!
//! Two doors, and what decides between them is not the command line but
//! whether anybody is looking. A program reading amx's output wants the table
//! and nothing else. A person at a terminal wants the view, and it is drawn on
//! the terminal they typed the command in.
//!
//! Which tmux that terminal is inside, or whether it is inside one at all,
//! decides nothing here. The view is a program on a screen; it needs a screen
//! and no more than that. The agents are elsewhere either way — each of them a
//! session of its own, which is what `enter` on a row reaches.

use anyhow::Result;
use std::io::IsTerminal;

use crate::config::Config;
use crate::store::now;
use crate::{paths, tui, verbs};

/// Which door bare `amx` opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Door {
    /// Nobody is reading a screen: the table, once.
    Table,
    /// Somebody is: the view, on the terminal they typed it at.
    View,
}

/// The door, given whether anybody is looking at a terminal.
pub fn door(terminal: bool) -> Door {
    match terminal {
        true => Door::View,
        false => Door::Table,
    }
}

/// Open the front door against the machine.
pub fn from_env(config: &Config) -> Result<i32> {
    let root = paths::state_root()?;

    match door(std::io::stdout().is_terminal()) {
        Door::Table => verbs::ls::run(&root, false, now(), &mut std::io::stdout().lock()),
        Door::View => tui::run(&root, config),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cockpit_the_door_is_chosen_by_whether_anybody_is_looking() {
        // Something is reading the output, and reading is all it does.
        assert_eq!(door(false), Door::Table);

        // Somebody is at a terminal, and the list is drawn on it.
        assert_eq!(door(true), Door::View);
    }
}
