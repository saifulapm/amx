//! `amx doctor`: check what amx needs from this machine and say what is missing.
//!
//! Each failing check carries a remedy. The checks cover tmux, the configured
//! agent, the config, hook wiring for every installed agent, the amx on the
//! PATH, the state root, handoffs that still carry the environment inline,
//! agents stopped at a setup screen, trust-store keys for removed trees, and
//! orphaned ids and trees. Two more run only when they apply: whether the
//! running tmux server's directory still exists, and, with `--dir`, whether an
//! agent started there would stop at its vendor's folder-trust screen, where it
//! fires no hooks and waits for somebody to attach.
//!
//! - An agent that is not installed gets no line; the configured one always does.
//! - `--fix` touches only amx's own files: handoffs, trust-store keys for trees
//!   amx removed (the store is backed up first), orphaned id directories and
//!   zeroed clocks. Wiring an agent is `amx setup`'s job.
//! - The `--dir` check writes nothing, so a caller can branch on the exit code.

use anyhow::{Context, Result};
use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::derive::View;
use crate::rules::Rule;
use crate::store::{Kind, Phase};
use crate::vendor::{Hooks, Wire};
use crate::{derive, exit, install, registry, spawn, store, tmux, trust, worktree};

/// One thing amx looked at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub name: &'static str,
    pub found: String,
    /// What to do about it; `None` when the check passes.
    pub remedy: Option<String>,
}

impl Check {
    fn ok(name: &'static str, found: impl Into<String>) -> Check {
        Check {
            name,
            found: found.into(),
            remedy: None,
        }
    }

    fn wrong(name: &'static str, found: impl Into<String>, remedy: impl Into<String>) -> Check {
        Check {
            name,
            found: found.into(),
            remedy: Some(remedy.into()),
        }
    }

    pub fn is_ok(&self) -> bool {
        self.remedy.is_none()
    }
}

/// Everything [`report`] judges, gathered from the machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Findings {
    /// The installed tmux version, if any.
    pub tmux: Option<(u32, u32)>,
    /// The configured agent command and where the PATH resolves it.
    pub vendor: String,
    pub vendor_path: Option<PathBuf>,
    /// The config file and the warnings from reading it.
    pub config: PathBuf,
    pub config_warnings: Vec<String>,
    /// The home directory every vendor's wiring is written under.
    pub home: PathBuf,
    /// One entry per agent to check, in table order.
    pub wirings: Vec<VendorWiring>,
    /// This amx, and every distinct amx file the PATH finds, in search order.
    pub exe: PathBuf,
    pub on_path: Vec<PathBuf>,
    /// The state root, and why amx cannot use it when it cannot.
    pub state_root: PathBuf,
    pub state_error: Option<String>,
    /// Handoffs that still carry the spawner's environment inline.
    pub dirty_handoffs: Vec<PathBuf>,
    /// Agents stopped at a vendor setup screen.
    pub parked: Vec<Parked>,
    /// The tmux server amx would use, when one is running and its cwd can be
    /// read.
    pub server: Option<StandingServer>,
    /// The vendor's trust store, for a vendor amx answers by writing one, and
    /// the removed trees it still names.
    pub store: Option<PathBuf>,
    pub stale: Vec<PathBuf>,
    /// The `--dir` directory and whether its vendor would ask to trust it.
    pub folder: Option<Folder>,
    /// Id directories with no record, left by a spawn that died between
    /// claiming its id and writing the record, with their age in seconds.
    pub orphan_ids: Vec<(PathBuf, u64)>,
    /// Trees under a repository's `.amx/worktrees` that no record names.
    pub orphan_trees: Vec<PathBuf>,
    /// Records whose clock is zero while their log has turns, with the
    /// seconds those turns add up to.
    pub zeroed: Vec<(String, u64)>,
}

/// Seconds an id directory without a record stands before `--fix` removes
/// it. A starting spawn holds one only briefly.
const ORPHAN_AGE: u64 = 600;

/// A directory an agent would start in, and whether its vendor would show the
/// folder-trust screen there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Folder {
    pub dir: PathBuf,
    /// The repository this is a linked worktree of. The vendor resolves the
    /// tree to it before looking up trust, as amx does when it writes trust.
    pub repo: Option<PathBuf>,
    /// Whether the store trusts the directory or its repository. `None` for a
    /// vendor whose store amx does not read.
    pub covered: Option<bool>,
    /// The config's `trust` key, which has amx answer for a linked worktree at
    /// spawn.
    pub trust: bool,
}

/// One agent's hook wiring, read off the disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VendorWiring {
    pub vendor: &'static str,
    pub hooks: Option<&'static Hooks>,
    /// Where this agent's wiring goes.
    pub wire: PathBuf,
    pub wired: install::Wired,
    /// Opt-in wires, each with its path and state. Usually empty.
    pub opt_in: Vec<(PathBuf, install::Wired)>,
}

/// The tmux server amx would use, and the directory its process is in.
///
/// The socket is kept so the remedy can address the right server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StandingServer {
    pub socket: tmux::Socket,
    pub cwd: tmux::ServerCwd,
}

/// An agent stopped at a vendor setup screen. Only a person can get it past.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parked {
    pub id: String,
    pub screen: Setup,
}

/// The screen a parked agent is stopped at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Setup {
    /// A setup screen from the vendor's ruleset, by rule name. `trust` is set
    /// when it is the folder-trust question and amx can answer it for this
    /// vendor.
    Gate { screen: String, trust: bool },
    /// A screen no rule claims, under a record that never left `starting`.
    Unread,
}

impl Setup {
    /// The screen, worded for the line that names the agent.
    fn says(&self) -> String {
        match self {
            Setup::Gate { screen, .. } => format!("its vendor's {screen} screen"),
            Setup::Unread => "an opening screen amx has no rule for".to_string(),
        }
    }
}

/// Judge the findings.
///
/// The server and folder checks appear only when there is something to check.
pub fn report(found: &Findings) -> Vec<Check> {
    let mut checks = vec![tmux_check(found), vendor_check(found), config_check(found)];
    checks.extend(found.wirings.iter().map(wiring_check));
    checks.extend(found.wirings.iter().flat_map(opt_in_checks));
    checks.extend([amx_check(found), state_check(found), env_check(found)]);
    checks.extend(server_check(found));
    checks.push(setup_check(found));
    checks.push(store_check(found));
    checks.extend(folder_check(found));
    checks.push(orphan_check(found));
    checks
}

/// Whether any id directory or worktree amx made has no record.
///
/// `--fix` removes the id directories. Trees may hold work, so they are only
/// named.
fn orphan_check(found: &Findings) -> Check {
    let (ids, trees) = (found.orphan_ids.len(), found.orphan_trees.len());
    if ids == 0 && trees == 0 {
        return Check::ok("orphans", "every id and every tree amx made has a record");
    }
    let mut said = Vec::new();
    if ids > 0 {
        said.push(match ids {
            1 => "one id directory has no record".to_string(),
            n => format!("{n} id directories have no record"),
        });
    }
    for tree in &found.orphan_trees {
        said.push(format!("no record names {}", tree.display()));
    }
    let remedy = match (ids > 0, trees > 0) {
        (true, false) => "run `amx doctor --fix`".to_string(),
        (false, _) => "look in each tree, then `git worktree remove` it".to_string(),
        (true, true) => {
            "run `amx doctor --fix` for the ids; look in each tree, then `git worktree remove` it"
                .to_string()
        }
    };
    Check::wrong("orphans", said.join("; "), remedy)
}

fn tmux_check(found: &Findings) -> Check {
    let (want_major, want_minor) = tmux::MINIMUM_VERSION;
    match found.tmux {
        Some((major, minor)) if (major, minor) >= tmux::MINIMUM_VERSION => {
            Check::ok("tmux", format!("{major}.{minor}"))
        }
        Some((major, minor)) => Check::wrong(
            "tmux",
            format!("{major}.{minor}"),
            format!(
                "amx addresses panes by id, which needs tmux {want_major}.{want_minor} or newer"
            ),
        ),
        None => Check::wrong(
            "tmux",
            "not installed",
            format!("install tmux {want_major}.{want_minor} or newer"),
        ),
    }
}

/// Whether the configured agent is on the PATH.
///
/// A command with no table entry is read as claude; that passes with a note.
fn vendor_check(found: &Findings) -> Check {
    match &found.vendor_path {
        Some(path) if registry::entry(&found.vendor).is_none() => Check::ok(
            "agent",
            format!(
                "{} at {}; no entry for {}: read as claude",
                found.vendor,
                path.display(),
                registry::program(&found.vendor)
            ),
        ),
        Some(path) => Check::ok("agent", format!("{} at {}", found.vendor, path.display())),
        None => Check::wrong(
            "agent",
            format!("`{}` is not on the PATH", program(&found.vendor)),
            format!(
                "install it, or set `agent` in {} to a command that is",
                found.config.display()
            ),
        ),
    }
}

fn config_check(found: &Findings) -> Check {
    if found.config_warnings.is_empty() {
        return Check::ok("config", found.config.display().to_string());
    }
    Check::wrong(
        "config",
        found.config_warnings.join("; "),
        format!("edit {}", found.config.display()),
    )
}

/// Whether the files this vendor's entry ships are the ones installed.
///
/// A vendor without hooks passes: amx reads its pane instead. The remedy is
/// `amx setup <agent>`, named per agent.
fn wiring_check(found: &VendorWiring) -> Check {
    let who = found.vendor;
    let Some(hooks) = found.hooks else {
        return Check::ok(
            "hooks",
            format!("{who} reports nothing amx can wire, so its pane is what amx reads"),
        );
    };
    let wire = found.wire.display();
    let what = match hooks.wire {
        Wire::File { .. } => "extension",
        Wire::Plugin { .. } => "plugin",
        Wire::Placed { .. } => "plugin",
        Wire::Hooks { .. } => return hooks_check(who, &found.wire, &found.wired),
    };

    match &found.wired {
        // Named in the vendor's own terms: pi loads an extension, claude a
        // plugin.
        install::Wired::File {
            present: true,
            current: true,
        } => Check::ok("hooks", format!("{who}: the {what} at {wire}")),
        install::Wired::File { present: true, .. } => Check::wrong(
            "hooks",
            format!("{who}: {wire} is not the {what} this amx ships"),
            setup_with(who),
        ),
        install::Wired::File { .. } | install::Wired::Nothing => Check::wrong(
            "hooks",
            format!("{who}: no {what} at {wire}"),
            setup_with(who),
        ),
    }
}

