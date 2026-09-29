//! Bare `amx`: the view when stdout is a terminal, the `ls` table otherwise.
//!
//! The view runs in whatever terminal amx was started in, inside tmux or not;
//! each agent lives in a tmux session of its own.

use anyhow::Result;
use std::io::IsTerminal;
use std::path::Path;

use crate::config::{self, Config};
use crate::store::now;
use crate::verbs::ls::Scope;
use crate::{paths, tui, verbs};

/// What bare `amx` shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Door {
    /// The `ls` table, printed once.
    Table,
    /// The interactive view.
    View,
}

/// The view on a terminal, the table otherwise.
pub fn door(terminal: bool) -> Door {
    match terminal {
        true => Door::View,
        false => Door::Table,
    }
}

/// Run bare `amx`, limited to agents under `dir` when one is given.
///
/// A view limited to a directory runs under that project's config, and its
/// header counts agents against the project's `max_agents`. An unlimited view
/// runs under the person's config and counts against `max_total`. Warnings
/// from the project's config are not shown; `main` has printed the person's.
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
        assert_eq!(door(false), Door::Table);
        assert_eq!(door(true), Door::View);
    }
}
