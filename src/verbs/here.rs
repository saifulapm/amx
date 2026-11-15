//! `amx here`: start a claude in the current directory under amx, then attach
//! this terminal to its pane the way `amx attach` does.

use anyhow::{Context, Result};

use crate::cli::NewArgs;
use crate::exit;
use crate::paths;
use crate::spawn;
use crate::verbs::{attach, new};

/// Runs the verb from the command line.
pub fn from_env(vendor_args: &[String]) -> Result<i32> {
    // The same start as `amx new --no-worktree` with no task, so the agent
    // takes its place under the caps and the config like any other.
    let args = NewArgs {
        task: None,
        file: None,
        edit: false,
        name: None,
        role: None,
        dir: None,
        no_worktree: true,
        base: None,
        branch: None,
        pr: None,
        with_changes: false,
        exec: false,
        check: false,
        agent: None,
        vendor_args: vendor_args.to_vec(),
        context_brief: None,
        parent: None,
    };
    let mut said = Vec::new();
    let code = new::from_env_into(&args, &mut said)?;
    if code != exit::OK {
        return Ok(code);
    }
    let said = String::from_utf8(said).context("reading the new agent's id")?;
    let id = said.trim();

    let root = paths::state_root()?;
    let env = spawn::env_snapshot(std::env::vars());
    let inside = std::env::var("TMUX").ok().filter(|v| !v.is_empty());
    attach::run(&root, id, &env, inside.as_deref())
}
