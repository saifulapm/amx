//! `amx new` — start an agent on a task.
//!
//! The order matters and is the same every time: mint an id, cut a worktree,
//! write the handoff, start the pane, then write the record. The pane waits
//! for the record before it starts the vendor, so the vendor's first hook
//! always has somewhere to go, and nothing but the id is ever printed — a
//! caller reads that id straight into its next command.

use anyhow::{Context, Result, bail};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::cli::NewArgs;
use crate::config::Config;
use crate::spawn::{self, Handoff, Placement};
use crate::store::{Meta, now};
use crate::{exit, ids, paths, worktree};

/// Run the verb against the machine.
pub fn from_env(config: &Config, args: &NewArgs) -> Result<i32> {
    let root = paths::state_root()?;
    let dir = match &args.dir {
        Some(dir) => dir.clone(),
        None => std::env::current_dir().context("no working directory")?,
    };
    let env = spawn::env_snapshot(std::env::vars());
    let mut out = std::io::stdout().lock();
    let mut problems = std::io::stderr().lock();

    run(&root, &dir, env, config, args, &mut out, &mut problems)
}

/// The verb, with everything it reads named.
#[allow(clippy::too_many_arguments)]
pub fn run(
    root: &Path,
    dir: &Path,
    env: std::collections::BTreeMap<String, String>,
    config: &Config,
    args: &NewArgs,
    out: &mut impl Write,
    problems: &mut impl Write,
) -> Result<i32> {
    std::fs::create_dir_all(root).with_context(|| format!("creating {}", root.display()))?;

    // The cap is about agents that are still going. One that has finished is a
    // record, not a running program.
    let live = spawn::live(root)?;
    if live.len() >= config.max_agents {
        writeln!(
            problems,
            "amx new: {} agents already running, and max_agents is {}",
            live.len(),
            config.max_agents
        )?;
        return Ok(exit::BLOCKED);
    }

    let id = match &args.name {
        Some(name) => {
            ids::validate_name(name, root)?;
            name.clone()
        }
        None => ids::generate(&args.task, root)?,
    };

    let agent_dir = paths::agent_dir_in(root, &id)?;
    make_dir(&agent_dir)?;

    // From here on a failure leaves nothing behind: an id that half exists is
    // worse than one that does not.
    match start(root, &agent_dir, dir, env, config, args, &id) {
        Ok(()) => {
            writeln!(out, "{id}")?;
            Ok(exit::OK)
        }
        Err(e) => {
            let _ = std::fs::remove_dir_all(&agent_dir);
            Err(e)
        }
    }
}

fn start(
    root: &Path,
    agent_dir: &Path,
    dir: &Path,
    mut env: std::collections::BTreeMap<String, String>,
    config: &Config,
    args: &NewArgs,
    id: &str,
) -> Result<()> {
    let tree = cut_worktree(dir, id, config, args)?;
    let cwd = tree
        .as_ref()
        .map(|tree| tree.path.clone())
        .unwrap_or_else(|| dir.to_path_buf());

    let agent = args.agent.clone().unwrap_or_else(|| config.agent.clone());
    env.insert(crate::hook::ID_ENV.to_string(), id.to_string());
    spawn::write_handoff(
        agent_dir,
        &Handoff {
            task: args.task.clone(),
            command: spawn::vendor_command(&agent, &args.vendor_args, &args.task),
            env,
        },
    )?;

    let server = spawn::server(root)?;
    let placement = if args.bg {
        Placement::Background
    } else {
        Placement::Wall
    };
    let boot = vec![
        std::env::current_exe()?.to_string_lossy().into_owned(),
        "_boot".to_string(),
        id.to_string(),
    ];
    let pane = spawn::place(&server, placement, id, &cwd, &boot, &root.join("wall.lock"))?;

    spawn::record(
        root,
        &Meta {
            id: id.to_string(),
            task: args.task.clone(),
            dir: cwd,
            worktree: tree.as_ref().map(|tree| tree.path.clone()),
            branch: tree.as_ref().map(|tree| tree.branch.clone()),
            base: tree.as_ref().map(|tree| tree.base.clone()),
            socket: server.socket().clone(),
            pane,
            session: None,
            transcript: None,
            created: now(),
        },
    )?;
    Ok(())
}

/// A worktree of its own, when the agent is being sent into a repository and
/// nobody has said not to.
fn cut_worktree(
    dir: &Path,
    id: &str,
    config: &Config,
    args: &NewArgs,
) -> Result<Option<worktree::Worktree>> {
    if args.no_worktree || !config.worktrees {
        return Ok(None);
    }
    if !dir.is_dir() {
        bail!("{} is not a directory to run in", dir.display());
    }
    // Somewhere that is not a repository is somewhere to work in as it is.
    let Some(repo) = worktree::repo_root(dir)? else {
        return Ok(None);
    };
    Ok(Some(worktree::create(&repo, id)?))
}

/// The agent's own directory, which nobody else has any business reading.
fn make_dir(dir: &PathBuf) -> Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .with_context(|| format!("creating {}", dir.display()))
}
