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
use std::path::Path;

use crate::config::{self, Config};
use crate::store::now;
use crate::verbs::ls::Scope;
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
///
/// The directory, when one is named, is the question rather than the door: it
/// narrows what is behind both of them to the agents working under that
/// directory, so `amx --dir /srv/app` is that project's agents drawn on a
/// terminal and that project's agents down a pipe.
///
/// It decides one more thing for the view. A view about a project is opened
/// under that project's own file — the dials it launches at, the palette it
/// paints in and the cap it counts against are all the project's, laid over
/// the person's — and a view about every agent on the machine is opened under
/// the person's file alone, because no project's file speaks for a machine.
/// What is wrong with a project's file is not the view's to say: `main` has
/// already said whatever the person's file had to answer for, and a project's
/// is read here the way every other reader reads one.
pub fn from_env(config: &Config, dir: Option<&Path>) -> Result<i32> {
    let root = paths::state_root()?;
    let scope = Scope::of(dir)?;

    match (door(std::io::stdout().is_terminal()), dir) {
        (Door::Table, _) => {
            verbs::ls::run(&root, false, &scope, now(), &mut std::io::stdout().lock())
        }
        (Door::View, Some(dir)) => {
            let (theirs, _) = config::for_dir(dir);
            tui::run(&root, &theirs, &scope, Some(theirs.max_agents))
        }
        (Door::View, None) => tui::run(&root, config, &scope, config.max_total),
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
