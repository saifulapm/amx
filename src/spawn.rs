//! Starting an agent: placing its pane and handing it what it runs.
//!
//! An agent is one detached tmux session, `amx-<id>`, on the server the person
//! already uses; `new-session -d` never moves anybody's screen. Nothing stays
//! resident: the pane runs `amx _boot <id>`, which reads the handoff, restores
//! the environment and execs the vendor with `amx _exit` after it.
//!
//! - The task and the environment travel in owner-only files, never on the
//!   tmux command line, where they could be read as syntax. The handoff keeps
//!   the task for later verbs; the environment file is read once by `_boot`
//!   and removed.
//! - The environment is the one `new` ran in, since the tmux server's own can
//!   be hours old.
//! - `AMX_BIN`, `AMX_ID` and `AMX_AGENT_DIR` are set over the snapshot, so an
//!   agent spawned from another agent's pane gets its own values.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::registry;
use crate::store::{Agent, Meta, Phase};
use crate::tmux::{PaneId, PaneOwners, Server, Socket, Spawn};
use crate::vendor::Vendor;

/// File holding the [`Handoff`].
pub const HANDOFF: &str = "handoff.json";

/// File holding the environment for `_boot`, removed once read.
pub const BOOT_ENV: &str = "boot-env.json";

/// Variable naming the agent's scratch directory.
pub const AGENT_DIR_ENV: &str = "AMX_AGENT_DIR";

/// Variable naming the agent's record directory.
///
/// amx's extensions inside the pane stream the agent's output there; see
/// `store::LIVE`.
pub const RECORD_DIR_ENV: &str = "AMX_DIR";

/// Variable naming the agent whose pane a child was started in.
pub const PARENT_ENV: &str = "AMX_PARENT";

/// Variable naming the parent's record directory, for `amx logs $AMX_PARENT`.
pub const PARENT_DIR_ENV: &str = "AMX_PARENT_DIR";

/// Variable giving an agent's depth: 0 for a root, 1 for its child.
pub const DEPTH_ENV: &str = "AMX_DEPTH";

/// Name of the scratch directory inside the agent's record directory.
const SCRATCH: &str = "scratch";

/// tmux's default socket name, the server a bare `tmux` reaches.
const DEFAULT_SOCKET: &str = "default";

/// Test-only override of the socket agents are placed on.
const SOCKET_ENV: &str = "AMX_TMUX_SOCKET";

/// Variables describing the pane a command was typed in, left out of the
/// snapshot.
///
/// Vendor session markers come from the registry instead; see
/// [`env_snapshot`].
const NOT_INHERITED: [&str; 4] = ["TMUX", "TMUX_PANE", "PWD", "OLDPWD"];

/// How long `_boot` waits for its record to appear.
const RECORD_PATIENCE: Duration = Duration::from_secs(10);

/// How many bytes of an agent's pane `_boot` keeps in the record.
///
/// A shell command's output is kept whole. A vendor's pane is a full-screen
/// drawing that grows without bound, so only the start is kept: a vendor that
/// dies before its first hook says why there (pi's bad-model error is the
/// first 96 bytes).
pub const BOOT_BYTES: u64 = 64 * 1024;

/// What a pane is started with, apart from the environment.
///
/// It outlives the boot that reads it, so the environment goes in
/// [`BOOT_ENV`] instead.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Handoff {
    /// The task the agent was given.
    pub task: String,
    /// The vendor argv.
    pub command: Vec<String>,
}

/// The environment an agent inherits from the one `new` ran in.
///
/// Drops the calling pane's variables and every vendor's session markers: a
/// vendor that sees its spawner's markers thinks it is a child session and
/// keeps no transcript.
pub fn env_snapshot(vars: impl IntoIterator<Item = (String, String)>) -> BTreeMap<String, String> {
    vars.into_iter()
        .filter(|(name, _)| !NOT_INHERITED.contains(&name.as_str()) && !marks_a_session(name))
        .collect()
}

/// Every vendor's session-marker variables.
///
/// All vendors, since the caller could be inside any of them.
fn session_markers() -> impl Iterator<Item = &'static str> {
    registry::entries()
        .iter()
        .flat_map(|vendor| vendor.not_inherited.iter().copied())
}

/// Whether `name` is some vendor's session marker.
fn marks_a_session(name: &str) -> bool {
    session_markers().any(|marker| marker == name)
}

/// Lay the config's harness table for `agent`'s program over `env`.
///
/// A table entry replaces the snapshot's value. Must run before amx's own
/// variables go in, or a table could change [`crate::hook::ID_ENV`].
pub fn harness_env(
    env: &mut BTreeMap<String, String>,
    config: &crate::config::Config,
    agent: &str,
) {
    for (name, value) in config.harness(registry::program(agent)).env {
        env.insert(name, value);
    }
}

/// The model, permission and effort for a spawn, each a vendor value or
/// [`registry::DEFAULT`].
///
/// `new` resolves them; [`vendor_command`] turns them into flags.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dials {
    pub model: String,
    pub permission: String,
    pub effort: String,
}

impl Default for Dials {
    /// Every dial at [`registry::DEFAULT`], which sends no flag.
    fn default() -> Dials {
        Dials {
            model: registry::DEFAULT.to_string(),
            permission: registry::DEFAULT.to_string(),
            effort: registry::DEFAULT.to_string(),
        }
    }
}

/// The vendor argv for a spawn.
///
/// The configured command and launch words, the turned dials, the session and
/// trust flags, `vendor_args`, the end-of-options word and the task.
///
/// `session` is the id offered to a vendor with a start flag
/// ([`opens_under_id`] answers that without building the argv). `trust` is the
/// config key; without it no trust flag is written. Every flag amx adds yields
/// to one already on the command line, so the vendor never gets a flag twice.
pub fn vendor_command(
    agent: &str,
    dials: &Dials,
    vendor_args: &[String],
    task: &str,
    session: Option<&str>,
    trust: bool,
) -> Vec<String> {
    let command: Vec<String> = agent.split_whitespace().map(str::to_string).collect();
    let mut command = with_launch_words(command, registry::entry(agent), vendor_args);
    let carried: Vec<&str> = command.iter().skip(1).map(String::as_str).collect();

    let mut args = session_flag(registry::entry(agent), &carried, vendor_args, session);
    args.extend(trust_flag(agent, &carried, vendor_args, trust));
    args.extend(vendor_args.iter().cloned());

    command.extend(registry::inject(
        agent,
        &dials.model,
        &dials.permission,
        &dials.effort,
        &args,
    ));
    command.extend(
        registry::entry(agent)
            .and_then(|vendor| vendor.ends_options)
            .map(str::to_string),
    );
    command.push(as_words(registry::entry(agent), task));
    command
}

/// `command` with the vendor's launch words inserted after the program,
/// skipping any already in `command` or `vendor_args`.
///
/// `resume` and `fork` keep those words, so they never end up doubled.
fn with_launch_words(
    mut command: Vec<String>,
    vendor: Option<&Vendor>,
    vendor_args: &[String],
) -> Vec<String> {
    let Some(vendor) = vendor else {
        return command;
    };
    let carried: Vec<&str> = command.iter().skip(1).map(String::as_str).collect();
    let missing: Vec<String> = vendor
        .launch
        .iter()
        .filter(|word| !already(word, &carried, vendor_args))
        .map(|word| word.to_string())
        .collect();
    drop(command.splice(1..1, missing));
    command
}

/// The recorded vendor's end-of-options word, for a verb appending a message
/// to its argv.
pub fn ends_options_of(handoff: &Handoff) -> Option<&'static str> {
    vendor_of(handoff)?.ends_options
}

/// A message as the argv word handed to `vendor`.
///
/// Typed as [`as_typed`] types it, with a leading space where the vendor reads
/// a leading `@` as a file to attach ([`Vendor::attaches_at`]), and prefixed
/// with `<prompt_flag>=` where the vendor has a [`Vendor::prompt_flag`].
pub fn as_words(vendor: Option<&Vendor>, message: &str) -> String {
    let typed = as_typed(vendor, message);
    let typed = if vendor.is_some_and(|vendor| vendor.attaches_at) && message.starts_with('@') {
        format!(" {typed}")
    } else {
        typed
    };
    match vendor.and_then(|vendor| vendor.prompt_flag) {
        Some(flag) => format!("{flag}={typed}"),
        None => typed,
    }
}

/// A message as typed into `vendor`'s composer, with a trailing space where
/// its last word would open a popup ([`Vendor::popups`]).
pub fn as_typed(vendor: Option<&Vendor>, message: &str) -> String {
    let last = message
        .rsplit(char::is_whitespace)
        .next()
        .unwrap_or_default();
    let popups = vendor.map(|vendor| vendor.popups).unwrap_or_default();
    match last.chars().next() {
        Some(first) if popups.contains(&first) => format!("{message} "),
        _ => message.to_string(),
    }
}