/// Whether amx's hook groups are in the vendor's hooks file and trusted in its
/// config under their current hash.
///
/// codex does not run a group without matching trust until somebody answers
/// its review screen.
fn hooks_check(who: &str, dir: &Path, wired: &install::Wired) -> Check {
    let hooks = dir.join(install::HOOKS_FILE);
    let config = dir.join(install::CONFIG_FILE);
    match wired {
        install::Wired::File {
            present: true,
            current: true,
        } => Check::ok(
            "hooks",
            format!(
                "{who}: amx's hooks in {}, trusted in {}",
                hooks.display(),
                config.display()
            ),
        ),
        install::Wired::File { present: true, .. } => Check::wrong(
            "hooks",
            format!(
                "{who}: amx's hooks in {} are not trusted in {}, so {who} will not run them",
                hooks.display(),
                config.display()
            ),
            setup_with(who),
        ),
        install::Wired::File { .. } | install::Wired::Nothing => Check::wrong(
            "hooks",
            format!("{who}: no hooks of amx's in {}", hooks.display()),
            setup_with(who),
        ),
    }
}

/// Lines for the opt-in wires that are installed.
///
/// An absent opt-in file passes silently: nobody asked for it. A stale one was
/// written by an older amx and may call a verb whose shape has changed.
fn opt_in_checks(found: &VendorWiring) -> Vec<Check> {
    found
        .opt_in
        .iter()
        .filter_map(|(path, wired)| {
            let at = path.display();
            let what = "extension";
            match wired {
                install::Wired::File { present: false, .. } | install::Wired::Nothing => None,
                install::Wired::File { current: true, .. } => Some(Check::ok(
                    "hooks",
                    format!("{}: the {what} at {at}", found.vendor),
                )),
                install::Wired::File { .. } => Some(Check::wrong(
                    "hooks",
                    format!("{}: {at} is not the {what} this amx ships", found.vendor),
                    format!("run `amx setup {} --subagent`", found.vendor),
                )),
            }
        })
        .collect()
}

/// The command that wires `who`.
fn setup_with(who: &str) -> String {
    format!("run `amx setup {who}`")
}

/// Whether the amx the PATH finds is this one, and the only one.
///
/// Hooks report to the first amx on the PATH, and each amx judges the wiring
/// against what it ships, so with two installs a rebuilt amx may never run
/// while the other passes its own checks.
fn amx_check(found: &Findings) -> Check {
    let exe = found.exe.display();
    let Some(first) = found.on_path.first() else {
        let dir = found.exe.parent().unwrap_or(&found.exe).display();
        return Check::wrong(
            "amx",
            format!("{exe} is not on the PATH"),
            format!("an agent started by hand reports to the amx the PATH finds; put {dir} on it"),
        );
    };
    let others: Vec<String> = found
        .on_path
        .iter()
        .filter(|amx| **amx != found.exe)
        .map(|amx| amx.display().to_string())
        .collect();
    if others.is_empty() {
        return Check::ok("amx", format!("{exe}, the only amx on the PATH"));
    }
    let what = if *first == found.exe {
        format!(
            "{exe} is first on the PATH, which also finds {}",
            others.join(", ")
        )
    } else {
        format!(
            "this amx is {exe}, but the PATH finds {} first",
            first.display()
        )
    };
    Check::wrong(
        "amx",
        what,
        "the hooks report to the amx the PATH finds, and each amx judges its own wiring; \
         install once, and make the other a symlink to it or take it off the PATH",
    )
}

/// Whether the tmux server amx would use still stands in a directory that
/// exists.
///
/// A server keeps its start directory for life. Once that is deleted, every
/// pane it forks starts in a missing directory and the vendor exits at once,
/// silently. `None` when no server is running or its cwd cannot be read.
fn server_check(found: &Findings) -> Option<Check> {
    let standing = found.server.as_ref()?;
    let (pid, where_) = (standing.cwd.pid, standing.cwd.path.display());
    Some(if standing.cwd.stale {
        Check::wrong(
            "server",
            format!("tmux server {pid}'s directory is gone: {where_}"),
            format!(
                "every pane it starts inherits that and dies at once; \
                 restart it: tmux {} kill-server",
                address(&standing.socket)
            ),
        )
    } else {
        Check::ok("server", format!("tmux server {pid} in {where_}"))
    })
}

/// How a tmux command line addresses `socket`.
fn address(socket: &tmux::Socket) -> String {
    match socket {
        tmux::Socket::Name(name) => format!("-L {name}"),
        tmux::Socket::Path(path) => format!("-S {}", path.display()),
    }
}

/// Whether amx can use the state root, and whether every record's clock
/// matches its log.
///
/// A record can carry a zero clock while its log has turns; `--fix` rebuilds
/// the clock from the log.
fn state_check(found: &Findings) -> Check {
    if let Some(why) = &found.state_error {
        return Check::wrong(
            "state",
            why.clone(),
            "amx keeps every agent there, so until that is fixed it has nowhere to put one",
        );
    }
    let zeroed: Vec<String> = found
        .zeroed
        .iter()
        .map(|(id, worked)| {
            format!(
                "{id} worked 0s and its log adds up to {}",
                derive::in_words(*worked)
            )
        })
        .collect();
    match zeroed.as_slice() {
        [] => Check::ok("state", found.state_root.display().to_string()),
        _ => Check::wrong("state", zeroed.join("; "), "run `amx doctor --fix`"),
    }
}

/// Whether any handoff still carries the spawner's environment inline.
///
/// [`spawn::Handoff`] no longer has an `env` field. serde ignores the stale
/// key, but the file still holds the environment.
fn env_check(found: &Findings) -> Check {
    let dirty = found.dirty_handoffs.len();
    if dirty == 0 {
        return Check::ok(
            "env",
            "no handoff.json still carries the environment inline",
        );
    }
    let what = if dirty == 1 {
        "one handoff.json still carries the environment inline".to_string()
    } else {
        format!("{dirty} handoff.json files still carry the environment inline")
    };
    Check::wrong("env", what, "run `amx doctor --fix`")
}

fn setup_check(found: &Findings) -> Check {
    let Some(first) = found.parked.first() else {
        return Check::ok(
            "gate",
            "no agent is stopped at a screen its vendor puts first",
        );
    };

    let each: Vec<String> = found
        .parked
        .iter()
        .map(|agent| format!("{} at {}", agent.id, agent.screen.says()))
        .collect();
    let what = match each.as_slice() {
        [one] => one.clone(),
        many => format!("{} agents are stopped: {}", many.len(), many.join(", ")),
    };

    let remedy = match &first.screen {
        // The config key is offered only for the folder-trust question, and
        // only for a vendor whose answer amx writes.
        Setup::Gate { trust: true, .. } => format!(
            "answer it yourself: amx attach {}, or set trust = true in the \
             config and amx answers it for any linked worktree",
            first.id
        ),
        _ => format!("answer it yourself: amx attach {}", first.id),
    };
    Check::wrong("gate", what, remedy)
}

/// Whether the vendor's trust store still names trees amx cut and removed.
///
/// The vendor adds a project entry for every directory it starts in, so each
/// removed agent tree leaves a key behind. A vendor without such a store
/// passes.
fn store_check(found: &Findings) -> Check {
    let Some(store) = &found.store else {
        return Check::ok(
            "store",
            format!(
                "{} keeps no store amx writes trees into",
                program(&found.vendor)
            ),
        );
    };
    let store = store.display();

    let stale = found.stale.len();
    if stale == 0 {
        return Check::ok("store", format!("no tree amx cut is left in {store}"));
    }
    let what = if stale == 1 {
        format!("one tree amx cut is gone and still in {store}")
    } else {
        format!("{stale} trees amx cut are gone and still in {store}")
    };
    Check::wrong("store", what, "run `amx doctor --fix`")
}

/// Whether an agent started in the `--dir` directory would meet its vendor's
/// folder-trust screen.
///
/// It passes when the store covers the directory, the vendor keeps no store
/// amx reads, or the directory is a linked worktree and `trust = true` has amx
/// answer at spawn. Otherwise the remedy names the repository for a worktree,
/// since its entry covers every tree, and the directory itself for anything
/// else.
fn folder_check(found: &Findings) -> Option<Check> {
    let folder = found.folder.as_ref()?;
    let (who, dir) = (program(&found.vendor), folder.dir.display());
    Some(match folder.covered {
        None => Check::ok(
            "trust",
            format!("{who} keeps no store amx reads a folder's answer from"),
        ),
        Some(true) => Check::ok(
            "trust",
            format!("{who} starts in {dir} without its folder-trust screen"),
        ),
        Some(false) if folder.trust && folder.repo.is_some() => Check::ok(
            "trust",
            format!(
                "{who} would ask about {dir}, and amx answers for a linked worktree at the spawn"
            ),
        ),
        Some(false) => Check::wrong(
            "trust",
            format!("{who} would stop at its folder-trust screen in {dir}"),
            match &folder.repo {
                Some(repo) => format!(
                    "start {who} in {} once and answer it yourself, or set trust = true in \
                     the config and amx answers it for any linked worktree",
                    repo.display()
                ),
                None => format!("start {who} in {dir} once and answer it yourself"),
            },
        ),
    })
}