/// The vendor's session start flag, or `None` when it has none or the command
/// line already carries it or a flag that conflicts with it.
fn start_flag(
    vendor: Option<&Vendor>,
    carried: &[&str],
    vendor_args: &[String],
) -> Option<&'static str> {
    let session = vendor?.session?;
    let start = session.start?;
    let present = |flag: &str| already(flag, carried, vendor_args);
    if present(start) || session.conflicts.iter().any(|conflict| present(conflict)) {
        return None;
    }
    Some(start)
}

/// Whether `flag` is on the command line, alone or as `flag=value`, in the
/// configured command or in `vendor_args`.
///
/// The same reading as `vendor::already` and `resume::names_a_session`.
fn already(flag: &str, carried: &[&str], vendor_args: &[String]) -> bool {
    let joined = format!("{flag}=");
    carried
        .iter()
        .any(|arg| *arg == flag || arg.starts_with(&joined))
        || vendor_args
            .iter()
            .any(|arg| arg == flag || arg.starts_with(&joined))
}

/// The folder-trust flag to add, if any.
///
/// `None` without consent, for a vendor amx cannot answer or answers with a
/// store write, and when any spelling that settles the question is already on
/// the command line.
fn trust_flag(
    agent: &str,
    carried: &[&str],
    vendor_args: &[String],
    trust: bool,
) -> Option<String> {
    if !trust {
        return None;
    }
    let flag = crate::trust::flag_for(agent)?;
    let settled = flag
        .settled
        .iter()
        .any(|written| already(written, carried, vendor_args));
    (!settled).then(|| flag.send.to_string())
}

/// The session flag and the id it opens, or nothing.
///
/// Separate from [`vendor_command`] so a test can check the two land together.
fn session_flag(
    vendor: Option<&Vendor>,
    carried: &[&str],
    vendor_args: &[String],
    session: Option<&str>,
) -> Vec<String> {
    let mut args = Vec::new();
    if let Some(id) = session
        && let Some(flag) = start_flag(vendor, carried, vendor_args)
    {
        args.push(flag.to_string());
        args.push(id.to_string());
    }
    args
}

/// Whether the spawn opens its session under the amx id, so the record can
/// carry [`Meta::session`] from the start.
///
/// [`Meta::session`]: crate::store::Meta::session
pub fn opens_under_id(agent: &str, vendor_args: &[String]) -> bool {
    let carried: Vec<&str> = agent.split_whitespace().skip(1).collect();
    start_flag(registry::entry(agent), &carried, vendor_args).is_some()
}

/// The argv for a shell command row: `sh -c <command>`.
///
/// Passed whole, so pipelines, redirects and `&&` run as one row with one exit
/// code. `sh` rather than the login shell, so it behaves the same everywhere.
pub fn exec_command(command: &str) -> Vec<String> {
    vec!["sh".to_string(), "-c".to_string(), command.to_string()]
}

/// Write the handoff, readable by the owner only.
pub fn write_handoff(dir: &Path, handoff: &Handoff) -> Result<()> {
    write_owned(&dir.join(HANDOFF), handoff)
}

/// Write the environment for `_boot`, readable by the owner only.
pub fn write_boot_env(dir: &Path, env: &BTreeMap<String, String>) -> Result<()> {
    write_owned(&dir.join(BOOT_ENV), env)
}

/// Write `value` as pretty JSON, atomically and owner-only.
fn write_owned<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value).context("writing the record")?;
    bytes.push(b'\n');
    crate::store::write_atomic(path, &bytes)
}

/// The agent's scratch directory, created if missing.
///
/// It sits beside the record files and goes when the agent's directory does,
/// so anything worth keeping belongs in the worktree.
pub fn scratch(agent_dir: &Path) -> Result<PathBuf> {
    use std::os::unix::fs::DirBuilderExt;

    let dir = agent_dir.join(SCRATCH);
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(crate::paths::DIR_MODE)
        .create(&dir)
        .with_context(|| format!("creating {}", dir.display()))?;
    crate::paths::keep_to_the_owner(&dir, crate::paths::DIR_MODE)?;
    Ok(dir)
}

/// The registry entry for the program the handoff's command names.
///
/// `None` for a program amx has no entry for (a wrapper, an unknown vendor)
/// and for an empty command.
pub fn vendor_of(handoff: &Handoff) -> Option<&'static Vendor> {
    registry::entry(handoff.command.first()?)
}

pub fn read_handoff(dir: &Path) -> Result<Handoff> {
    let path = dir.join(HANDOFF);
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("reading {}", path.display()))
}

/// Read the boot environment and remove the file.
fn take_boot_env(dir: &Path) -> Result<BTreeMap<String, String>> {
    let path = dir.join(BOOT_ENV);
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let env = serde_json::from_str(&text).with_context(|| format!("reading {}", path.display()))?;
    std::fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
    Ok(env)
}

/// The server agents are placed on: the one this process is inside, else the
/// default socket.
///
/// No conf is passed: the server is the person's and reads their config.
pub fn server() -> Result<Server> {
    if let Some(inside) = std::env::var("TMUX").ok().filter(|v| !v.is_empty())
        && let Some(server) = Server::from_tmux_env(&inside)
    {
        return Ok(server);
    }

    let socket = std::env::var(SOCKET_ENV)
        .ok()
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| DEFAULT_SOCKET.to_string());
    Ok(Server::named(socket))
}

/// The name of the session holding agent `id`.
///
/// Built from [`crate::tmux::SESSION_PREFIX`], which tmux.rs reads back to
/// tell whose a pane is.
fn session_name(id: &str) -> String {
    format!("{}{id}", crate::tmux::SESSION_PREFIX)
}

/// Kill the session [`place`] made for `id`, if it is still there, freeing
/// the name for the next agent with that id.
pub fn end_session(server: &Server, id: &str) -> Result<()> {
    if let Some(session) = server.session_named(&session_name(id))? {
        server.kill_session(&session)?;
    }
    Ok(())
}

/// Start the agent's pane in a detached session named for `id`.
///
/// Detached so the spawn never switches anybody's client. A tmux older than
/// [`crate::tmux::MINIMUM_VERSION`] is refused before anything is created.
pub fn place(server: &Server, id: &str, cwd: &Path, command: &[String]) -> Result<PaneId> {
    meets_the_floor(crate::tmux::version()?)?;
    let name = session_name(id);
    let command: Vec<&str> = command.iter().map(String::as_str).collect();
    let (session, pane) = server.new_session(&Spawn {
        name: Some(&name),
        cwd: Some(cwd),
        command: &command,
        ..Spawn::default()
    })?;

    // Otherwise tmux destroys the session when a client detaches from it.
    server.set_session_option(&session, "destroy-unattached", "off")?;
    // A tmux.conf with remain-on-exit would keep the dead pane and with it the
    // session name, which a resume needs free.
    server.set_session_option(&session, "remain-on-exit", "off")?;
    // A detached pane has no terminal to answer background colour queries, so
    // tmux answers from window-style: the colours the view last read.
    if let Some(style) = crate::shade::remembered(&crate::paths::state_root()?) {
        server.set_session_option(&session, "window-style", &style)?;
    }
    Ok(pane)
}

/// Refuse a tmux older than [`crate::tmux::MINIMUM_VERSION`].
fn meets_the_floor((major, minor): (u32, u32)) -> Result<()> {
    let (want_major, want_minor) = crate::tmux::MINIMUM_VERSION;
    if (major, minor) < (want_major, want_minor) {
        bail!(
            "tmux {major}.{minor} is installed and amx needs tmux {want_major}.{want_minor} or newer"
        );
    }
    Ok(())
}