/// Print the checks, apply the `--fix` repairs, and return OK only when every
/// check then passes.
pub fn run(found: &Findings, fix: bool, now: u64, out: &mut impl Write) -> Result<i32> {
    let mut current = found.clone();
    let mut checks = report(&current);
    for check in &checks {
        match &check.remedy {
            None => writeln!(out, "  ok  {:<7} {}", check.name, check.found)?,
            Some(remedy) => writeln!(
                out,
                "  no  {:<7} {}\n         {remedy}",
                check.name, check.found
            )?,
        }
    }

    if fix && !current.dirty_handoffs.is_empty() {
        let cleaned = clean_handoffs(&current.dirty_handoffs)?;
        writeln!(
            out,
            "\ncleaned the environment out of {cleaned} handoff.json {}",
            if cleaned == 1 { "file" } else { "files" }
        )?;
        current.dirty_handoffs = Vec::new();
        checks = report(&current);
    }

    if fix
        && !current.stale.is_empty()
        && let Some(store) = current.store.clone()
    {
        let forgotten = forget_trees(&store, &current.stale, now)?;
        writeln!(
            out,
            "\nforgot {forgotten} {} from {}",
            if forgotten == 1 { "tree" } else { "trees" },
            store.display()
        )?;
        current.stale = Vec::new();
        checks = report(&current);
    }

    if fix && !current.orphan_ids.is_empty() {
        let (old, young): (Vec<_>, Vec<_>) = current
            .orphan_ids
            .iter()
            .cloned()
            .partition(|(_, age)| *age >= ORPHAN_AGE);
        for (dir, _) in &old {
            std::fs::remove_dir_all(dir).with_context(|| format!("removing {}", dir.display()))?;
        }
        writeln!(
            out,
            "\nremoved {} id {} with no record",
            old.len(),
            if old.len() == 1 {
                "directory"
            } else {
                "directories"
            }
        )?;
        current.orphan_ids = young;
        checks = report(&current);
    }

    if fix && !current.zeroed.is_empty() {
        let rebuilt = rebuild_clocks(&current.state_root, &current.zeroed)?;
        writeln!(
            out,
            "\nrebuilt the clock of {rebuilt} {} from its log",
            if rebuilt == 1 { "agent" } else { "agents" }
        )?;
        current.zeroed = Vec::new();
        checks = report(&current);
    }

    Ok(if checks.iter().all(Check::is_ok) {
        exit::OK
    } else {
        exit::FAILURE
    })
}

/// The agents to check, and their wiring.
///
/// Every table entry whose program is on the PATH, in table order, plus the
/// configured agent even when missing, so a machine with no agent still gets
/// a hooks line. A configured command with no table entry is judged as the
/// first vendor, since a wrapper around claude loads claude's files.
fn wirings(agent: &str, home: &Path, env: install::Env, path: Option<&OsStr>) -> Vec<VendorWiring> {
    let configured = registry::entry(agent).or_else(|| registry::entries().first());
    registry::entries()
        .iter()
        .filter(|vendor| {
            configured.is_some_and(|it| it.name == vendor.name)
                || on_path(vendor.name, path).is_some()
        })
        .map(|vendor| {
            let hooks = vendor.hooks.as_ref();
            VendorWiring {
                vendor: vendor.name,
                hooks,
                wire: hooks.map_or_else(
                    || home.to_path_buf(),
                    |h| install::wire_path(&h.wire, home, env),
                ),
                wired: hooks.map_or(install::Wired::Nothing, |h| {
                    install::wired(&h.wire, home, env)
                }),
                opt_in: hooks.map_or_else(Vec::new, |h| {
                    h.opt_in
                        .iter()
                        .map(|wire| {
                            (
                                install::wire_path(wire, home, env),
                                install::wired(wire, home, env),
                            )
                        })
                        .collect()
                }),
            }
        })
        .collect()
}

/// Gather the findings under `config`, including `dir` when doctor was
/// pointed at one.
pub fn gather(
    config: &Config,
    config_warnings: Vec<String>,
    dir: Option<&Path>,
) -> Result<Findings> {
    let home = install::home()?;
    let exe = std::env::current_exe()?;
    let path = std::env::var_os("PATH");
    let wirings = wirings(&config.agent, &home, &install::process_env, path.as_deref());
    let state_root = crate::paths::state_root()?;
    // Only a vendor whose trust screen amx answers by writing its store has
    // keys of amx's in it. An unreadable store yields no stale trees.
    let store = trust::writes_a_store(&config.agent)
        .then(|| {
            // The harness table's env, since that is where its agents look for
            // the store.
            let mut env = spawn::env_snapshot(std::env::vars());
            spawn::harness_env(&mut env, config, &config.agent);
            trust::store_in(&env)
        })
        .flatten();
    let stale = store
        .as_deref()
        .and_then(|store| trust::stale_trees(store).ok())
        .unwrap_or_default();

    Ok(Findings {
        tmux: tmux::version().ok(),
        vendor: config.agent.clone(),
        vendor_path: on_path(program(&config.agent), path.as_deref()),
        config: crate::paths::config_file()?,
        config_warnings,
        home,
        wirings,
        on_path: every_on_path("amx", path.as_deref()),
        exe: exe.canonicalize().unwrap_or(exe),
        state_error: usable(&state_root),
        dirty_handoffs: dirty_handoffs(&state_root),
        parked: parked(
            // An unreadable state root is reported by the state check.
            &derive::views(&state_root, store::now()).unwrap_or_default(),
        ),
        orphan_ids: orphan_ids(&state_root, store::now()),
        orphan_trees: orphan_trees(&state_root, dir),
        zeroed: zeroed(&state_root),
        state_root,
        server: standing_server(),
        folder: dir.map(|dir| folder(dir, store.as_deref(), config.trust)),
        store,
        stale,
    })
}

/// Records under `root` with a zero clock whose log's turns add up to more.
fn zeroed(root: &Path) -> Vec<(String, u64)> {
    derive::records(root)
        .unwrap_or_default()
        .into_iter()
        .filter(|record| record.state.worked == 0)
        .filter_map(|record| {
            let worked = derive::worked_off_the_log(&record.agent, &record.meta)?;
            (worked > 0).then_some((record.meta.id, worked))
        })
        .collect()
}

/// Write each zeroed record's clock from its log, under its writer, and return
/// how many still needed it. A record that gained a span since the reading is
/// left alone.
fn rebuild_clocks(root: &Path, zeroed: &[(String, u64)]) -> Result<usize> {
    let mut rebuilt = 0;
    for (id, worked) in zeroed {
        let agent = store::Agent::open(root, id)?;
        let writer = agent.writer()?;
        writer.observe(|state| {
            if state.worked == 0 {
                state.worked = *worked;
                rebuilt += 1;
            }
        })?;
    }
    Ok(rebuilt)
}

/// Id-named directories under `root` with no record, and their age in
/// seconds.
fn orphan_ids(root: &Path, now: u64) -> Vec<(PathBuf, u64)> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut found: Vec<(PathBuf, u64)> = entries
        .flatten()
        .filter(|entry| entry.file_name().to_str().is_some_and(crate::ids::is_valid))
        .map(|entry| entry.path())
        .filter(|dir| dir.is_dir() && !dir.join("meta.json").exists())
        .map(|dir| {
            let made = std::fs::metadata(&dir)
                .and_then(|meta| meta.modified())
                .ok()
                .and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(now, |at| at.as_secs());
            let age = now.saturating_sub(made);
            (dir, age)
        })
        .collect();
    found.sort();
    found
}

/// Trees under `.amx/worktrees` that no record names, in every repository amx
/// has cut trees in and in the one `dir` belongs to.
fn orphan_trees(root: &Path, dir: Option<&Path>) -> Vec<PathBuf> {
    let named: Vec<PathBuf> = store::list(root)
        .unwrap_or_default()
        .iter()
        .filter_map(|id| store::Agent::open(root, id).ok()?.meta().ok()?.worktree)
        .collect();
    let mut repos: BTreeSet<PathBuf> = named
        .iter()
        .filter_map(|tree| tree.parent()?.parent()?.parent().map(Path::to_path_buf))
        .collect();
    if let Some(dir) = dir
        && let Ok(repo) = worktree::main_repo(dir)
    {
        repos.insert(repo);
    }
    let same = |a: &Path, b: &Path| {
        std::fs::canonicalize(a).unwrap_or_else(|_| a.to_path_buf())
            == std::fs::canonicalize(b).unwrap_or_else(|_| b.to_path_buf())
    };
    let mut found = Vec::new();
    for repo in repos {
        let Ok(trees) = std::fs::read_dir(repo.join(".amx/worktrees")) else {
            continue;
        };
        for tree in trees.flatten().map(|entry| entry.path()) {
            if tree.is_dir() && !named.iter().any(|kept| same(kept, &tree)) {
                found.push(tree);
            }
        }
    }
    found.sort();
    found
}

/// What the vendor would do for an agent started in `dir`, read off the store
/// and git.
///
/// A store amx cannot read counts as not covering the directory.
fn folder(dir: &Path, store: Option<&Path>, trust: bool) -> Folder {
    let repo = worktree::is_linked(dir)
        .then(|| worktree::main_repo(dir).ok())
        .flatten();
    Folder {
        // Canonical, since the line prints it and `--dir .` is common.
        dir: std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf()),
        covered: store.map(|store| trust::covers(store, dir, repo.as_deref()).unwrap_or(false)),
        repo,
        trust,
    }
}

/// Handoffs under `root` that still carry the environment inline.
fn dirty_handoffs(root: &Path) -> Vec<PathBuf> {
    store::list(root)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|id| {
            let path = root.join(id).join(spawn::HANDOFF);
            carries_env(&path).then_some(path)
        })
        .collect()
}

fn carries_env(path: &Path) -> bool {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .is_some_and(|doc| doc.get("env").is_some())
}

/// Rewrite each dirty handoff without its `env` key, at the mode
/// [`spawn::write_handoff`] uses, and return how many needed it.
fn clean_handoffs(dirty: &[PathBuf]) -> Result<usize> {
    let mut cleaned = 0;
    for path in dirty {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        let mut doc: serde_json::Value =
            serde_json::from_str(&text).with_context(|| format!("reading {}", path.display()))?;
        let Some(object) = doc.as_object_mut() else {
            continue;
        };
        if object.remove("env").is_none() {
            continue;
        }
        let mut bytes = serde_json::to_vec_pretty(&doc).context("writing the handoff")?;
        bytes.push(b'\n');
        std::fs::write(path, &bytes).with_context(|| format!("writing {}", path.display()))?;
        crate::paths::keep_to_the_owner(path, crate::paths::FILE_MODE)?;
        cleaned += 1;
    }
    Ok(cleaned)
}

/// Remove each stale tree's key from the store and return how many were
/// still there.
fn forget_trees(store: &Path, stale: &[PathBuf], now: u64) -> Result<usize> {
    let mut forgotten = 0;
    for tree in stale {
        if trust::forget_tree(store, tree, now)? {
            forgotten += 1;
        }
    }
    Ok(forgotten)
}

/// The tmux server a spawn would use, when one is running.
fn standing_server() -> Option<StandingServer> {
    let server = crate::spawn::server().ok()?;
    Some(StandingServer {
        socket: server.socket().clone(),
        cwd: server.cwd()?,
    })
}

/// Agents stopped at a vendor setup screen, and which screen.
///
/// A screen the vendor's ruleset marks as setup is named by its rule.
/// Otherwise a record still `starting` (`SessionStart` alone does not move it)
/// under a screen no rule claims is [`Setup::Unread`]. That covers claude's
/// login prompt, which has no rule because measuring it means logging a real
/// claude out. It also catches a changed opening screen or a slow first frame;
/// the remedy, attach and look, is harmless either way.
fn parked(views: &[View]) -> Vec<Parked> {
    views
        .iter()
        .filter_map(|view| {
            let screen = if let Some(gate) = gate(view) {
                Setup::Gate {
                    screen: gate.name.clone(),
                    // The ruleset says it is the folder-trust question; the
                    // table says whether amx can answer it for this vendor.
                    trust: gate.kind == Some(Kind::Trust) && trust::is_vendor(runs(view)),
                }
            } else if view.state.state == Phase::Starting && view.phase() == Phase::Unknown {
                Setup::Unread
            } else {
                return None;
            };
            Some(Parked {
                id: view.id().to_string(),
                screen,
            })
        })
        .collect()
}

/// The setup rule this agent's reader claimed, if any.
///
/// The verdict must also be `waiting`: anything else means the agent is past
/// the screen or never reached it.
fn gate(view: &View) -> Option<&'static Rule> {
    if view.phase() != Phase::Waiting {
        return None;
    }
    let claimed = view.verdict.rule.as_deref()?;
    crate::rules::of(runs(view))
        .rules()
        .iter()
        .find(|rule| rule.setup && rule.name == claimed)
}

/// The command running this agent, which picks its ruleset and trust entry.
/// A record naming none falls back as [`crate::rules::of`] does.
fn runs(view: &View) -> &str {
    view.meta.agent.as_deref().unwrap_or_default()
}

/// Why amx cannot use `root`, if it cannot.
///
/// Agent directories are made with their missing parents, so the nearest
/// existing ancestor must be writable. An existing root must also be readable,
/// since readers list it. Without this check an unusable root lists as no
/// agents, the same as an unused one.
fn usable(root: &Path) -> Option<String> {
    let mut dir = root;
    while !dir.exists() {
        dir = dir.parent()?;
    }

    let mut needs = nix::unistd::AccessFlags::W_OK | nix::unistd::AccessFlags::X_OK;
    if dir == root {
        needs |= nix::unistd::AccessFlags::R_OK;
    }

    let why = nix::unistd::access(dir, needs).err()?.desc();
    Some(if dir == root {
        format!("{} is not readable and writable: {why}", dir.display())
    } else {
        format!(
            "{} would be made in {}, which is not writable: {why}",
            root.display(),
            dir.display()
        )
    })
}

/// Run the verb against the machine, and against `dir` when given.
pub fn from_env(fix: bool, dir: Option<&Path>) -> Result<i32> {
    if let Some(dir) = dir
        && !dir.is_dir()
    {
        anyhow::bail!(
            "{} is not a directory an agent could start in",
            dir.display()
        );
    }
    let cwd = std::env::current_dir().ok();
    let root = crate::paths::state_root()?;
    let (config, warnings) = config_in(dir, cwd.as_deref(), &root);
    let found = gather(&config, warnings, dir)?;
    let mut out = std::io::stdout().lock();
    run(&found, fix, crate::store::now(), &mut out)
}

/// The config an agent started in `dir`, else `cwd`, would run under, since a
/// project's file names the agent worth checking. With neither, the person's
/// file alone.
fn config_in(dir: Option<&Path>, cwd: Option<&Path>, root: &Path) -> (Config, Vec<String>) {
    match dir.or(cwd) {
        Some(here) => crate::config::for_dir_in(here, root),
        None => crate::config::load(),
    }
}

/// The first word of a configured command, path included.
fn program(command: &str) -> &str {
    command.split_whitespace().next().unwrap_or(command)
}

/// Where `program` resolves on `path`.
fn on_path(program: &str, path: Option<&OsStr>) -> Option<PathBuf> {
    // A command with a slash is a path; a shell would not search for it.
    if program.contains('/') {
        let named = PathBuf::from(program);
        return runnable(&named).then_some(named);
    }

    let path = path?;
    std::env::split_paths(path)
        .map(|dir| dir.join(program))
        .find(|candidate| runnable(candidate))
}

/// Every runnable `program` on `path`, in search order, each file once however
/// many names reach it.
fn every_on_path(program: &str, path: Option<&OsStr>) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = Vec::new();
    for dir in path.map(std::env::split_paths).into_iter().flatten() {
        let candidate = dir.join(program);
        if !runnable(&candidate) {
            continue;
        }
        let file = candidate.canonicalize().unwrap_or(candidate);
        if !found.contains(&file) {
            found.push(file);
        }
    }
    found
}