/// `amx _boot <id>`: become the agent.
///
/// `new` writes the record only after the pane exists, so this waits for it;
/// otherwise the vendor's first hooks would have nowhere to go.
pub fn boot(root: &Path, id: &str) -> Result<i32> {
    use std::os::unix::process::CommandExt;

    let dir = crate::paths::agent_dir_in(root, id)?;
    wait_for(&dir.join("meta.json"))?;
    let meta = Agent::open(root, id)?.meta()?;
    let handoff = read_handoff(&dir)?;
    let env = take_boot_env(&dir)?;

    let Some(vendor) = handoff.command.first() else {
        bail!("the handoff for {id} names no command to run");
    };

    // Before the exec, so the first bytes are captured. A pipe that fails
    // costs the output file only; the command still runs.
    if let Err(e) = keep_output(&meta, &keeping_output(&meta, &dir)) {
        crate::warn!("amx: {id}: what it prints will not be kept: {e:#}");
    }

    let mut command = std::process::Command::new("sh");
    command
        // `"$0" "$@"` keeps the task out of the shell's parsing; `_exit`
        // records the exit code.
        .arg("-c")
        .arg(r#""$0" "$@"; "$AMX_BIN" _exit "$AMX_ID" $?"#)
        .arg(vendor)
        .args(&handoff.command[1..]);

    // The pane inherits the tmux server's environment, which can hold a
    // vendor's session markers from whenever the server started. The snapshot
    // never saw them, so strip them here.
    for marker in session_markers() {
        command.env_remove(marker);
    }

    let parent = meta
        .parent
        .as_deref()
        .and_then(|parent| crate::paths::agent_dir_in(root, parent).ok());
    for (name, value) in pane_env(
        &env,
        &std::env::current_exe()?,
        id,
        &dir,
        &scratch(&dir)?,
        meta.parent.as_deref().zip(parent.as_deref()),
        meta.depth,
    ) {
        command.env(name, value);
    }

    // Exec, so the pane's process is the vendor itself.
    Err(command.exec()).context("starting the agent's command")
}

/// The shell command that pipes a pane into its output file.
///
/// A shell command's output is appended whole, since a resume adds to the
/// same record. An agent's first [`BOOT_BYTES`] overwrite the file, since a
/// resume is a new boot.
fn keeping_output(meta: &Meta, dir: &Path) -> String {
    let path = quoted(&dir.join(crate::store::OUTPUT));
    match meta.agent.is_none() {
        true => format!("cat >> {path}"),
        false => format!("head -c {BOOT_BYTES} > {path}"),
    }
}

/// Pipe this pane's output through `command` with tmux `pipe-pane`.
///
/// The pane and server come from `$TMUX_PANE` and `$TMUX`: on a resume the
/// record still names the old pane until tmux has made the new one. The
/// record's socket is the fallback server. `command` runs under the server's
/// `sh`, so paths go in through [`quoted`].
fn keep_output(meta: &Meta, command: &str) -> Result<()> {
    let pane = std::env::var("TMUX_PANE").context("reading $TMUX_PANE")?;
    let server = std::env::var("TMUX")
        .ok()
        .and_then(|inside| Server::from_tmux_env(&inside))
        .unwrap_or_else(|| Server::from_socket(meta.socket.clone()));
    server.pipe_pane(&PaneId::new(pane)?, command)
}

/// `path` single-quoted as one shell word.
fn quoted(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', r"'\''"))
}

/// The pane's environment: the snapshot with amx's per-pane variables set
/// over it.
///
/// The snapshot often comes from another agent's pane, so the id, record and
/// scratch variables are always replaced, and the parent variables are set
/// from `parent` or removed. [`crate::hook::NESTED_ENV`] is always removed:
/// this pane is an agent, not a shell nested in one.
fn pane_env(
    snapshot: &BTreeMap<String, String>,
    bin: &Path,
    id: &str,
    record: &Path,
    scratch: &Path,
    parent: Option<(&str, &Path)>,
    depth: u32,
) -> BTreeMap<String, String> {
    let mut env = snapshot.clone();
    env.remove(crate::hook::NESTED_ENV);
    env.insert("AMX_BIN".to_string(), bin.to_string_lossy().into_owned());
    env.insert(crate::hook::ID_ENV.to_string(), id.to_string());
    env.insert(
        RECORD_DIR_ENV.to_string(),
        record.to_string_lossy().into_owned(),
    );
    env.insert(
        AGENT_DIR_ENV.to_string(),
        scratch.to_string_lossy().into_owned(),
    );
    match parent {
        Some((parent, dir)) => {
            env.insert(PARENT_ENV.to_string(), parent.to_string());
            env.insert(
                PARENT_DIR_ENV.to_string(),
                dir.to_string_lossy().into_owned(),
            );
            env.insert(DEPTH_ENV.to_string(), depth.to_string());
        }
        None => {
            env.remove(PARENT_ENV);
            env.remove(PARENT_DIR_ENV);
            env.remove(DEPTH_ENV);
        }
    }
    env
}

/// [`boot`] against the machine's state directory.
pub fn boot_from_env(id: &str) -> Result<i32> {
    boot(&crate::paths::state_root()?, id)
}

/// Wait up to [`RECORD_PATIENCE`] for another process to create `path`.
fn wait_for(path: &Path) -> Result<()> {
    let deadline = Instant::now() + RECORD_PATIENCE;
    while !path.exists() {
        if Instant::now() >= deadline {
            bail!("{} never arrived", path.display());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}

/// Ids of the agents running a turn, machine-wide.
pub fn live(root: &Path) -> Result<Vec<String>> {
    Ok(named(going(root)?))
}

/// Ids of the agents running a turn in `project`.
///
/// An agent belongs to the project [`project_of`] finds for its directory,
/// the same project whose config file sets the cap.
pub fn live_under(root: &Path, project: &Path) -> Result<Vec<String>> {
    // Agents share directories, and each distinct one costs a git call.
    let mut projects: BTreeMap<PathBuf, PathBuf> = BTreeMap::new();
    let theirs = going(root)?
        .into_iter()
        .filter(|meta| {
            projects
                .entry(meta.dir.clone())
                .or_insert_with_key(|dir| project_of(dir))
                == project
        })
        .collect();
    Ok(named(theirs))
}

/// Ids of every agent that has not ended and still has its pane.
///
/// Unlike [`live`], this includes shell rows and idle agents: `uninstall`
/// checks it before deleting every record.
pub fn unfinished(root: &Path) -> Result<Vec<String>> {
    Ok(named(answering(root, |phase, _| !phase.is_terminal())?))
}

/// Records of vendor agents in a turn: `Starting`, `Working` or `Waiting`.
///
/// Shell rows and `Idle` agents run no turn, so a cap does not count them;
/// nor does it count `Unknown`.
fn going(root: &Path) -> Result<Vec<Meta>> {
    answering(root, |phase, meta| {
        matches!(phase, Phase::Starting | Phase::Working | Phase::Waiting) && meta.agent.is_some()
    })
}

/// Records `wanted` accepts whose pane still answers for them.
///
/// Ownership rather than presence, since after a server restart a pane number
/// can belong to another agent. Records whose state or meta cannot be read are
/// skipped. A tmux that cannot be asked is an error, since the record may be a
/// running agent.
fn answering(root: &Path, wanted: impl Fn(Phase, &Meta) -> bool) -> Result<Vec<Meta>> {
    let mut kept = Vec::new();
    // One pane listing per server, however many agents sit on it.
    let mut owners: Vec<(Socket, PaneOwners)> = Vec::new();
    for id in crate::store::list(root)? {
        let agent = Agent::open(root, &id)?;
        let Ok(state) = agent.state() else { continue };
        let Ok(meta) = agent.meta() else { continue };
        if !wanted(state.state, &meta) {
            continue;
        }
        let listed = match owners.iter().position(|(socket, _)| socket == &meta.socket) {
            Some(at) => &owners[at].1,
            None => {
                let listed = Server::from_socket(meta.socket.clone()).owners_for_now()?;
                owners.push((meta.socket.clone(), listed));
                &owners.last().expect("just pushed").1
            }
        };
        if listed.pane_answers_for(&meta.pane, &meta.id) {
            kept.push(meta);
        }
    }
    Ok(kept)
}

/// The ids of `agents`, sorted.
fn named(agents: Vec<Meta>) -> Vec<String> {
    let mut ids: Vec<String> = agents.into_iter().map(|meta| meta.id).collect();
    ids.sort();
    ids
}

/// The project an agent belongs to: the repository behind its tree, else its
/// directory.
///
/// Starts no process, since the wall asks it of every agent on every reading.
/// An amx tree is read off its path, so the answer survives the tree's
/// removal; any other linked worktree (such as `workflow run`'s) is read off
/// its `.git` file.
pub fn project_dir(meta: &Meta) -> PathBuf {
    let tree = meta.worktree.as_deref().unwrap_or(&meta.dir);
    crate::worktree::repo_of(tree)
        .or_else(|| crate::worktree::repo_of_linked(tree))
        .unwrap_or_else(|| meta.dir.clone())
}

/// The project a directory belongs to: the repository behind it, or the
/// directory itself.
///
/// Derived from [`crate::paths::project_config`], so a cap counts exactly the
/// agents that read the config file setting it.
pub fn project_of(dir: &Path) -> PathBuf {
    crate::paths::project_config(dir)
        .as_deref()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| dir.to_path_buf())
}

/// The refusal message when a spawn in `project` would exceed a cap, else
/// `None`.
///
/// `max_agents` counts the project's own agents; `max_total`, when set,
/// counts every agent on the machine. Both count agents running a turn (see
/// [`going`]) plus places claimed by spawns still setting up.
pub fn at_capacity(
    root: &Path,
    project: &Path,
    max_agents: usize,
    max_total: Option<usize>,
) -> Result<Option<String>> {
    let claims = crate::store::claims(root)?;
    let here: BTreeSet<String> = live_under(root, project)?
        .into_iter()
        .chain(
            claims
                .iter()
                .filter(|(_, theirs)| theirs == project)
                .map(|(id, _)| id.clone()),
        )
        .collect();
    let here = here.len();
    if here >= max_agents {
        return Ok(Some(format!(
            "{here} agents already running in {}, and max_agents is {max_agents}",
            project.display()
        )));
    }

    let Some(ceiling) = max_total else {
        return Ok(None);
    };
    let everywhere: BTreeSet<String> = live(root)?
        .into_iter()
        .chain(claims.into_iter().map(|(id, _)| id))
        .collect();
    let everywhere = everywhere.len();
    Ok((everywhere >= ceiling).then(|| {
        format!("{everywhere} agents already running on this machine, and max_total is {ceiling}")
    }))
}

/// Check the caps and, if there is room, run `claim` and hold its place.
///
/// `claim` creates the agent's directory and returns it; the place is held
/// until the returned [`crate::store::Claim`] drops. The inner `Err` is
/// [`at_capacity`]'s refusal. Runs under [`crate::store::spawn_lock`], which is
/// released on return so the next spawn counts this claim without waiting for
/// setup.
pub fn take_a_place<T>(
    root: &Path,
    project: &Path,
    max_agents: usize,
    max_total: Option<usize>,
    claim: impl FnOnce() -> Result<(T, PathBuf)>,
) -> Result<Result<(T, crate::store::Claim), String>> {
    let _counting = crate::store::spawn_lock(root)?;
    if let Some(full) = at_capacity(root, project, max_agents, max_total)? {
        return Ok(Err(full));
    }
    let (claimed, dir) = claim()?;
    let held = crate::store::Claim::hold(&dir, project)?;
    Ok(Ok((claimed, held)))
}

/// Write the record for a spawned agent.
pub fn record(root: &Path, meta: &Meta) -> Result<Agent> {
    Agent::create(root, meta)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn vars(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect()
    }

    #[test]
    fn spawn_the_agent_inherits_everything_but_the_pane_it_was_asked_from() {
        let snapshot = env_snapshot(vars(&[
            ("PATH", "/usr/bin"),
            ("ANTHROPIC_MODEL", "opus"),
            ("TMUX", "/tmp/tmux-1000/default,42,0"),
            ("TMUX_PANE", "%7"),
            ("PWD", "/srv/app"),
            ("OLDPWD", "/home/dev"),
        ]));

        assert_eq!(snapshot.get("PATH").unwrap(), "/usr/bin");
        assert_eq!(snapshot.get("ANTHROPIC_MODEL").unwrap(), "opus");
        for gone in NOT_INHERITED {
            assert!(
                !snapshot.contains_key(gone),
                "{gone} describes where the command was typed, not where the agent runs"
            );
        }
    }

    #[test]
    fn spawn_leaves_behind_the_session_markers_of_every_vendor_amx_knows() {
        // Every vendor's markers are dropped, whichever session the command was
        // typed in. The names come from the registry, not a copy here.
        let typed_inside: Vec<(String, String)> = registry::entries()
            .iter()
            .flat_map(|vendor| vendor.not_inherited)
            .map(|name| (name.to_string(), "the spawner's".to_string()))
            .collect();
        assert!(!typed_inside.is_empty(), "a table with a vendor in it");

        let snapshot = env_snapshot(typed_inside);
        assert!(snapshot.is_empty(), "{snapshot:?}");
    }

    #[test]
    fn spawn_spells_no_vendors_variable_of_its_own() {
        // Only the pane's variables are spelled in spawn.rs. Vendor markers come
        // from the registry, so a vendor renaming one leaves no stale copy.
        let ships = include_str!("spawn.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap_or_default();
        for vendor in registry::entries() {
            for name in vendor.not_inherited {
                assert!(
                    !ships.contains(name),
                    "spawn keeps its own copy of {}'s {name}",
                    vendor.name
                );
            }
        }
    }

    #[test]
    fn spawn_a_claude_spawning_another_does_not_make_it_a_child() {
        // Handed its spawner's markers, claude 2.1.240 turned transcript saving
        // off (inherited CLAUDE_CODE_CHILD_SESSION), and an agent with no
        // transcript cannot be quoted by `result` or continued by `resume` or
        // `fork`.
        let snapshot = env_snapshot(vars(&[
            ("CLAUDECODE", "1"),
            ("CLAUDE_PID", "12345"),
            ("CLAUDE_CODE_SESSION_ID", "abc-123"),
            ("CLAUDE_CODE_CHILD_SESSION", "1"),
            ("CLAUDE_CODE_ENTRYPOINT", "cli"),
            ("CLAUDE_CODE_EXECPATH", "/opt/claude/claude"),
            ("CLAUDE_EFFORT", "high"),
            ("CLAUDE_CODE_NO_FLICKER", "1"),
            ("CLAUDE_CODE_DISABLE_FEEDBACK_SURVEY", "1"),
        ]));

        for lineage in [
            "CLAUDECODE",
            "CLAUDE_PID",
            "CLAUDE_CODE_SESSION_ID",
            "CLAUDE_CODE_CHILD_SESSION",
            "CLAUDE_CODE_ENTRYPOINT",
            "CLAUDE_CODE_EXECPATH",
        ] {
            assert!(
                !snapshot.contains_key(lineage),
                "{lineage} names the session the command was typed in, \
                 not the agent"
            );
        }
        assert!(
            !snapshot.contains_key("CLAUDE_EFFORT"),
            "the spawner's effort is a dial nobody turned on this agent"
        );
        // Preferences belong to the person, not the session, and are kept.
        assert_eq!(snapshot.get("CLAUDE_CODE_NO_FLICKER").unwrap(), "1");
        assert_eq!(
            snapshot.get("CLAUDE_CODE_DISABLE_FEEDBACK_SURVEY").unwrap(),
            "1"
        );
    }

    #[test]
    fn spawn_the_task_is_the_last_word_the_vendor_is_given() {
        let command = vendor_command(
            "claude --model opus",
            &Dials::default(),
            &["--session-id".to_string(), "abc-123".to_string()],
            "fix the login bug",
            None,
            false,
        );
        assert_eq!(
            command,
            [
                "claude",
                "--model",
                "opus",
                "--session-id",
                "abc-123",
                "fix the login bug"
            ]
        );
    }

    #[test]
    fn spawn_dials_become_flags_in_front_of_what_the_caller_passed_through() {
        let command = vendor_command(
            "claude",
            &Dials {
                model: "opus".to_string(),
                effort: "high".to_string(),
                ..Dials::default()
            },
            &["--session-id".to_string(), "abc-123".to_string()],
            "fix the login bug",
            None,
            false,
        );
        assert_eq!(
            command,
            [
                "claude",
                "--model",
                "opus",
                "--effort",
                "high",
                "--session-id",
                "abc-123",
                "fix the login bug"
            ],
            "the permission dial nobody turned sends no flag at all"
        );
    }

    #[test]
    fn spawn_dials_stand_down_from_a_flag_the_argv_already_carries() {
        // However and wherever the flag was written, claude never gets it twice.
        let dials = Dials {
            model: "opus".to_string(),
            permission: "plan".to_string(),
            effort: "high".to_string(),
        };

        let command = vendor_command(
            "claude --effort max",
            &dials,
            &["--model=sonnet".to_string()],
            "fix the login bug",
            None,
            false,
        );
        assert_eq!(
            command,
            [
                "claude",
                "--effort",
                "max",
                "--permission-mode",
                "plan",
                "--model=sonnet",
                "fix the login bug"
            ]
        );
    }

    #[test]
    fn spawn_dials_send_nothing_to_a_vendor_the_table_has_no_entry_for() {
        // The e2e stand-in is unregistered, so dials change nothing.
        let command = vendor_command(
            "mock-claude",
            &Dials {
                model: "opus".to_string(),
                permission: "plan".to_string(),
                effort: "high".to_string(),
            },
            &[],
            "fix the login bug",
            None,
            false,
        );
        assert_eq!(command, ["mock-claude", "fix the login bug"]);
    }

    #[test]
    fn spawn_claude_declares_no_start_flag_and_its_argv_is_unchanged() {
        // claude's SessionStart hook reports the session it opened, so its
        // entry declares no start flag and a session id changes nothing.
        let without = vendor_command(
            "claude",
            &Dials::default(),
            &[],
            "fix the login bug",
            None,
            false,
        );
        let with = vendor_command(
            "claude",
            &Dials::default(),
            &[],
            "fix the login bug",
            Some("fix-login-a1b"),
            false,
        );
        assert_eq!(with, without);
        assert_eq!(with, ["claude", "fix the login bug"]);
    }

    #[test]
    fn spawn_a_vendor_that_declares_a_start_flag_is_offered_it_with_the_agents_own_id() {
        // The `second` fixture declares a start flag. start_flag returns the
        // flag alone; vendor_command adds the id.
        use crate::vendor::second::SECOND;

        assert_eq!(
            start_flag(Some(&SECOND), &[], &[]),
            Some("--open"),
            "the vendor's own start flag, offered with nothing on the argv yet"
        );
    }

    #[test]
    fn spawn_the_start_flag_stands_down_from_one_the_argv_already_carries() {
        use crate::vendor::second::SECOND;

        assert_eq!(
            start_flag(Some(&SECOND), &["--open"], &[]),
            None,
            "the command line already says which session this vendor opens"
        );
        assert_eq!(
            start_flag(Some(&SECOND), &[], &["--open".to_string()]),
            None,
            "wherever it was written, in the configured command or after it"
        );
    }

    #[test]
    fn spawn_the_start_flag_stands_down_from_a_flag_the_entry_lists_as_conflicting() {
        use crate::vendor::SessionSpec;
        use crate::vendor::second::SECOND;

        // SECOND's only conflict is its own start flag, which cannot tell this
        // arm from the one above, so build a vendor with a separate conflict.
        let disagrees = Vendor {
            session: Some(SessionSpec {
                conflicts: &["--resume-elsewhere"],
                ..SECOND.session.unwrap()
            }),
            ..SECOND
        };
        assert_eq!(
            start_flag(Some(&disagrees), &[], &["--resume-elsewhere".to_string()]),
            None,
            "a flag the entry lists as conflicting already says which \
             session this vendor opens"
        );
        assert_eq!(
            start_flag(Some(&disagrees), &["--open"], &[]),
            None,
            "and its own start flag still stands down, same as before"
        );
    }

    #[test]
    fn spawn_the_start_flag_stands_down_from_a_joined_spelling_of_itself() {
        use crate::vendor::second::SECOND;

        // `flag=value` counts as the flag, as in vendor::already and
        // resume::names_a_session.
        assert_eq!(
            start_flag(Some(&SECOND), &["--open=mine"], &[]),
            None,
            "flag=value already carries the flag"
        );
        assert_eq!(
            start_flag(Some(&SECOND), &[], &["--open=mine".to_string()]),
            None,
            "wherever it was written"
        );
    }

    #[test]
    fn spawn_the_start_flag_is_none_from_a_vendor_with_no_session_vocabulary_or_no_entry() {
        assert_eq!(start_flag(registry::entry("claude"), &[], &[]), None);
        assert_eq!(start_flag(registry::entry("mock-claude"), &[], &[]), None);
        assert_eq!(start_flag(None, &[], &[]), None);
    }

    #[test]
    fn spawn_the_flag_and_the_id_it_opens_land_in_the_vendors_own_args() {
        use crate::vendor::second::SECOND;

        // The minted id lands beside the flag, beyond start_flag saying yes.
        assert_eq!(
            session_flag(Some(&SECOND), &[], &[], Some("fix-login-a1b")),
            ["--open", "fix-login-a1b"]
        );
        assert_eq!(
            session_flag(Some(&SECOND), &[], &[], None),
            Vec::<String>::new(),
            "nothing to open a session under, so nothing is offered"
        );
        assert_eq!(
            session_flag(registry::entry("claude"), &[], &[], Some("fix-login-a1b")),
            Vec::<String>::new(),
            "claude declares no start flag to offer the id with"
        );
    }

    #[test]
    fn spawn_answers_pis_trust_screen_only_where_the_person_said_amx_may() {
        // pi answers the folder-trust screen with an argv flag, sent only with
        // the `trust` key's consent. Ungated, every pi would load whatever
        // repository it was pointed at.
        let approved = vendor_command(
            "pi",
            &Dials::default(),
            &[],
            "fix the login bug",
            None,
            true,
        );
        assert_eq!(approved, ["pi", "--approve", "--", "fix the login bug"]);

        let ungated = vendor_command(
            "pi",
            &Dials::default(),
            &[],
            "fix the login bug",
            None,
            false,
        );
        assert_eq!(ungated, ["pi", "--", "fix the login bug"]);
    }

    #[test]
    fn spawn_ends_pis_options_before_a_task_so_an_at_sign_is_words() {
        // pi reads a word starting with `-` as a flag until `--`, and one
        // starting with `@` as a file to attach on either side, so that word
        // gets a leading space. claude has neither reading.
        let pi = vendor_command(
            "pi",
            &Dials::default(),
            &[],
            "@alice asked for this",
            None,
            false,
        );
        assert_eq!(pi, ["pi", "--", " @alice asked for this"]);

        let claude = vendor_command(
            "claude",
            &Dials::default(),
            &[],
            "@alice asked for this",
            None,
            false,
        );
        assert_eq!(claude, ["claude", "@alice asked for this"]);
    }

    #[test]
    fn spawn_a_message_rides_on_the_prompt_flag_as_one_word() {
        // A vendor that takes no bare prompt gets one `flag=<text>` word, so a
        // leading `-` is never read as a flag. A last word that opens a popup
        // gets a trailing space, or the popup takes the Enter.
        use crate::vendor::second::ELSEWHERE;

        let vendor = Some(&ELSEWHERE);
        assert_eq!(as_words(vendor, "-v is broken"), "--say=-v is broken");
        assert_eq!(as_words(vendor, "look at #3"), "--say=look at #3 ");
        assert_eq!(as_words(vendor, "#3 is fixed"), "--say=#3 is fixed");
        assert_eq!(as_words(vendor, "look at #3 "), "--say=look at #3 ");

        let claude = registry::entry("claude");
        assert_eq!(as_words(claude, "look at #3"), "look at #3");
        assert_eq!(as_words(None, "-v is broken"), "-v is broken");
    }

    #[test]
    fn spawn_a_message_is_typed_with_a_space_only_after_a_popup_word() {
        use crate::vendor::second::ELSEWHERE;

        let vendor = Some(&ELSEWHERE);
        assert_eq!(as_typed(vendor, "look at #3"), "look at #3 ");
        assert_eq!(as_typed(vendor, "#3"), "#3 ");
        assert_eq!(as_typed(vendor, "#3 is fixed"), "#3 is fixed");
        assert_eq!(as_typed(vendor, "look at @3"), "look at @3");
        assert_eq!(as_typed(vendor, ""), "");
        for other in [registry::entry("claude"), registry::entry("pi"), None] {
            assert_eq!(as_typed(other, "look at #3"), "look at #3");
        }
    }

    #[test]
    fn spawn_launch_words_go_right_after_the_program_once() {
        // Launch words go right after the program, once: not again when the
        // configured command, a harness's args or the caller's args carry one.
        use crate::vendor::second::BRANCHING;

        let words =
            |words: &[&str]| -> Vec<String> { words.iter().map(|word| word.to_string()).collect() };
        let vendor = Some(&BRANCHING);
        assert_eq!(
            with_launch_words(words(&["second", "-m", "large"]), vendor, &[]),
            ["second", "--alone", "-m", "large"]
        );
        assert_eq!(
            with_launch_words(words(&["second", "-m", "large", "--alone"]), vendor, &[]),
            ["second", "-m", "large", "--alone"]
        );
        assert_eq!(
            with_launch_words(words(&["second"]), vendor, &words(&["--alone"])),
            ["second"],
            "the caller's own args are added later, and carry it already"
        );
        for other in [registry::entry("claude"), None] {
            assert_eq!(with_launch_words(words(&["x"]), other, &[]), ["x"]);
        }
    }

    #[test]
    fn spawn_sends_no_trust_flag_for_a_vendor_answered_some_other_way() {
        // claude is answered by a store entry written before the pane starts,
        // so it gets no flag even with the key on.
        let claude = vendor_command(
            "claude",
            &Dials::default(),
            &[],
            "fix the login bug",
            None,
            true,
        );
        assert_eq!(claude, ["claude", "fix the login bug"]);

        let unregistered = vendor_command(
            "mock-claude",
            &Dials::default(),
            &[],
            "fix the login bug",
            None,
            true,
        );
        assert_eq!(
            unregistered,
            ["mock-claude", "fix the login bug"],
            "and a command with no entry has no screen amx knows anything about"
        );
    }

    #[test]
    fn spawn_the_trust_flag_stands_down_from_an_argv_that_settles_it_already() {
        // All four spellings pi reads, wherever written. The opposites matter
        // most: pi takes the last one it sees and amx's flag lands after the
        // configured arguments, so an added `--approve` would overrule a
        // `--no-approve`.
        for written in ["--approve", "-a", "--no-approve", "-na"] {
            assert_eq!(
                trust_flag("pi", &[written], &[], true),
                None,
                "{written}, written in the configured command"
            );
            assert_eq!(
                trust_flag("pi", &[], &[written.to_string()], true),
                None,
                "{written}, written after the separator"
            );
        }
        assert_eq!(
            trust_flag("pi", &["--approve=yes"], &[], true),
            None,
            "a spelling pi would not read is still somebody saying it themselves"
        );
        assert_eq!(
            trust_flag("pi", &[], &[], true),
            Some("--approve".to_string()),
            "and an argv that says nothing about the folder is answered"
        );
    }

    #[test]
    fn spawn_an_agent_that_splits_to_nothing_still_spawns_something_that_runs() {
        // `agent = ""` in the config, or `--agent ""` typed by hand. Indexing
        // the split argv used to panic here.
        let command = vendor_command(
            "",
            &Dials::default(),
            &[],
            "fix the login bug",
            Some("fix-login-a1b"),
            false,
        );
        assert_eq!(command, ["fix the login bug"]);
    }

    #[test]
    fn spawn_an_unregistered_agent_still_spawns_something_that_runs() {
        // A program with no registry entry: the session parameter changes nothing.
        let command = vendor_command(
            "pi -c",
            &Dials::default(),
            &[],
            "fix the login bug",
            Some("fix-login-a1b"),
            false,
        );
        assert_eq!(command, ["pi", "-c", "--", "fix the login bug"]);
    }

    #[test]
    fn spawn_opens_under_id_answers_the_same_question_vendor_command_asks_itself() {
        // opens_under_id shares vendor_command's lookup and start_flag call.
        // The yes case is covered on start_flag above; this covers no entry
        // and no start flag.
        assert!(!opens_under_id("claude", &[]), "no start flag to offer");
        assert!(
            !opens_under_id("pi -c", &[]),
            "no entry, so nothing to offer either"
        );
    }

    #[test]
    fn spawn_an_agents_vendor_is_the_one_its_recorded_command_names() {
        let started = |command: &[&str]| Handoff {
            task: "fix the login bug".to_string(),
            command: command.iter().map(|word| word.to_string()).collect(),
        };

        assert_eq!(
            vendor_of(&started(&[
                "claude",
                "--model",
                "opus",
                "fix the login bug"
            ]))
            .map(|vendor| vendor.name),
            Some("claude")
        );
        // The e2e stand-in has no entry, so it has no vendor.
        assert!(vendor_of(&started(&["mock-claude", "fix the login bug"])).is_none());
        assert!(vendor_of(&started(&[])).is_none(), "nothing was recorded");
    }

    #[test]
    fn spawn_a_handoff_is_readable_by_nobody_else() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().unwrap();
        let handoff = Handoff {
            task: "fix the login bug".to_string(),
            command: vec!["claude".to_string(), "fix the login bug".to_string()],
        };
        write_handoff(dir.path(), &handoff).unwrap();

        let mode = std::fs::metadata(dir.path().join(HANDOFF))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(
            mode & 0o777,
            0o600,
            "what a command was launched with is not everyone's to read"
        );
        assert_eq!(read_handoff(dir.path()).unwrap(), handoff);
    }

    #[test]
    fn spawn_a_handoff_written_over_an_open_one_is_kept_to_its_owner() {
        use std::os::unix::fs::PermissionsExt;

        // `resume` rewrites handoffs an older amx may have left world-readable.
        let dir = TempDir::new().unwrap();
        let path = dir.path().join(HANDOFF);
        std::fs::write(&path, "{}").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        let handoff = Handoff {
            task: "fix the login bug".to_string(),
            command: vec!["claude".to_string(), "fix the login bug".to_string()],
        };
        write_handoff(dir.path(), &handoff).unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        assert_eq!(read_handoff(dir.path()).unwrap(), handoff);
    }

    #[test]
    fn spawn_the_boot_environment_is_read_once_and_then_gone() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().unwrap();
        let env = env_snapshot(vars(&[("ANTHROPIC_API_KEY", "not-a-real-key")]));
        write_boot_env(dir.path(), &env).unwrap();

        let path = dir.path().join(BOOT_ENV);
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "somebody's environment is in it");

        assert_eq!(take_boot_env(dir.path()).unwrap(), env);
        assert!(!path.exists(), "read once, and not there for a second read");
    }

    #[test]
    fn spawn_a_handoff_an_older_amx_wrote_still_parses_with_its_env_ignored() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join(HANDOFF),
            br#"{"task":"fix the login bug","command":["claude","fix the login bug"],"env":{"PATH":"/usr/bin"}}"#,
        )
        .unwrap();

        let handoff = read_handoff(dir.path()).unwrap();
        assert_eq!(
            handoff,
            Handoff {
                task: "fix the login bug".to_string(),
                command: vec!["claude".to_string(), "fix the login bug".to_string()],
            },
            "the stray env key an older amx wrote is ignored, not refused"
        );
    }

    #[test]
    fn spawn_boot_gives_up_rather_than_waiting_for_a_record_that_is_not_coming() {
        let root = TempDir::new().unwrap();
        assert!(boot(root.path(), "../elsewhere").is_err(), "not an id");
    }

    #[test]
    fn spawn_keeps_a_commands_output_whole_and_an_agents_bounded() {
        // A shell command's output is appended whole. An agent's is capped at
        // BOOT_BYTES and overwritten, since a resume is a new boot.
        let socket = crate::tmux::Socket::Name("amx".to_string());
        let pane = PaneId::new("%1").unwrap();
        let dir = Path::new("/srv/state/agents/fix-login-a1b");

        let vendor = meta("fix-login-a1b", socket.clone(), pane.clone());
        assert_eq!(
            keeping_output(&vendor, dir),
            format!("head -c {BOOT_BYTES} > '/srv/state/agents/fix-login-a1b/output'"),
        );

        let command = Meta {
            agent: None,
            ..meta("build-a1b", socket, pane)
        };
        assert_eq!(
            keeping_output(&command, dir),
            "cat >> '/srv/state/agents/fix-login-a1b/output'",
        );
    }

    /// A vendor agent's record, the kind a cap counts.
    fn meta(id: &str, socket: crate::tmux::Socket, pane: PaneId) -> Meta {
        Meta {
            role: None,
            parent: None,
            depth: 0,
            id: id.to_string(),
            task: "fix the login bug".to_string(),
            agent: Some("claude".to_string()),
            model: None,
            effort: None,
            dir: PathBuf::from("/srv/app"),
            worktree: None,
            branch: None,
            base: None,
            socket,
            pane,
            bg: false,
            session: None,
            transcript: None,
            created: 1,
        }
    }

    /// A tmux server killed when the test ends.
    struct Own(Server);

    impl Drop for Own {
        fn drop(&mut self) {
            let _ = self.0.kill();
        }
    }

    /// A long-running pane for agent `id`, created by `place`, whose session
    /// name is what makes the pane answer for the agent.
    fn placed(server: &Server, id: &str) -> PaneId {
        let command = ["sh", "-c", "while :; do sleep 0.05; done"].map(str::to_string);
        place(server, id, Path::new("/"), &command).expect("a pane for it")
    }

    #[test]
    fn spawn_refuses_a_tmux_below_the_floor_by_name() {
        let refused = meets_the_floor((3, 1)).expect_err("tmux 3.1 is below the floor");
        assert!(
            refused.to_string().contains("tmux 3.1"),
            "the refusal names the tmux it found: {refused}"
        );
        assert!(
            refused.to_string().contains("3.2"),
            "and the one it needs: {refused}"
        );
        meets_the_floor(crate::tmux::MINIMUM_VERSION).expect("the floor itself is enough");
        meets_the_floor((4, 0)).expect("and anything newer");
    }

    #[test]
    fn spawn_an_exited_agents_pane_is_gone_under_a_global_remain_on_exit() {
        // A tmux.conf may keep dead panes. An agent's dead pane would keep its
        // session name, and a resume could not reuse it.
        let server =
            Own(Server::named(format!("amx-remain-{}", std::process::id())).with_conf("/dev/null"));
        server
            .0
            .new_session(&Spawn {
                name: Some("theirs"),
                command: &["sh", "-c", "while :; do sleep 0.05; done"],
                ..Spawn::default()
            })
            .unwrap();
        server
            .0
            .run(&["set-option", "-g", "remain-on-exit", "on"])
            .unwrap();

        let exits = ["sh", "-c", "sleep 0.3"].map(str::to_string);
        place(&server.0, "exits-a1b", Path::new("/"), &exits).expect("a pane for it");
        let name = session_name("exits-a1b");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while server.0.session_named(&name).unwrap().is_some() {
            assert!(
                std::time::Instant::now() < deadline,
                "its session outlived it"
            );
            std::thread::sleep(std::time::Duration::from_millis(50));
        }

        place(&server.0, "exits-a1b", Path::new("/"), &exits).expect("its session again");
    }

    #[test]
    fn spawn_an_agent_of_a_tree_amx_cut_belongs_to_the_repository_behind_it() {
        let tree = PathBuf::from("/srv/app/.amx/worktrees/fix-login-a1b");
        let socket = crate::tmux::Socket::Name("amx".to_string());
        let pane = PaneId::new("%1").unwrap();

        let cut = Meta {
            parent: None,
            depth: 0,
            dir: tree.clone(),
            worktree: Some(tree),
            ..meta("fix-login-a1b", socket.clone(), pane.clone())
        };
        assert_eq!(project_dir(&cut), Path::new("/srv/app"));

        // Any other agent's project is its directory. The disk is not
        // consulted, since a record outlives its tree.
        let plain = meta("port-it-b2c", socket, pane);
        assert_eq!(project_dir(&plain), plain.dir);
    }

    #[test]
    fn spawn_an_agent_of_a_tree_somebody_else_cut_belongs_to_that_repository_too() {
        // `workflow run` cuts trees under `~/.local/state/workflow`, far from
        // the checkout. They are still the repository's worktrees, and their
        // `.git` file names it.
        let home = TempDir::new().unwrap();
        let repo = home.path().join("code/amx");
        let tree = home.path().join("state/workflow/worktrees/amx/t3");
        std::fs::create_dir_all(repo.join(".git/worktrees/t3")).unwrap();
        std::fs::create_dir_all(&tree).unwrap();
        std::fs::write(
            tree.join(".git"),
            format!("gitdir: {}\n", repo.join(".git/worktrees/t3").display()),
        )
        .unwrap();

        let socket = crate::tmux::Socket::Name("amx".to_string());
        let pane = PaneId::new("%1").unwrap();
        let worker = Meta {
            dir: tree.clone(),
            worktree: None,
            ..meta("wf-t3-a1b", socket.clone(), pane.clone())
        };
        assert_eq!(project_dir(&worker), repo);

        // A directory git never heard of is its own project, as is a tree whose
        // `.git` points somewhere other than a worktree record.
        std::fs::write(tree.join(".git"), "gitdir: /srv/elsewhere\n").unwrap();
        assert_eq!(project_dir(&worker), tree);
        std::fs::remove_file(tree.join(".git")).unwrap();
        assert_eq!(project_dir(&worker), tree);
    }

    #[test]
    fn spawn_the_project_behind_a_directory_is_where_its_config_file_is() {
        // An amx tree is read off its path, so this holds after the tree is gone.
        assert_eq!(
            project_of(Path::new("/srv/app/.amx/worktrees/fix-login-a1b")),
            Path::new("/srv/app")
        );

        // Outside a repository the directory is the project.
        let dir = TempDir::new().unwrap();
        assert_eq!(project_of(dir.path()), dir.path());

        // A relative spelling resolves to the absolute project records hold.
        let here = std::env::current_dir().unwrap();
        assert_eq!(
            project_of(Path::new("scratch")),
            project_of(&here.join("scratch"))
        );
        assert!(project_of(Path::new("scratch")).is_absolute());
    }

    #[test]
    fn live_counts_a_project_on_its_own_and_the_machine_over_all_of_them() {
        let root = TempDir::new().unwrap();
        let (alpha, beta) = (TempDir::new().unwrap(), TempDir::new().unwrap());
        let server =
            Own(Server::named(format!("amx-count-{}", std::process::id())).with_conf("/dev/null"));
        let socket = server.0.socket().clone();

        for (id, dir) in [
            ("first-a1b", alpha.path()),
            ("second-b2c", alpha.path()),
            ("third-c3d", beta.path()),
        ] {
            let of_theirs = Meta {
                parent: None,
                depth: 0,
                dir: dir.to_path_buf(),
                ..meta(id, socket.clone(), placed(&server.0, id))
            };
            Agent::create(root.path(), &of_theirs).expect("a record");
        }

        assert_eq!(
            live_under(root.path(), alpha.path()).unwrap(),
            ["first-a1b", "second-b2c"]
        );
        assert_eq!(live_under(root.path(), beta.path()).unwrap(), ["third-c3d"]);
        assert_eq!(
            live(root.path()).unwrap().len(),
            3,
            "and the machine is all"
        );

        // max_agents is per project: at the same number one project is full and
        // the other has room.
        let full = at_capacity(root.path(), alpha.path(), 2, None)
            .unwrap()
            .expect("alpha is at its cap");
        assert!(full.contains("max_agents is 2"), "{full}");
        assert!(
            full.contains(&alpha.path().display().to_string()),
            "the project it counted: {full}"
        );
        assert_eq!(
            at_capacity(root.path(), beta.path(), 2, None).unwrap(),
            None
        );

        // max_total counts every project.
        let over = at_capacity(root.path(), beta.path(), 2, Some(3))
            .unwrap()
            .expect("the machine is at its ceiling");
        assert!(over.contains("max_total is 3"), "{over}");
        assert_eq!(
            at_capacity(root.path(), beta.path(), 2, Some(4)).unwrap(),
            None,
            "and a ceiling nothing has reached refuses nothing"
        );
    }

    #[test]
    fn live_counts_neither_a_shell_command_nor_an_agent_at_its_prompt() {
        // All three panes answer. A shell command has no vendor and an idle
        // agent's turn is over; a cap rations turns, so neither counts.
        let root = TempDir::new().unwrap();
        let project = TempDir::new().unwrap();
        let server = Own(
            Server::named(format!("amx-running-{}", std::process::id())).with_conf("/dev/null")
        );
        let socket = server.0.socket().clone();

        for (id, agent, phase) in [
            ("run-tests-a1b", None, Phase::Starting),
            ("port-it-b2c", Some("claude".to_string()), Phase::Idle),
            ("fix-login-c3d", Some("claude".to_string()), Phase::Working),
        ] {
            let of_theirs = Meta {
                parent: None,
                depth: 0,
                agent,
                dir: project.path().to_path_buf(),
                ..meta(id, socket.clone(), placed(&server.0, id))
            };
            let record = Agent::create(root.path(), &of_theirs).expect("a record");
            record
                .writer()
                .unwrap()
                .update_state(|state| state.state = phase)
                .unwrap();
        }

        assert_eq!(live(root.path()).unwrap(), ["fix-login-c3d"]);
        assert_eq!(
            at_capacity(root.path(), project.path(), 2, None).unwrap(),
            None,
            "and a cap of two has a place left: only one of the three is running"
        );
    }

    #[test]
    fn a_cap_refuses_a_tmux_that_cannot_be_asked() {
        // A running record whose pane cannot be checked may hold a place, and
        // skipping it would let one spawn too many.
        let root = TempDir::new().unwrap();
        let project = TempDir::new().unwrap();
        let of_theirs = Meta {
            parent: None,
            depth: 0,
            dir: project.path().to_path_buf(),
            ..meta(
                "fix-login-a1b",
                crate::tmux::unaskable(),
                PaneId::new("%3").unwrap(),
            )
        };
        let record = Agent::create(root.path(), &of_theirs).expect("a record");
        record
            .writer()
            .unwrap()
            .update_state(|state| state.state = Phase::Working)
            .unwrap();

        let why = at_capacity(root.path(), project.path(), 2, None).unwrap_err();
        assert!(
            format!("{why:#}").starts_with("tmux could not be asked: "),
            "{why:#}"
        );
    }

    #[test]
    fn live_does_not_count_an_agent_whose_pane_answers_for_another() {
        // The old record's server died and tmux numbers panes from %0 per
        // server, so today's agent has the same pane number. A record whose
        // pane answers for another agent is not running.
        let root = TempDir::new().unwrap();
        let project = TempDir::new().unwrap();
        let server =
            Own(Server::named(format!("amx-owner-{}", std::process::id())).with_conf("/dev/null"));
        let pane = placed(&server.0, "today-b2c");
        let socket = server.0.socket().clone();

        for id in ["today-b2c", "yesterday-a1b"] {
            let of_theirs = Meta {
                parent: None,
                depth: 0,
                dir: project.path().to_path_buf(),
                ..meta(id, socket.clone(), pane.clone())
            };
            Agent::create(root.path(), &of_theirs).expect("a record");
        }

        assert_eq!(live(root.path()).unwrap(), ["today-b2c"]);
        assert_eq!(
            at_capacity(root.path(), project.path(), 2, None).unwrap(),
            None,
            "and the record that lost its pane fills none of the cap"
        );
    }

    #[test]
    fn two_spawns_at_the_cap_start_one() {
        // Two spawns at once with room for one: each must see the other's
        // claim, under a lock only one holds at a time.
        let home = TempDir::new().unwrap();
        let root = home.path().join("agents");
        std::fs::create_dir(&root).unwrap();
        let project = TempDir::new().unwrap();
        let server =
            Own(Server::named(format!("amx-cap-{}", std::process::id())).with_conf("/dev/null"));
        let running = Meta {
            parent: None,
            depth: 0,
            dir: project.path().to_path_buf(),
            ..meta(
                "running-a1b",
                server.0.socket().clone(),
                placed(&server.0, "running-a1b"),
            )
        };
        Agent::create(&root, &running).expect("a record");

        let at_once = std::sync::Barrier::new(2);
        let places: Vec<_> = std::thread::scope(|scope| {
            let spawns: Vec<_> = ["first-b2c", "second-c3d"]
                .map(|id| {
                    let (root, project, at_once) = (&root, project.path(), &at_once);
                    scope.spawn(move || {
                        at_once.wait();
                        take_a_place(root, project, 2, None, || {
                            let dir = root.join(id);
                            std::fs::create_dir(&dir)?;
                            Ok((id, dir))
                        })
                        .unwrap()
                    })
                })
                .into_iter()
                .collect();
            spawns
                .into_iter()
                .map(|spawn| spawn.join().unwrap())
                .collect()
        });

        let (started, refused): (Vec<_>, Vec<_>) = places.into_iter().partition(Result::is_ok);
        assert_eq!(started.len(), 1, "one of the two starts");
        let refusal = refused.into_iter().next().unwrap().err().unwrap();
        assert!(refusal.contains("max_agents is 2"), "{refusal}");

        // The lock is released before setup while the claim stays held, so the
        // next spawn counts without waiting. Polled briefly: a child another
        // test forks while the lock is held shares it until exec.
        let lock = std::fs::File::open(home.path().join("spawn.lock")).unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        while let Err(e) = lock.try_lock() {
            assert!(Instant::now() < deadline, "nobody holds the count: {e:?}");
            std::thread::sleep(Duration::from_millis(10));
        }
        drop(lock);
        assert!(
            at_capacity(&root, project.path(), 2, None)
                .unwrap()
                .is_some(),
            "and the claim still fills its place"
        );

        // A spawn that gives up leaves its place for the next.
        drop(started);
        assert_eq!(at_capacity(&root, project.path(), 2, None).unwrap(), None);
    }

    #[test]
    fn live_skips_an_agent_whose_state_json_is_unreadable_but_lists_the_rest() {
        let root = TempDir::new().unwrap();
        let server =
            Own(Server::named(format!("amx-spawn-{}", std::process::id())).with_conf("/dev/null"));
        let socket = server.0.socket().clone();

        let broken = Agent::create(
            root.path(),
            &meta(
                "broken-a1b",
                socket.clone(),
                placed(&server.0, "broken-a1b"),
            ),
        )
        .expect("a record");
        std::fs::write(broken.dir().join("state.json"), b"not json at all").expect("garbage bytes");
        Agent::create(
            root.path(),
            &meta("fine-b2c", socket, placed(&server.0, "fine-b2c")),
        )
        .expect("a record");

        let live = live(root.path()).expect("the walk to finish");
        assert_eq!(live, vec!["fine-b2c".to_string()]);
    }

    #[test]
    fn exec_a_command_is_handed_to_a_shell_whole() {
        assert_eq!(
            exec_command("npm test && echo ok > log"),
            ["sh", "-c", "npm test && echo ok > log"],
            "a pipeline, an && and a redirect are one row, because a shell is \
             what reads them"
        );
    }

    #[test]
    fn exec_a_pane_is_given_a_directory_of_its_own_beside_the_record() {
        use std::os::unix::fs::PermissionsExt;

        let root = TempDir::new().unwrap();
        let agent = root.path().join("fix-login-a1b");
        std::fs::create_dir_all(&agent).unwrap();

        let dir = scratch(&agent).unwrap();
        assert_eq!(dir, agent.join(SCRATCH), "beside the record, not in it");
        assert_eq!(
            std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
            0o700,
            "what an agent writes is its owner's, like the record next to it"
        );
        assert_eq!(
            scratch(&agent).unwrap(),
            dir,
            "and a pane started again gets the directory it had"
        );
    }

    #[test]
    fn exec_every_pane_is_told_which_directory_is_its_own() {
        // The snapshot may come from another agent's pane, so amx's per-pane
        // variables are written over it; otherwise the new agent would write
        // in the old one's directory.
        let inherited = env_snapshot(vars(&[
            ("PATH", "/usr/bin"),
            ("AMX_ID", "fix-login-a1b"),
            ("AMX_DIR", "/state/agents/fix-login-a1b"),
            ("AMX_AGENT_DIR", "/state/agents/fix-login-a1b/scratch"),
            ("AMX_PARENT", "fix-login-a1b"),
            ("AMX_PARENT_DIR", "/state/agents/fix-login-a1b"),
            ("AMX_DEPTH", "3"),
            ("AMX_NESTED", "1"),
        ]));

        let env = pane_env(
            &inherited,
            Path::new("/usr/local/bin/amx"),
            "port-it-b2c",
            Path::new("/state/agents/port-it-b2c"),
            Path::new("/state/agents/port-it-b2c/scratch"),
            None,
            0,
        );

        assert_eq!(
            env.get(AGENT_DIR_ENV).unwrap(),
            "/state/agents/port-it-b2c/scratch"
        );
        assert_eq!(
            env.get(RECORD_DIR_ENV).unwrap(),
            "/state/agents/port-it-b2c",
            "the record's own directory, for what amx's reporting streams beside it"
        );
        assert_eq!(env.get(crate::hook::ID_ENV).unwrap(), "port-it-b2c");
        assert_eq!(env.get("AMX_BIN").unwrap(), "/usr/local/bin/amx");
        assert_eq!(env.get("PATH").unwrap(), "/usr/bin", "and the rest stands");
        // A root has no parent, whatever the spawner's family was.
        assert_eq!(env.get(PARENT_ENV), None, "{env:?}");
        assert_eq!(env.get(PARENT_DIR_ENV), None, "{env:?}");
        assert_eq!(env.get(DEPTH_ENV), None, "{env:?}");
        // The spawner's shell is marked nested; this pane is an agent that
        // reports for itself.
        assert_eq!(env.get(crate::hook::NESTED_ENV), None, "{env:?}");
    }

    #[test]
    fn a_childs_pane_is_told_its_parent_and_its_depth() {
        let inherited = env_snapshot(vars(&[("PATH", "/usr/bin")]));
        let env = pane_env(
            &inherited,
            Path::new("/usr/local/bin/amx"),
            "port-it-b2c",
            Path::new("/state/agents/port-it-b2c"),
            Path::new("/state/agents/port-it-b2c/scratch"),
            Some(("fix-login-a1b", Path::new("/state/agents/fix-login-a1b"))),
            1,
        );

        assert_eq!(env.get(PARENT_ENV).unwrap(), "fix-login-a1b");
        assert_eq!(
            env.get(PARENT_DIR_ENV).unwrap(),
            "/state/agents/fix-login-a1b"
        );
        assert_eq!(env.get(DEPTH_ENV).unwrap(), "1");
    }

    /// A config whose only harness table is `name`'s, holding `env`.
    fn told(name: &str, env: &[(&str, &str)]) -> crate::config::Config {
        crate::config::Config {
            harnesses: BTreeMap::from([(
                name.to_string(),
                crate::config::HarnessConfig {
                    models: Vec::new(),
                    args: Vec::new(),
                    env: vars(env).into_iter().collect(),
                },
            )]),
            ..Default::default()
        }
    }

    #[test]
    fn harness_env_lays_the_tables_pairs_over_the_snapshot() {
        let name = registry::entries()[0].name;
        let config = told(
            name,
            &[("SOME_CONFIG_DIR", "/srv/work"), ("SOME_PROXY", "on")],
        );

        let mut env = env_snapshot(vars(&[("PATH", "/usr/bin"), ("SOME_PROXY", "off")]));
        harness_env(&mut env, &config, &format!("{name} --add-dir /tmp"));

        assert_eq!(env.get("SOME_CONFIG_DIR").unwrap(), "/srv/work");
        assert_eq!(
            env.get("SOME_PROXY").unwrap(),
            "on",
            "a pair replaces what the snapshot carried under that name"
        );
        assert_eq!(env.get("PATH").unwrap(), "/usr/bin", "and the rest stands");
    }

    #[test]
    fn harness_env_is_laid_under_the_variables_amx_puts_in_itself() {
        // A table that changed the id would file events under another agent,
        // so amx's variables go in after the table.
        let name = registry::entries()[0].name;
        let config = told(name, &[(crate::hook::ID_ENV, "somebody-else")]);

        let mut env = env_snapshot(vars(&[("PATH", "/usr/bin")]));
        harness_env(&mut env, &config, name);
        env.insert(crate::hook::ID_ENV.to_string(), "port-it-b2c".to_string());

        assert_eq!(env.get(crate::hook::ID_ENV).unwrap(), "port-it-b2c");
    }

    #[test]
    fn harness_env_for_a_program_with_no_table_changes_nothing() {
        let config = told(registry::entries()[0].name, &[("SOME_PROXY", "on")]);

        let snapshot = env_snapshot(vars(&[("PATH", "/usr/bin")]));
        let mut env = snapshot.clone();
        harness_env(&mut env, &config, "some-other-agent --flag");

        assert_eq!(env, snapshot);
    }
}