/// Whether `path` is an executable file.
fn runnable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vendor::Vendor;
    use crate::vendor::second::SECOND;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::TempDir;

    /// The screens a vendor's ruleset marks as setup gates.
    fn gates(agent: &str) -> Vec<&'static Rule> {
        crate::rules::of(agent)
            .rules()
            .iter()
            .filter(|rule| rule.setup)
            .collect()
    }

    /// A rule name from a vendor's ruleset, by meaning and whether it is a
    /// gate. Names are looked up so none is copied into this file.
    fn screen(agent: &str, means: Phase, gate: bool) -> &'static str {
        crate::rules::of(agent)
            .rules()
            .iter()
            .find(|rule| rule.state == means && rule.setup == gate)
            .map(|rule| rule.name.as_str())
            .unwrap_or_else(|| panic!("{agent:?} draws no such screen"))
    }

    /// One agent's wiring, present or missing, where its entry puts it.
    fn wiring(vendor: &'static str, there: bool) -> VendorWiring {
        let hooks = registry::entry(vendor).and_then(|v| v.hooks.as_ref());
        VendorWiring {
            vendor,
            hooks,
            wire: hooks.map_or_else(
                || PathBuf::from("/home/dev"),
                |h| install::wire_path(&h.wire, Path::new("/home/dev"), &install::no_env),
            ),
            wired: install::Wired::File {
                present: there,
                current: there,
            },
            opt_in: hooks.map_or_else(Vec::new, |h| {
                h.opt_in
                    .iter()
                    .map(|wire| {
                        (
                            install::wire_path(wire, Path::new("/home/dev"), &install::no_env),
                            install::Wired::File {
                                present: there,
                                current: there,
                            },
                        )
                    })
                    .collect()
            }),
        }
    }

    fn healthy() -> Findings {
        Findings {
            tmux: Some((3, 5)),
            vendor: "claude".to_string(),
            vendor_path: Some(PathBuf::from("/usr/local/bin/claude")),
            config: PathBuf::from("/home/dev/.config/amx/config.toml"),
            config_warnings: Vec::new(),
            home: PathBuf::from("/home/dev"),
            wirings: vec![wiring("claude", true)],
            exe: PathBuf::from("/home/dev/.cargo/bin/amx"),
            on_path: vec![PathBuf::from("/home/dev/.cargo/bin/amx")],
            state_root: PathBuf::from("/home/dev/.local/state/amx/agents"),
            state_error: None,
            dirty_handoffs: Vec::new(),
            parked: Vec::new(),
            server: None,
            store: Some(PathBuf::from("/home/dev/.claude.json")),
            stale: Vec::new(),
            folder: None,
            orphan_ids: Vec::new(),
            orphan_trees: Vec::new(),
            zeroed: Vec::new(),
        }
    }

    #[test]
    fn orphan_ids_and_trees_are_named_and_only_old_ids_are_fixed() {
        let root = TempDir::new().unwrap();
        let young = root.path().join("fix-login-a1b");
        let old = root.path().join("tidy-b2c");
        std::fs::create_dir_all(&young).unwrap();
        std::fs::create_dir_all(&old).unwrap();
        let tree = root.path().join("repo/.amx/worktrees/lost-c3d");
        std::fs::create_dir_all(&tree).unwrap();
        let found = Findings {
            orphan_ids: vec![(young.clone(), 30), (old.clone(), 900)],
            orphan_trees: vec![tree.clone()],
            ..healthy()
        };

        let orphans = check(&found, "orphans");
        assert!(
            orphans.found.contains("2 id directories have no record"),
            "{}",
            orphans.found
        );
        assert!(orphans.found.contains("lost-c3d"), "{}", orphans.found);
        assert!(orphans.remedy.as_deref().unwrap().contains("--fix"));

        let (code, said) = said(&found, true);
        assert_eq!(code, exit::FAILURE, "the tree is still there to look at");
        assert!(
            said.contains("removed 1 id directory with no record"),
            "{said}"
        );
        assert!(!old.exists(), "the old one went");
        assert!(young.exists(), "a spawn still starting keeps its claim");
        assert!(tree.exists(), "and a tree is never taken");
    }

    #[test]
    fn orphan_ids_are_the_directories_with_no_record_in_them() {
        let root = TempDir::new().unwrap();
        std::fs::create_dir_all(root.path().join("fix-login-a1b")).unwrap();
        std::fs::create_dir_all(root.path().join("kept-b2c")).unwrap();
        std::fs::write(root.path().join("kept-b2c/meta.json"), "{}").unwrap();
        std::fs::create_dir_all(root.path().join("Not An Id")).unwrap();
        let found = orphan_ids(root.path(), store::now());
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].0.ends_with("fix-login-a1b"));
    }

    #[test]
    fn a_zeroed_clock_with_turns_in_its_log_is_named_and_rebuilt() {
        let root = TempDir::new().unwrap();
        let record = |id: &str, kinds: &[(u64, &str)]| {
            let meta: store::Meta = serde_json::from_value(serde_json::json!({
                "id": id,
                "task": "fix the login bug",
                "agent": "claude",
                "dir": "/srv/app",
                "socket": {"name": "amx"},
                "pane": "%1",
                "created": 900,
            }))
            .unwrap();
            let agent = store::Agent::create(root.path(), &meta).unwrap();
            let writer = agent.writer().unwrap();
            for (at, kind) in kinds {
                writer
                    .append(&store::Event {
                        at: *at,
                        kind: kind.to_string(),
                        payload: serde_json::json!({"hook_event_name": kind}),
                    })
                    .unwrap();
            }
            writer
                .observe(|s| {
                    s.state = Phase::Done;
                    s.ended = 90_000;
                })
                .unwrap();
            drop(writer);
            agent
        };
        let zeroed = record(
            "zeroed-a1b",
            &[(1_010, "UserPromptSubmit"), (1_070, "Stop")],
        );
        // A log with no turns gives nothing to rebuild from.
        record("quiet-b2c", &[(1_000, "SessionStart")]);

        let found = Findings {
            zeroed: super::zeroed(root.path()),
            state_root: root.path().to_path_buf(),
            ..healthy()
        };
        assert_eq!(found.zeroed, vec![("zeroed-a1b".to_string(), 60)]);
        let clock = check(&found, "state");
        assert!(clock.found.contains("zeroed-a1b"), "{}", clock.found);
        assert!(clock.remedy.as_deref().unwrap().contains("--fix"));

        let (code, printed) = said(&found, true);
        assert_eq!(code, exit::OK, "{printed}");
        assert!(
            printed.contains("rebuilt the clock of 1 agent"),
            "{printed}"
        );
        assert_eq!(zeroed.state().unwrap().worked, 60);
        assert!(super::zeroed(root.path()).is_empty());
    }

    /// An agent as a reader returns it. The record is deserialised so new
    /// `Meta` fields need no change here.
    ///
    /// `agent` picks the ruleset; `None` falls back to the default vendor.
    fn view(
        agent: Option<&str>,
        id: &str,
        recorded: Phase,
        seen: Phase,
        rule: Option<&str>,
    ) -> derive::View {
        derive::View {
            meta: serde_json::from_value(serde_json::json!({
                "id": id,
                "task": "fix the login bug",
                "agent": agent,
                "dir": "/srv/app",
                "socket": {"name": "amx"},
                "pane": "%1",
                "created": 1,
            }))
            .expect("the record amx writes at spawn"),
            state: store::State {
                state: recorded,
                ..store::State::default()
            },
            verdict: derive::Verdict {
                phase: seen,
                evidence: derive::Evidence::Screen,
                rule: rule.map(str::to_string),
                age: 40,
                worked: 40,
            },
            doing: None,
        }
    }

    /// Whether permission tests can run: root bypasses mode bits.
    fn not_root() -> bool {
        if nix::unistd::Uid::effective().is_root() {
            eprintln!("skipping: running as root, which every directory lets in");
            return false;
        }
        true
    }

    /// Run git with no user config and a fixed identity.
    fn git(dir: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .current_dir(dir)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_AUTHOR_NAME", "amx tests")
            .env("GIT_AUTHOR_EMAIL", "tests@example.invalid")
            .env("GIT_COMMITTER_NAME", "amx tests")
            .env("GIT_COMMITTER_EMAIL", "tests@example.invalid")
            .output()
            .unwrap_or_else(|e| panic!("git {args:?}: {e}"));
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn check(found: &Findings, name: &str) -> Check {
        report(found)
            .into_iter()
            .find(|check| check.name == name)
            .unwrap_or_else(|| panic!("no check named {name}"))
    }

    fn said(found: &Findings, fix: bool) -> (i32, String) {
        let mut out = Vec::new();
        let code = run(found, fix, 1, &mut out).unwrap();
        (code, String::from_utf8(out).unwrap())
    }

    #[test]
    fn doctor_says_nothing_is_wrong_when_nothing_is() {
        let checks = report(&healthy());
        assert!(checks.iter().all(Check::is_ok), "{checks:#?}");
        assert_eq!(
            checks.len(),
            10,
            "tmux, the vendor, the config, the hooks, amx, the state root, env, setup, the store, \
             the orphans"
        );

        let (code, printed) = said(&healthy(), false);
        assert_eq!(code, exit::OK);
        assert!(printed.contains("tmux"), "{printed}");
    }

    /// A server whose cwd is `path`, stale or not.
    fn standing(path: &str, stale: bool) -> StandingServer {
        StandingServer {
            socket: crate::tmux::Socket::Name("default".to_string()),
            cwd: crate::tmux::ServerCwd {
                pid: 27267,
                path: PathBuf::from(path),
                stale,
            },
        }
    }

    #[test]
    fn doctor_says_nothing_about_a_server_it_cannot_see() {
        // No server, or no way to read its cwd: no line at all.
        let found = healthy();
        assert!(found.server.is_none());
        assert!(
            !report(&found).iter().any(|check| check.name == "server"),
            "no line at all, rather than a green one nobody measured"
        );
        assert_eq!(said(&found, false).0, exit::OK);
    }

    #[test]
    fn doctor_passes_a_server_standing_somewhere_that_is_still_there() {
        let mut found = healthy();
        found.server = Some(standing("/home/dev/src/app", false));

        let server = check(&found, "server");
        assert!(server.is_ok(), "{server:?}");
        assert!(
            server.found.contains("/home/dev/src/app"),
            "{}",
            server.found
        );
        assert_eq!(said(&found, false).0, exit::OK);
    }

    #[test]
    fn doctor_names_a_server_whose_directory_was_deleted() {
        // Without this check doctor passed while every new agent died within
        // a second.
        let mut found = healthy();
        found.server = Some(standing("/tmp/no-git-test", true));

        let server = check(&found, "server");
        assert!(
            server.found.contains("27267"),
            "which server: {}",
            server.found
        );
        assert!(
            server.found.contains("/tmp/no-git-test"),
            "and where it is stuck: {}",
            server.found
        );

        let remedy = server.remedy.as_deref().unwrap();
        assert!(
            remedy.contains("tmux -L default kill-server"),
            "a command that works on this server, not a general one: {remedy}"
        );
        assert_eq!(said(&found, false).0, exit::FAILURE);
    }

    #[test]
    fn the_restart_names_the_socket_the_server_is_actually_on() {
        // `-L default` for a server reached by path would restart the wrong
        // one.
        let mut found = healthy();
        found.server = Some(StandingServer {
            socket: crate::tmux::Socket::Path(PathBuf::from("/run/user/1000/tmux/sock")),
            ..standing("/tmp/gone", true)
        });

        let remedy = check(&found, "server").remedy.unwrap();
        assert!(
            remedy.contains("tmux -S /run/user/1000/tmux/sock kill-server"),
            "{remedy}"
        );
    }

    #[test]
    fn doctor_names_the_floor_when_tmux_is_too_old() {
        let mut found = healthy();
        found.tmux = Some((3, 0));

        let tmux = check(&found, "tmux");
        let remedy = tmux.remedy.as_deref().unwrap();
        assert!(remedy.contains("3.2"), "{remedy}");
        assert!(tmux.found.contains("3.0"), "{}", tmux.found);
        assert_eq!(said(&found, false).0, exit::FAILURE);
    }

    #[test]
    fn doctor_names_tmux_when_there_is_none() {
        let mut found = healthy();
        found.tmux = None;

        let tmux = check(&found, "tmux");
        assert!(tmux.remedy.as_deref().unwrap().contains("tmux"));
    }

    #[test]
    fn doctor_points_at_the_config_when_the_vendor_is_not_there() {
        let mut found = healthy();
        found.vendor_path = None;
        found.vendor = "claude --model opus".to_string();

        let vendor = check(&found, "agent");
        let remedy = vendor.remedy.as_deref().unwrap();
        assert!(remedy.contains("agent"), "the config key to set: {remedy}");
        assert!(vendor.found.contains("claude"), "{}", vendor.found);
    }

    #[test]
    fn doctor_warns_of_an_agent_the_table_has_no_entry_for_and_passes_it() {
        // An unknown wrapper is read as claude: worth a note, but it passes.
        let mut found = healthy();
        found.vendor = "/opt/bin/my-agent --fast".to_string();
        found.vendor_path = Some(PathBuf::from("/opt/bin/my-agent"));
        let agent = check(&found, "agent");
        assert!(agent.is_ok(), "{agent:?}");
        assert!(
            agent
                .found
                .contains("no entry for my-agent: read as claude"),
            "{}",
            agent.found
        );

        // A path to a known vendor is that vendor, with no note.
        found.vendor = "/opt/pi/bin/pi".to_string();
        let agent = check(&found, "agent");
        assert!(agent.is_ok(), "{agent:?}");
        assert!(!agent.found.contains("no entry"), "{}", agent.found);
    }

    #[test]
    fn a_path_to_pi_is_asked_about_as_pi() {
        let home = Path::new("/home/dev");
        assert_eq!(
            wirings("/opt/pi/bin/pi", home, &install::no_env, None)
                .iter()
                .map(|w| w.vendor)
                .collect::<Vec<_>>(),
            ["pi"]
        );
    }

    /// A project whose config names `agent`, allowed under `root`.
    fn a_project_for(agent: &str, root: &Path) -> TempDir {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join(".amx")).unwrap();
        std::fs::write(
            dir.path().join(".amx/config.toml"),
            format!("agent = \"{agent}\"\n"),
        )
        .unwrap();
        let file = crate::paths::project_config(dir.path()).unwrap();
        crate::consent::allow_in(root, &file).unwrap();
        dir
    }

    #[test]
    fn doctor_asks_about_the_agent_of_the_project_it_is_pointed_at_else_stands_in() {
        // The agent, hooks and store checks follow this config, so a codex
        // project is checked as codex.
        let state = TempDir::new().unwrap();
        let root = state.path().join("agents");
        let codex = a_project_for("codex", &root);
        let pi = a_project_for("pi", &root);

        let (pointed, _) = config_in(Some(codex.path()), Some(pi.path()), &root);
        assert_eq!(
            pointed.agent, "codex",
            "--dir over the directory it stands in"
        );
        let (standing, _) = config_in(None, Some(pi.path()), &root);
        assert_eq!(standing.agent, "pi", "else the directory it stands in");

        assert!(
            wirings(
                &pointed.agent,
                Path::new("/home/dev"),
                &install::no_env,
                None
            )
            .iter()
            .any(|w| w.vendor == "codex"),
            "the hooks line is codex's"
        );
    }

    #[test]
    fn doctor_repeats_what_the_config_said_about_itself() {
        let mut found = healthy();
        found.config_warnings = vec!["ignoring unknown key `wardrobe`".to_string()];

        let config = check(&found, "config");
        assert!(config.found.contains("wardrobe"), "{}", config.found);
        assert!(
            config.remedy.as_deref().unwrap().contains("config.toml"),
            "the file to edit is named"
        );
    }

    #[test]
    fn doctor_says_a_vendor_that_reports_nothing_leaves_the_pane_to_read() {
        // A vendor without hooks has nothing to wire; amx reads its pane.
        let hooks = wiring_check(&VendorWiring {
            vendor: SECOND.name,
            hooks: None,
            wire: PathBuf::from("/home/dev"),
            wired: install::Wired::Nothing,
            opt_in: Vec::new(),
        });
        assert!(
            hooks.is_ok(),
            "nothing here is anybody's to repair: {hooks:?}"
        );
        assert!(hooks.found.contains(SECOND.name), "{}", hooks.found);
        assert!(hooks.found.contains("pane"), "{}", hooks.found);

        // A vendor with hooks is judged, and the remedy names its setup line.
        let hooks = wiring_check(&wiring("claude", false));
        assert!(!hooks.is_ok(), "{hooks:?}");
        let remedy = hooks.remedy.as_deref().unwrap();
        assert!(remedy.contains("amx setup claude"), "{remedy}");
        assert!(
            !remedy.contains("--fix"),
            "doctor repairs none of it: {remedy}"
        );
    }

    #[test]
    fn doctor_fails_hooks_whose_trust_is_missing_or_stale() {
        // codex runs a hook group only while config.toml trusts it under the
        // group's current hash, so being in hooks.json is not enough.
        const HOOKS: Hooks = Hooks {
            wire: install::HOOKS_WIRE,
            ..crate::vendor::claude::HOOKS
        };
        let home = TempDir::new().unwrap();
        let dir = install::wire_path(&HOOKS.wire, home.path(), &install::no_env);
        let judged = || {
            wiring_check(&VendorWiring {
                vendor: "codex",
                hooks: Some(&HOOKS),
                wire: dir.clone(),
                wired: install::wired(&HOOKS.wire, home.path(), &install::no_env),
                opt_in: Vec::new(),
            })
        };

        let missing = judged();
        assert!(!missing.is_ok(), "{missing:?}");
        assert_eq!(
            missing.found,
            format!(
                "codex: no hooks of amx's in {}",
                dir.join("hooks.json").display()
            )
        );
        assert_eq!(missing.remedy.as_deref(), Some("run `amx setup codex`"));

        install::install_wire(&HOOKS.wire, home.path(), &install::no_env, 1).unwrap();
        let wired = judged();
        assert!(wired.is_ok(), "{wired:?}");
        assert_eq!(
            wired.found,
            format!(
                "codex: amx's hooks in {}, trusted in {}",
                dir.join("hooks.json").display(),
                dir.join("config.toml").display()
            )
        );

        let config = dir.join("config.toml");
        let text = std::fs::read_to_string(&config).unwrap();
        let stale = text.replacen("trusted_hash = \"sha256:", "trusted_hash = \"sha256:0", 1);
        assert_ne!(stale, text);
        std::fs::write(&config, stale).unwrap();
        let untrusted = judged();
        assert!(!untrusted.is_ok(), "{untrusted:?}");
        assert!(
            untrusted.found.contains("are not trusted in"),
            "{untrusted:?}"
        );
        assert_eq!(untrusted.remedy.as_deref(), Some("run `amx setup codex`"));

        std::fs::write(&config, "").unwrap();
        assert!(!judged().is_ok(), "no trust at all");
    }

    #[test]
    fn a_wrapper_somebody_wrote_is_judged_as_the_vendor_underneath_it_is() {
        // An unknown command is checked as the first vendor, whose files a
        // wrapper loads.
        let asked = wirings("my-claude", Path::new("/home/dev"), &install::no_env, None);
        assert_eq!(
            asked.iter().map(|w| w.vendor).collect::<Vec<_>>(),
            ["claude"],
            "and only that one, since no other agent is on this PATH"
        );
    }

    #[test]
    fn an_agent_this_machine_has_not_got_is_not_asked_about() {
        // An uninstalled pi gets no line. The configured agent always gets
        // one, so its hooks line can name the setup command.
        let dir = TempDir::new().unwrap();
        let pi = dir.path().join("pi");
        std::fs::write(&pi, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&pi, std::fs::Permissions::from_mode(0o755)).unwrap();

        let home = Path::new("/home/dev");
        assert_eq!(
            wirings("claude", home, &install::no_env, None)
                .iter()
                .map(|w| w.vendor)
                .collect::<Vec<_>>(),
            ["claude"]
        );
        assert_eq!(
            wirings(
                "claude",
                home,
                &install::no_env,
                Some(dir.path().as_os_str())
            )
            .iter()
            .map(|w| w.vendor)
            .collect::<Vec<_>>(),
            ["claude", "pi"],
            "pi is installed here, so it is asked about too"
        );
    }

    /// pi with its hooks set here, independent of the table entry.
    static PI_WIRED: Vendor = Vendor {
        hooks: Some(crate::vendor::pi::HOOKS),
        ..crate::vendor::pi::VENDOR
    };

    #[test]
    fn doctor_judges_a_file_wire_by_the_file_that_is_there() {
        let mut found = VendorWiring {
            vendor: PI_WIRED.name,
            hooks: PI_WIRED.hooks.as_ref(),
            wire: PathBuf::from("/home/dev/.pi/agent/extensions/amx.ts"),
            wired: install::Wired::File {
                present: false,
                current: false,
            },
            opt_in: Vec::new(),
        };
        let hooks = wiring_check(&found);
        assert!(!hooks.is_ok());
        assert!(hooks.found.contains("no extension"), "{}", hooks.found);
        // The remedy names this agent, so somebody with two runs the right
        // one.
        assert_eq!(
            hooks.remedy.as_deref(),
            Some("run `amx setup pi`"),
            "{hooks:?}"
        );

        found.wired = install::Wired::File {
            present: true,
            current: false,
        };
        let hooks = wiring_check(&found);
        assert!(!hooks.is_ok());
        assert!(
            hooks.found.contains("not the extension this amx ships"),
            "{}",
            hooks.found
        );
        assert_eq!(hooks.remedy.as_deref(), Some("run `amx setup pi`"));

        found.wired = install::Wired::File {
            present: true,
            current: true,
        };
        let hooks = wiring_check(&found);
        assert!(hooks.is_ok(), "{hooks:?}");
        assert!(hooks.found.contains("amx.ts"), "{}", hooks.found);
    }

    #[test]
    fn doctor_finds_a_program_the_way_a_shell_would() {
        let first = TempDir::new().unwrap();
        let second = TempDir::new().unwrap();
        let claude = second.path().join("claude");
        std::fs::write(&claude, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o755)).unwrap();

        let path = std::ffi::OsString::from(format!(
            "{}:{}",
            first.path().display(),
            second.path().display()
        ));
        assert_eq!(on_path("claude", Some(&path)), Some(claude.clone()));
        assert_eq!(on_path("nowhere", Some(&path)), None);
        assert_eq!(on_path("claude", None), None);

        // A path is checked directly, not searched for.
        assert_eq!(
            on_path(&claude.to_string_lossy(), None),
            Some(claude.clone())
        );
        assert_eq!(on_path("/nowhere/claude", None), None);
    }

    #[test]
    fn doctor_will_not_run_a_file_that_is_not_executable() {
        let dir = TempDir::new().unwrap();
        let claude = dir.path().join("claude");
        std::fs::write(&claude, "text").unwrap();
        std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o644)).unwrap();

        let path = std::ffi::OsString::from(dir.path().to_string_lossy().to_string());
        assert_eq!(on_path("claude", Some(&path)), None);
    }

    #[test]
    fn doctor_names_a_second_amx_on_the_path() {
        // Two installs: `amx doctor --fix` under the stale one judged the
        // stale extension against its own copy and passed.
        let stale = PathBuf::from("/home/dev/.cargo/bin/amx");
        let fresh = PathBuf::from("/home/dev/.local/bin/amx");

        let mut found = healthy();
        found.exe = stale.clone();
        found.on_path = vec![stale.clone(), fresh.clone()];
        let amx = check(&found, "amx");
        assert!(!amx.is_ok(), "{amx:?}");
        assert!(
            amx.found.contains("/home/dev/.local/bin/amx"),
            "the other one is named: {}",
            amx.found
        );
        assert!(
            amx.remedy.as_deref().unwrap().contains("symlink"),
            "and one install answering to both names is the way out: {amx:?}"
        );
        assert_eq!(said(&found, false).0, exit::FAILURE);

        // Under the fresh one, the PATH still finds the stale one first, and
        // hand-started agents report there.
        found.exe = fresh;
        let amx = check(&found, "amx");
        assert!(!amx.is_ok(), "{amx:?}");
        assert!(
            amx.found.contains("/home/dev/.cargo/bin/amx"),
            "{}",
            amx.found
        );
        assert!(amx.found.contains("first"), "{}", amx.found);
    }

    #[test]
    fn doctor_passes_the_one_amx_the_path_finds_and_names_one_it_cannot() {
        let amx = check(&healthy(), "amx");
        assert!(amx.is_ok(), "{amx:?}");
        assert!(
            amx.found.contains("/home/dev/.cargo/bin/amx"),
            "{}",
            amx.found
        );

        // Run by path with no amx on the PATH: a hand-started agent has
        // nowhere to report.
        let mut found = healthy();
        found.on_path = Vec::new();
        let amx = check(&found, "amx");
        assert!(!amx.is_ok(), "{amx:?}");
        assert!(
            amx.remedy
                .as_deref()
                .unwrap()
                .contains("/home/dev/.cargo/bin"),
            "the directory to put on the PATH: {amx:?}"
        );
        assert!(
            names_no_other_vendor(amx.remedy.as_deref().unwrap(), &found.vendor),
            "the check is about amx, not about a vendor: {amx:?}"
        );
    }

    /// Whether `said` names no table vendor other than `in_hand`.
    fn names_no_other_vendor(said: &str, in_hand: &str) -> bool {
        let words: Vec<&str> = said
            .split(|c: char| !c.is_ascii_alphanumeric() && c != '-')
            .collect();
        registry::entries()
            .iter()
            .filter(|vendor| vendor.name != registry::program(in_hand))
            .all(|vendor| !words.contains(&vendor.name))
    }

    #[test]
    fn every_amx_on_the_path_is_one_file_however_it_is_named() {
        let first = TempDir::new().unwrap();
        let second = TempDir::new().unwrap();
        let third = TempDir::new().unwrap();
        let fourth = TempDir::new().unwrap();
        let program = |dir: &TempDir, mode: u32| {
            let amx = dir.path().join("amx");
            std::fs::write(&amx, "#!/bin/sh\n").unwrap();
            std::fs::set_permissions(&amx, std::fs::Permissions::from_mode(mode)).unwrap();
            amx
        };
        let real = program(&first, 0o755);
        // A symlink: one install under two names.
        std::os::unix::fs::symlink(&real, second.path().join("amx")).unwrap();
        // Not executable, so not a program.
        program(&third, 0o644);
        // A second install: the fault.
        let other = program(&fourth, 0o755);

        let path = std::env::join_paths([second.path(), first.path(), third.path(), fourth.path()])
            .unwrap();
        assert_eq!(
            every_on_path("amx", Some(&path)),
            vec![real.canonicalize().unwrap(), other.canonicalize().unwrap()],
            "in the PATH's order, each once"
        );
        assert_eq!(every_on_path("amx", None), Vec::<PathBuf>::new());
    }

    #[test]
    fn doctor_reads_the_program_out_of_a_command_with_arguments() {
        assert_eq!(program("claude"), "claude");
        assert_eq!(program("claude --model opus"), "claude");
        assert_eq!(
            program("/usr/local/bin/claude --resume"),
            "/usr/local/bin/claude"
        );
    }

    #[test]
    fn doctor_names_an_unwritable_state_root_instead_of_saying_no_agents() {
        // An unwritable root and an unused one both list no agents.
        let mut found = healthy();
        found.state_root = PathBuf::from("/srv/amx/agents");
        found.state_error =
            Some("/srv/amx/agents cannot be written to: Permission denied".to_string());

        let state = check(&found, "state");
        assert!(state.found.contains("/srv/amx/agents"), "{}", state.found);
        assert!(state.found.contains("Permission denied"), "{}", state.found);
        assert!(
            state.remedy.is_some(),
            "and something to do about it: {state:?}"
        );
        assert_eq!(said(&found, false).0, exit::FAILURE);
    }

    #[test]
    fn a_state_root_that_is_not_there_yet_is_judged_by_the_directory_it_would_go_in() {
        let dir = TempDir::new().unwrap();
        assert_eq!(usable(&dir.path().join("state/amx/agents")), None);
    }

    #[test]
    fn a_state_root_nobody_can_write_to_names_it_and_says_why() {
        if !not_root() {
            return;
        }
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("agents");
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o500)).unwrap();

        let why = usable(&root).expect("read only, so a new agent has nowhere to go");
        assert!(why.contains(&root.display().to_string()), "{why}");
        assert!(why.to_lowercase().contains("permission denied"), "{why}");
    }

    #[test]
    fn a_state_root_that_cannot_be_made_names_the_directory_that_refused() {
        if !not_root() {
            return;
        }
        let dir = TempDir::new().unwrap();
        let closed = dir.path().join("state");
        std::fs::create_dir(&closed).unwrap();
        std::fs::set_permissions(&closed, std::fs::Permissions::from_mode(0o500)).unwrap();

        // The root is two levels below the read-only directory, which is the
        // one named.
        let why = usable(&closed.join("amx/agents")).expect("nowhere to make it");
        assert!(why.contains(&closed.display().to_string()), "{why}");
    }

    /// An agent record holding `handoff`. `store::list` needs only `meta.json`
    /// to exist, so it need not parse.
    fn a_record(root: &Path, id: &str, handoff: &[u8]) -> PathBuf {
        let dir = root.join(id);
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(dir.join("meta.json"), "{}").unwrap();
        let handoff_path = dir.join(spawn::HANDOFF);
        std::fs::write(&handoff_path, handoff).unwrap();
        handoff_path
    }

    #[test]
    fn env_scan_finds_a_dirty_handoff_and_cleans_it_leaving_a_clean_one_alone() {
        let root = TempDir::new().unwrap();
        let dirty = a_record(
            root.path(),
            "fix-login-a1b",
            br#"{"task":"fix the login bug","command":["claude"],"env":{"PATH":"/usr/bin"}}"#,
        );
        let clean_before = b"{\"task\":\"port the importer\",\"command\":[\"claude\"]}\n";
        let clean = a_record(root.path(), "port-importer-c2d", clean_before);

        let found = dirty_handoffs(root.path());
        assert_eq!(
            found,
            vec![dirty.clone()],
            "the clean record is not among them"
        );

        assert_eq!(clean_handoffs(&found).unwrap(), 1);

        let after: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&dirty).unwrap()).unwrap();
        assert!(after.get("env").is_none(), "the stray key is gone");
        assert_eq!(after["task"], "fix the login bug");
        let mode = std::fs::metadata(&dirty).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "kept to its owner");

        assert_eq!(
            std::fs::read_to_string(&clean).unwrap().as_bytes(),
            clean_before,
            "a clean record is left alone"
        );
    }

    #[test]
    fn doctor_fix_cleans_the_environment_out_of_a_handoff_and_says_how_many() {
        let dir = TempDir::new().unwrap();
        let handoff = dir.path().join("handoff.json");
        std::fs::write(
            &handoff,
            br#"{"task":"fix the login bug","command":["claude"],"env":{"PATH":"/usr/bin"}}"#,
        )
        .unwrap();

        let mut found = healthy();
        found.dirty_handoffs = vec![handoff.clone()];

        let env = check(&found, "env");
        assert!(!env.is_ok(), "{env:?}");
        assert!(env.remedy.as_deref().unwrap().contains("--fix"), "{env:?}");

        let (code, printed) = said(&found, true);
        assert_eq!(code, exit::OK, "nothing is wrong any more: {printed}");
        assert!(printed.contains('1'), "how many it cleaned: {printed}");

        let after: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&handoff).unwrap()).unwrap();
        assert!(after.get("env").is_none(), "{after}");
    }

    /// A claude trust store with two of the person's own keys and one key per
    /// tree in `trees`.
    fn a_store(dir: &TempDir, trees: &[&Path]) -> PathBuf {
        let store = dir.path().join(".claude.json");
        let mut projects = serde_json::Map::new();
        projects.insert("/src/app".to_string(), serde_json::json!({}));
        projects.insert("/src/other".to_string(), serde_json::json!({}));
        for tree in trees {
            projects.insert(
                tree.display().to_string(),
                serde_json::json!({ "hasTrustDialogAccepted": true }),
            );
        }
        let document = serde_json::json!({ "numStartups": 412, "projects": projects });
        std::fs::write(&store, serde_json::to_string_pretty(&document).unwrap()).unwrap();
        store
    }

    /// A tree amx would have cut for `id`, under a repository that is not there.
    fn a_gone_tree(id: &str) -> PathBuf {
        PathBuf::from(format!("/src/app/.amx/worktrees/{id}"))
    }

    #[test]
    fn doctor_counts_the_trees_the_vendors_store_still_names_after_they_went() {
        // The store gains a key per directory the vendor starts in, so every
        // removed agent tree leaves one behind.
        let mut found = healthy();
        found.store = Some(PathBuf::from("/home/dev/.claude.json"));
        let store = check(&found, "store");
        assert!(store.is_ok(), "nothing of amx's is left in it: {store:?}");
        assert!(
            store.found.contains("/home/dev/.claude.json"),
            "the file that was read: {}",
            store.found
        );

        found.stale = vec![a_gone_tree("fix-login-a1b"), a_gone_tree("port-cli-b91")];
        let store = check(&found, "store");
        assert!(!store.is_ok(), "{store:?}");
        assert!(store.found.contains('2'), "how many: {}", store.found);
        assert!(
            store
                .remedy
                .as_deref()
                .unwrap()
                .contains("amx doctor --fix"),
            "{store:?}"
        );
        assert_eq!(said(&found, false).0, exit::FAILURE);
    }

    #[test]
    fn doctor_asks_nothing_of_a_vendor_that_keeps_no_store_amx_writes() {
        // A vendor answered by an argv flag, as pi is, has no store to prune.
        let mut found = healthy();
        found.vendor = SECOND.name.to_string();
        found.store = None;

        let store = check(&found, "store");
        assert!(store.is_ok(), "{store:?}");
        assert!(store.found.contains(SECOND.name), "{}", store.found);
        assert_eq!(said(&found, false).0, exit::OK);
    }

    #[test]
    fn doctor_fix_forgets_the_stale_trees_and_leaves_the_rest_of_the_store_alone() {
        let dir = TempDir::new().unwrap();
        let gone = a_gone_tree("fix-login-a1b");
        let store = a_store(&dir, &[&gone]);
        let before = std::fs::read_to_string(&store).unwrap();

        let mut found = healthy();
        found.store = Some(store.clone());
        found.stale = vec![gone.clone()];

        let (code, printed) = said(&found, true);
        assert_eq!(code, exit::OK, "nothing is left to prune: {printed}");
        assert!(printed.contains("forgot 1 tree"), "how many: {printed}");
        assert!(
            printed.contains(&store.display().to_string()),
            "and out of which file: {printed}"
        );

        let after: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&store).unwrap()).unwrap();
        assert_eq!(after["projects"].get(gone.display().to_string()), None);
        assert!(after["projects"].get("/src/app").is_some(), "{after}");
        assert!(after["projects"].get("/src/other").is_some(), "{after}");
        assert_eq!(after["numStartups"], 412);

        let copy = install::latest_backup(&store).unwrap().expect("a copy");
        assert_eq!(
            std::fs::read_to_string(&copy).unwrap(),
            before,
            "the file as it was, keys and all"
        );
    }

    /// Findings for a `--dir` directory with the given answers.
    fn asked(repo: Option<&str>, covered: Option<bool>, trust: bool) -> Findings {
        Findings {
            folder: Some(Folder {
                dir: PathBuf::from("/srv/app/plan/t1"),
                repo: repo.map(PathBuf::from),
                covered,
                trust,
            }),
            ..healthy()
        }
    }

    #[test]
    fn doctor_says_whether_an_agent_started_in_a_directory_would_meet_the_trust_screen() {
        // Only asked with --dir, so a caller can check before starting an
        // agent it will never attach to. Every answer shows in the exit code.
        assert!(
            report(&healthy()).iter().all(|check| check.name != "trust"),
            "nothing was asked about, so nothing is said"
        );

        let fine = check(&asked(None, Some(true), false), "trust");
        assert!(fine.is_ok(), "{fine:?}");
        assert!(fine.found.contains("/srv/app/plan/t1"), "{}", fine.found);

        let plain = asked(None, Some(false), true);
        let stopped = check(&plain, "trust");
        assert!(
            stopped.found.contains("folder-trust screen"),
            "{}",
            stopped.found
        );
        let remedy = stopped.remedy.as_deref().unwrap();
        assert!(remedy.contains("/srv/app/plan/t1"), "{remedy}");
        assert!(
            !remedy.contains("trust = true"),
            "the key answers for a linked worktree and this is not one: {remedy}"
        );
        assert_eq!(said(&plain, false).0, exit::FAILURE);

        let linked = check(&asked(Some("/srv/app"), Some(false), false), "trust");
        let remedy = linked.remedy.as_deref().unwrap();
        assert!(
            remedy.contains("/srv/app") && remedy.contains("trust = true"),
            "the repository covers every tree in it, and so does the key: {remedy}"
        );

        let answered = check(&asked(Some("/srv/app"), Some(false), true), "trust");
        assert!(answered.is_ok(), "{answered:?}");
        assert!(answered.found.contains("amx answers"), "{}", answered.found);

        let unread = check(&asked(None, None, false), "trust");
        assert!(unread.is_ok(), "{unread:?}");
        assert!(unread.found.contains("no store"), "{}", unread.found);
    }

    #[test]
    fn doctor_reads_the_directory_it_was_asked_about_and_writes_nothing() {
        let dir = TempDir::new().unwrap();
        let repo = dir.path().join("app");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["commit", "-q", "--allow-empty", "-m", "first"]);
        let theirs = dir.path().join("workflow/plan/t1");
        git(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                &theirs.to_string_lossy(),
                "-b",
                "t1",
            ],
        );
        let store = dir.path().join(".claude.json");
        let bytes = serde_json::to_string_pretty(&serde_json::json!({
            "numStartups": 412,
            "projects": { trust::key_for(&repo): { "hasTrustDialogAccepted": true } }
        }))
        .unwrap();
        std::fs::write(&store, &bytes).unwrap();

        let looked = folder(&theirs, Some(&store), false);
        assert_eq!(
            looked.repo.as_deref().map(trust::key_for),
            Some(trust::key_for(&repo)),
            "the repository the vendor resolves the tree to"
        );
        assert_eq!(looked.covered, Some(true), "and its entry covers the tree");
        assert_eq!(
            folder(&repo, Some(&store), false).repo,
            None,
            "a checkout belongs to nothing above it"
        );

        std::fs::write(&store, "{}\n").unwrap();
        assert_eq!(folder(&theirs, Some(&store), true).covered, Some(false));
        assert_eq!(folder(&theirs, None, true).covered, None);
        assert_eq!(
            std::fs::read_to_string(&store).unwrap(),
            "{}\n",
            "a look, not a write"
        );
        assert!(
            !store.with_extension("json.lock").exists()
                && !dir.path().join(".claude.json.lock").exists(),
            "and no lock was taken to look"
        );
    }

    #[test]
    fn doctor_names_the_agent_stopped_at_the_vendors_trust_question() {
        // The rule name comes from claude's ruleset, never from this file.
        let gate = screen("claude", Phase::Waiting, true);
        let mut found = healthy();
        found.parked = parked(&[view(
            Some("claude"),
            "fix-auth-2k3",
            Phase::Starting,
            Phase::Waiting,
            Some(gate),
        )]);

        let stopped = check(&found, "gate");
        assert!(stopped.found.contains("fix-auth-2k3"), "{}", stopped.found);
        assert!(
            stopped.found.contains(gate),
            "the screen, as the vendor drawing it names it: {}",
            stopped.found
        );
        let remedy = stopped.remedy.as_deref().unwrap();
        assert!(remedy.contains("amx attach fix-auth-2k3"), "{remedy}");
        assert!(
            remedy.contains("trust = true"),
            "the key that makes it never happen again is named: {remedy}"
        );
        assert_eq!(said(&found, false).0, exit::FAILURE);
    }

    #[test]
    fn doctor_names_an_agent_stopped_at_another_vendors_gate() {
        // pi's gates are its own screens; a check keyed on claude's rule
        // names would miss them.
        let gates = gates("pi");
        assert!(!gates.is_empty(), "pi's document marks its own gates");

        for gate in gates {
            let mut found = healthy();
            found.parked = parked(&[view(
                Some("pi"),
                "port-cli-b91",
                Phase::Starting,
                Phase::Waiting,
                Some(&gate.name),
            )]);

            let stopped = check(&found, "gate");
            assert!(stopped.found.contains("port-cli-b91"), "{}", stopped.found);
            assert!(
                stopped.found.contains(gate.name.as_str()),
                "{}",
                stopped.found
            );
            let remedy = stopped.remedy.as_deref().unwrap();
            assert!(remedy.contains("amx attach port-cli-b91"), "{remedy}");

            // The trust key is offered only for the folder-trust question.
            if gate.kind != Some(Kind::Trust) {
                assert!(!remedy.contains("trust = true"), "{remedy}");
            }
        }
    }

    #[test]
    fn the_offer_to_answer_a_gate_is_made_for_the_vendors_amx_answers_it_for() {
        // A wrapper and a record naming no command are read against claude's
        // screens, but amx writes trust only for a command it has an entry
        // for, so the key is not offered.
        for command in [Some("my-claude"), None] {
            let mut found = healthy();
            found.parked = parked(&[view(
                command,
                "fix-auth-2k3",
                Phase::Starting,
                Phase::Waiting,
                Some(screen(command.unwrap_or_default(), Phase::Waiting, true)),
            )]);

            let stopped = check(&found, "gate");
            assert!(!stopped.is_ok(), "the agent is still stopped: {stopped:?}");
            let remedy = stopped.remedy.as_deref().unwrap();
            assert!(remedy.contains("amx attach fix-auth-2k3"), "{remedy}");
            assert!(!remedy.contains("trust = true"), "{command:?}: {remedy}");
        }
    }

    #[test]
    fn doctor_names_an_agent_the_vendor_never_let_start() {
        // How a login prompt looks from outside: a screen no rule claims, on
        // an agent that never reported.
        let mut found = healthy();
        found.parked = vec![Parked {
            id: "port-cli-b91".to_string(),
            screen: Setup::Unread,
        }];

        let stopped = check(&found, "gate");
        assert!(stopped.found.contains("port-cli-b91"), "{}", stopped.found);
        assert!(
            stopped.remedy.as_deref().unwrap().contains("amx attach"),
            "somebody has to look at it: {stopped:?}"
        );
    }

    #[test]
    fn doctor_names_every_agent_stopped_at_a_gate_and_one_to_start_with() {
        let mut found = healthy();
        found.parked = vec![
            Parked {
                id: "fix-auth-2k3".to_string(),
                screen: Setup::Gate {
                    screen: screen("claude", Phase::Waiting, true).to_string(),
                    trust: true,
                },
            },
            Parked {
                id: "port-cli-b91".to_string(),
                screen: Setup::Unread,
            },
        ];

        let stopped = check(&found, "gate");
        assert!(stopped.found.contains('2'), "how many: {}", stopped.found);
        assert!(stopped.found.contains("fix-auth-2k3"), "{}", stopped.found);
        assert!(stopped.found.contains("port-cli-b91"), "{}", stopped.found);
        assert!(
            stopped
                .remedy
                .as_deref()
                .unwrap()
                .contains("amx attach fix-auth-2k3"),
            "one of them to start with: {stopped:?}"
        );
    }

    #[test]
    fn only_an_agent_that_never_got_started_is_stopped_at_a_gate() {
        // Most records here name no command, as older records and shell
        // commands do, so their screens come from the fallback ruleset.
        let working = screen("", Phase::Working, false);
        let idle = screen("", Phase::Idle, false);
        let asking = screen("", Phase::Waiting, false);
        let gate = screen("", Phase::Waiting, true);
        // A pi gate other than folder trust: the trust offer depends on the
        // screen, not the vendor.
        let elsewhere = gates("pi")
            .into_iter()
            .find(|rule| rule.kind != Some(Kind::Trust))
            .expect("pi gates a run with more than its trust question");

        let views = vec![
            view(
                None,
                "works-a1b",
                Phase::Working,
                Phase::Working,
                Some(working),
            ),
            view(
                None,
                "asks-b2c",
                Phase::Working,
                Phase::Waiting,
                Some(asking),
            ),
            view(None, "waits-c3d", Phase::Idle, Phase::Idle, Some(idle)),
            view(
                Some("claude"),
                "trust-d4e",
                Phase::Starting,
                Phase::Waiting,
                Some(gate),
            ),
            view(None, "login-e5f", Phase::Starting, Phase::Unknown, None),
            // Interrupted mid-turn onto an unclaimed screen: it did start.
            view(None, "lost-f6g", Phase::Working, Phase::Unknown, None),
            // Started and idle at its prompt.
            view(None, "fresh-g7h", Phase::Starting, Phase::Idle, Some(idle)),
            // A pi gate on a pi record. Read against claude's ruleset it
            // would match no rule and go unreported.
            view(
                Some("pi"),
                "setup-h8i",
                Phase::Starting,
                Phase::Waiting,
                Some(&elsewhere.name),
            ),
        ];

        assert_eq!(
            parked(&views),
            vec![
                Parked {
                    id: "trust-d4e".to_string(),
                    screen: Setup::Gate {
                        screen: gate.to_string(),
                        trust: true,
                    },
                },
                Parked {
                    id: "login-e5f".to_string(),
                    screen: Setup::Unread,
                },
                Parked {
                    id: "setup-h8i".to_string(),
                    screen: Setup::Gate {
                        screen: elsewhere.name.clone(),
                        trust: false,
                    },
                },
            ]
        );
    }

    #[test]
    fn a_state_root_nobody_can_read_is_not_an_empty_one() {
        if !not_root() {
            return;
        }
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("agents");
        std::fs::create_dir(&root).unwrap();
        // Write and search but no read: listing fails, which would otherwise
        // pass for an empty root.
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o300)).unwrap();

        let why = usable(&root);
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(why.is_some(), "a root amx cannot list is a root to report");
    }
}
