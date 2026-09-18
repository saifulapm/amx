//! `amx doctor` — what amx needs from this machine, and what is missing.
//!
//! Nine things have to be true before an agent can run: a tmux new enough to
//! address panes by id, a vendor command to run, a config amx can read, amx's
//! own files where each installed agent loads them, one amx on the PATH and
//! this the one, a state root amx can keep an agent in, no handoff still
//! carrying the spawner's environment from before that moved to a file of its
//! own, no agent already stopped at a screen the vendor puts in front of the
//! work, and no tree amx cut still named in the vendor's own trust store after
//! the tree itself has gone. Each check that fails says what to do about it,
//! because a check that only says "no" leaves somebody guessing at a machine
//! they thought was fine.
//!
//! Nine kinds of check, that is, rather than nine lines. The wiring one is
//! asked of every agent this machine has and names which agent it is about, so
//! somebody with claude and pi reads two of those lines and is asked the same
//! nine things. An agent that is not installed is not a machine with something
//! missing from it and gets no line at all.
//!
//! What two of them are worth depends on the vendor, and the vendor is what
//! says. The table answers the first: one that reports nothing has no wiring to
//! be missing. The vendor's own screens document answers the second — which of
//! the screens amx can recognise stand in front of the work, so that a check
//! naming an agent stopped at one holds no list of screens of its own — and
//! whether amx knows how to answer the one it is stopped at decides what it is
//! offered. A check that asked for a repair nobody can make would send somebody
//! looking for a fault in their own machine.
//!
//! A tenth is asked only where there is something to ask it of. When a tmux
//! server is already running, and the machine can say where a process is
//! standing, doctor checks that the directory that server is standing in still
//! exists. A server holds the directory it was started in for as long as it
//! lives, and once that goes, every pane it forks starts somewhere that is not
//! there and dies at once. No server yet is not a fault, and neither is a
//! platform amx cannot ask, so both go unsaid rather than answered green.
//!
//! An eleventh is asked only when doctor is pointed at a directory, `amx --dir
//! <path> doctor`: whether an agent started there would meet its vendor's
//! folder-trust screen. That screen is drawn in front of the session every
//! hook comes from, so an agent that meets it reports nothing and sits there
//! until somebody attaches, and a caller that cannot attach, `workflow run`
//! starting a reader it will never look at, loses the agent to a question it
//! never sees. The check reads the vendor's store and asks git what the
//! directory is, and writes nothing anywhere, so that the caller can ask it at
//! the top of a run and branch on the exit code.
//!
//! `--fix` makes two repairs, and both of them are amx's own files to mend.
//! Rewriting a handoff that still carries the environment needs no asking: amx
//! wrote every one of those files itself, and taking a stray key back out of
//! one is not a change anybody could object to. Nor does forgetting a tree amx
//! cut, which is amx's own key for a directory that is not there any more, and
//! the file is copied aside before it goes.
//!
//! Wiring an agent is not among them. That writes under somebody's home, and
//! it is `amx setup` that does it, named agent by named agent; doctor says
//! which agent is unwired and prints the line that wires it.

use anyhow::{Context, Result};
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
    /// What amx found, said plainly.
    pub found: String,
    /// What to do about it, when there is something to do.
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

/// What amx found on the machine, gathered before anything is judged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Findings {
    /// The installed tmux, or `None` when there is none.
    pub tmux: Option<(u32, u32)>,
    /// The configured vendor command, and where it resolved to.
    pub vendor: String,
    pub vendor_path: Option<PathBuf>,
    /// The config file, and anything amx had to say about reading it.
    pub config: PathBuf,
    pub config_warnings: Vec<String>,
    /// The person's home, which every vendor's wiring is written under.
    pub home: PathBuf,
    /// One per agent this machine has, in table order: where its wiring goes
    /// and what is there now.
    pub wirings: Vec<VendorWiring>,
    /// This amx, and every amx the PATH finds in the order it looks — each
    /// a file, named once however many names it goes by.
    pub exe: PathBuf,
    pub on_path: Vec<PathBuf>,
    /// Where every agent's record is kept, and why amx cannot use it when it
    /// cannot.
    pub state_root: PathBuf,
    pub state_error: Option<String>,
    /// Handoffs still carrying the spawner's environment inline, from before
    /// it moved to a file of its own beside the handoff.
    pub dirty_handoffs: Vec<PathBuf>,
    /// The agents that never got past the vendor's own setup.
    pub parked: Vec<Parked>,
    /// The tmux server amx would put an agent on, when one is already running
    /// and this machine can say where it is standing.
    pub server: Option<StandingServer>,
    /// The vendor's own trust store, for a vendor whose screen amx answers by
    /// writing one, and the trees it still names that the disk has not got.
    pub store: Option<PathBuf>,
    pub stale: Vec<PathBuf>,
    /// The directory doctor was pointed at, when it was, and what the vendor
    /// would do for an agent started there.
    pub folder: Option<Folder>,
}

/// A directory an agent would be started in, and whether its vendor would
/// draw the folder-trust screen there. Read off the store and off git, and
/// written nowhere.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Folder {
    pub dir: PathBuf,
    /// The repository this is a linked worktree of, when it is one. The vendor
    /// resolves the tree to it before looking the folder up, and so does the
    /// answer amx writes at a spawn.
    pub repo: Option<PathBuf>,
    /// Whether the store already lets an agent in, by the directory's own
    /// entry or the repository's. `None` for a vendor that keeps no store amx
    /// reads.
    pub covered: Option<bool>,
    /// The config's `trust` key, which is what has amx answer for a linked
    /// worktree when the agent starts.
    pub trust: bool,
}

/// One agent's wiring, read off the disk.
///
/// The vendor is carried by name and by entry both: the name is what a check
/// about it says, and the entry is what says which files should be there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VendorWiring {
    pub vendor: &'static str,
    pub hooks: Option<&'static Hooks>,
    /// Where this agent's wiring goes, under the home.
    pub wire: PathBuf,
    pub wired: install::Wired,
    /// The wires a person opts into, each with where it would go and what is
    /// there now. Empty is the usual state: most machines never ask.
    pub opt_in: Vec<(PathBuf, install::Wired)>,
}

/// The server amx would use, and where its own process is standing.
///
/// The socket comes along because the remedy is a command line, and a restart
/// aimed at the wrong server is worse than no advice at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StandingServer {
    pub socket: tmux::Socket,
    pub cwd: tmux::ServerCwd,
}

/// An agent stopped at a screen the vendor puts in front of the work, and
/// which screen it is. Nobody but the person at the keyboard can get it past
/// one, so this is a check that names names rather than one amx can fix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parked {
    pub id: String,
    pub screen: Setup,
}

/// What is in the way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Setup {
    /// A screen this agent's own vendor draws in front of the work, under the
    /// name that vendor's document gives it, and whether it is one amx could
    /// have answered for the tree it cut: the folder-trust question, on a
    /// vendor whose answer amx knows how to write.
    Gate { screen: String, trust: bool },
    /// A screen no rule claims, under a record that has never left `starting`.
    Unread,
}

impl Setup {
    /// What is in the way, worded for the line that names the agent it stopped.
    ///
    /// A gate is named the way the vendor drawing it names it, because the
    /// screen is the vendor's and so is the word for it. What is left is the
    /// screen nobody has a rule for, which can only be described.
    fn says(&self) -> String {
        match self {
            Setup::Gate { screen, .. } => format!("its vendor's {screen} screen"),
            Setup::Unread => "an opening screen amx has no rule for".to_string(),
        }
    }
}

/// Judge what was found.
///
/// Nine of these are asked on every machine. The tenth is asked only where
/// there is something to ask it of: a tmux server already running, on a
/// platform that can say where a process is standing.
pub fn report(found: &Findings) -> Vec<Check> {
    let mut checks = vec![tmux_check(found), vendor_check(found), config_check(found)];
    checks.extend(found.wirings.iter().map(wiring_check));
    checks.extend(found.wirings.iter().flat_map(opt_in_checks));
    checks.extend([amx_check(found), state_check(found), env_check(found)]);
    checks.extend(server_check(found));
    checks.push(setup_check(found));
    checks.push(store_check(found));
    checks.extend(folder_check(found));
    checks
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

fn vendor_check(found: &Findings) -> Check {
    match &found.vendor_path {
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

/// Whether amx's hooks are where this vendor's reports would come from.
///
/// A vendor that reports nothing is not a machine with something missing from
/// it: there is nothing to write, nothing for anybody to repair, and what amx
/// has instead is the pane. A command amx has no entry for is measured neither
/// way and is judged as the first vendor is — a wrapper somebody wrote around
/// it loads the same files.
///
/// What is judged is whether the files the entry ships are the files that are
/// there. Repairing it is `amx setup`'s, not doctor's: the remedy names the
/// agent so that a person with two of them types the right line.
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
    };

    match &found.wired {
        // The vendor's own word for what amx wrote there: pi loads an
        // extension, claude loads a plugin, and a person sent to look at one
        // under the other's name is a person looking for the wrong thing.
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

/// The lines about the wires a person opted into.
///
/// A vendor with one is judged only where it already stands: an absent opt-in
/// file is a machine that never asked for the tool, which is not a fault and
/// not something to send anybody to fix. A stale one is: it is amx's file, an
/// older amx wrote it, and an upgrade of the reporting wire alone leaves the
/// tool calling a verb whose shape has moved.
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

/// The line that wires this check's agent.
fn setup_with(who: &str) -> String {
    format!("run `amx setup {who}`")
}

/// Whether the amx the PATH finds is this one, and the only one.
///
/// Two installed amx diverge quietly. A pi somebody started by hand reports
/// to whichever amx the PATH finds first, and `--fix` judges the wiring on
/// disk against what the amx running it ships: so the one on the PATH takes
/// the reports and passes its own wiring, the one that was rebuilt never
/// runs, and a doctor run under the first says the machine is fine. It was,
/// for an amx nobody meant to be using. Every amx on the PATH is named here
/// so that the one this is not becomes the fault it is.
fn amx_check(found: &Findings) -> Check {
    let exe = found.exe.display();
    let Some(first) = found.on_path.first() else {
        let dir = found.exe.parent().unwrap_or(&found.exe).display();
        return Check::wrong(
            "amx",
            format!("{exe} is not on the PATH"),
            format!("a pi started by hand reports to the amx the PATH finds; put {dir} on it"),
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

/// Whether the server amx would use is still standing somewhere that exists.
///
/// A tmux server keeps the directory it was started in for as long as it
/// lives. Delete that directory and the server carries on holding it: every
/// pane forked afterwards starts in a place that is not there, and the vendor
/// exits before it draws a frame. From the outside that looks like an agent
/// that failed in under a second having said nothing, which is a long way from
/// the cause.
///
/// `None` where there is nothing to ask — no server yet, or no way to look —
/// because a check nobody could act on is the kind that sends a person hunting
/// a fault in their own machine.
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

/// How a tmux command line names this socket.
fn address(socket: &tmux::Socket) -> String {
    match socket {
        tmux::Socket::Name(name) => format!("-L {name}"),
        tmux::Socket::Path(path) => format!("-S {}", path.display()),
    }
}

fn state_check(found: &Findings) -> Check {
    match &found.state_error {
        None => Check::ok("state", found.state_root.display().to_string()),
        Some(why) => Check::wrong(
            "state",
            why.clone(),
            "amx keeps every agent there, so until that is fixed it has nowhere to put one",
        ),
    }
}

/// Whether any handoff still carries the spawner's environment inline, from
/// before it moved to a file of its own beside the handoff.
///
/// [`spawn::Handoff`] dropped its `env` field, so a record an older amx wrote
/// still reads fine — serde drops the stray key rather than refusing it — but
/// the bytes on disk go on holding somebody's environment long after the pane
/// that needed it ever ran.
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
        // The one screen amx can take off the person's hands, once they have
        // said so: the config key is the consent the write stands behind. Only
        // offered where the gate is the folder-trust question and the vendor
        // standing at it is one amx answers that question for, because the key
        // does nothing for any other.
        Setup::Gate { trust: true, .. } => format!(
            "answer it yourself: amx attach {}, or set trust = true in the \
             config and amx answers it for any linked worktree",
            first.id
        ),
        _ => format!("answer it yourself: amx attach {}", first.id),
    };
    Check::wrong("gate", what, remedy)
}

/// Whether the vendor's own trust store still names trees amx cut and removed.
///
/// The vendor writes a project entry for every directory it is ever started
/// in, and amx cuts a tree per agent, so the file grows a key for each one and
/// keeps it long after the tree has gone. Nothing the person did put those
/// keys there, and nothing but amx knows which of them were its own.
///
/// A vendor that answers its folder-trust screen some other way keeps no store
/// amx has ever written in, and that is not a machine with something missing
/// from it.
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

/// Whether an agent started in the directory doctor was pointed at would meet
/// its vendor's folder-trust screen, when it was pointed at one.
///
/// Three ways to be fine: the store covers the directory already, the vendor
/// keeps no store amx reads, or the directory is a linked worktree and the
/// config's key has amx answer for it at the spawn. What is left is a screen
/// somebody has to answer by hand, and the remedy says where: the repository,
/// for a tree, because its entry covers every tree in it and the key does the
/// same; the directory itself for anything else, because a checkout and a
/// plain directory are the person's own to trust.
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

/// Print the checks, offer the one repair amx can make, and answer with an
/// exit code: zero when there is nothing left to do.
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

    Ok(if checks.iter().all(Check::is_ok) {
        exit::OK
    } else {
        exit::FAILURE
    })
}

/// The agents this machine is asked about, and what is wired for each.
///
/// Every entry in the table whose command the PATH finds, in table order, and
/// the configured one whether or not it is there. A vendor that is not
/// installed is not a machine with something missing from it, so it is not
/// mentioned at all; the configured one is always asked about because its
/// absence is a fault the `agent` check is already making, and because a
/// machine with no agent installed should still read a hooks line rather than
/// silently none.
///
/// The configured agent is resolved the way [`hooks_of`] resolves it: a
/// command amx has no entry for is judged as the first vendor is, since a
/// wrapper somebody wrote around claude loads the same files claude does.
fn wirings(agent: &str, home: &Path, path: Option<&OsStr>) -> Vec<VendorWiring> {
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
                wire: hooks
                    .map_or_else(|| home.to_path_buf(), |h| install::wire_path(&h.wire, home)),
                wired: hooks.map_or(install::Wired::Nothing, |h| install::wired(&h.wire, home)),
                opt_in: hooks.map_or_else(Vec::new, |h| {
                    h.opt_in
                        .iter()
                        .map(|wire| (install::wire_path(wire, home), install::wired(wire, home)))
                        .collect()
                }),
            }
        })
        .collect()
}

/// Look at the machine, and at `dir` when doctor was pointed at one.
pub fn gather(config: &Config, dir: Option<&Path>) -> Result<Findings> {
    let home = install::home()?;
    let exe = std::env::current_exe()?;
    let path = std::env::var_os("PATH");
    let wirings = wirings(&config.agent, &home, path.as_deref());
    let (_, config_warnings) = crate::config::load();
    let state_root = crate::paths::state_root()?;
    // Only for the vendor whose screen amx answers by writing its store: any
    // other keeps no file amx has ever left a key in. A store amx cannot read
    // names no tree it can be sure of either, and `new` is where that file is
    // refused by name.
    let store = trust::writes_a_store(&config.agent)
        .then(|| {
            // With the harness table's pairs laid over this environment, since
            // that is the environment its agents read the store in.
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
        vendor_path: on_path(program(&config.agent), std::env::var_os("PATH").as_deref()),
        config: crate::paths::config_file()?,
        config_warnings,
        home,
        wirings,
        on_path: every_on_path("amx", std::env::var_os("PATH").as_deref()),
        exe: exe.canonicalize().unwrap_or(exe),
        state_error: usable(&state_root),
        dirty_handoffs: dirty_handoffs(&state_root),
        parked: parked(
            // A state root amx cannot read has no agents to report on, and the
            // check above is where that is said. Here it means none were found.
            &derive::views(&state_root, store::now()).unwrap_or_default(),
        ),
        state_root,
        server: standing_server(),
        folder: dir.map(|dir| folder(dir, store.as_deref(), config.trust)),
        store,
        stale,
    })
}

/// What the vendor would do for an agent started in `dir`, read off the store
/// and off git.
///
/// A store amx cannot read covers nothing, as far as this can tell: `new`
/// refuses to write one by name, so the screen would be drawn and nobody
/// would answer it, which is the answer given.
fn folder(dir: &Path, store: Option<&Path>, trust: bool) -> Folder {
    let repo = worktree::is_linked(dir)
        .then(|| worktree::main_repo(dir).ok())
        .flatten();
    Folder {
        // Resolved, because the line and the remedy name it, and `--dir .` is
        // the usual way to ask.
        dir: std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf()),
        covered: store.map(|store| trust::covers(store, dir, repo.as_deref()).unwrap_or(false)),
        repo,
        trust,
    }
}

/// Every handoff under `root` that still carries the spawner's environment
/// inline. A root amx cannot read has none to report, the same as it has no
/// agents.
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

/// Whether the handoff at `path` still has an `env` key in it.
fn carries_env(path: &Path) -> bool {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .is_some_and(|doc| doc.get("env").is_some())
}

/// Rewrite each dirty handoff without its stray `env` key, at the mode
/// [`spawn::write_handoff`] promises. Answers how many actually needed it —
/// one gone before this got to it is not a fault, just skipped.
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

/// Take each stale tree's key back out of the store, and answer how many
/// there was anything to take out for — one the vendor rewrote away between
/// the reading and this is not a fault, just nothing to do.
fn forget_trees(store: &Path, stale: &[PathBuf], now: u64) -> Result<usize> {
    let mut forgotten = 0;
    for tree in stale {
        if trust::forget_tree(store, tree, now)? {
            forgotten += 1;
        }
    }
    Ok(forgotten)
}

/// The server amx would start an agent on, when one is already running.
///
/// The same resolution a spawn does, so doctor judges the server that would
/// actually be used rather than whichever one is easiest to find.
fn standing_server() -> Option<StandingServer> {
    let server = crate::spawn::server().ok()?;
    Some(StandingServer {
        socket: server.socket().clone(),
        cwd: server.cwd()?,
    })
}

/// The agents stopped at a screen the vendor draws before it will do anything
/// else, and which screen each of them is at.
///
/// Two shapes, because amx can name one of them and can only describe the
/// other. A screen its vendor's document marks as a gate has a rule measured
/// off a live vendor, so an agent stopped there is named for what it is — and
/// named out of that document, which is what lets one check speak for every
/// vendor's gates rather than for the first vendor's.
///
/// What is left is a screen no document has a rule for, and claude's login
/// prompt is why there is a second shape at all: it cannot honestly be given a
/// rule from here, because measuring it means logging a real claude out and a
/// string nobody read off a running vendor is exactly what the ruleset's anchor
/// law forbids.
///
/// So that one is described rather than named: a record that has never left
/// `starting` — the vendor has begun no turn, and `SessionStart` alone does not
/// move it — under a screen no rule claims. A vendor that changed its opening
/// screen reads the same way, and so does one still drawing its first frame
/// once the record has gone stale enough for the pane to be asked. That is the
/// cost of describing it, and it is the cheaper mistake: the remedy is to
/// attach and look, which is what a person would do anyway.
fn parked(views: &[View]) -> Vec<Parked> {
    views
        .iter()
        .filter_map(|view| {
            let screen = if let Some(gate) = gate(view) {
                Setup::Gate {
                    screen: gate.name.clone(),
                    // The document says the screen is the folder-trust
                    // question; the table says whether amx can answer that
                    // question for the vendor drawing it. Both, or the offer
                    // below is a config key that changes nothing.
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

/// The gate this agent is standing at, when the screen its reader claimed is
/// one its own vendor's document marks as one.
///
/// The verdict has to say `waiting` as well: a rule is found again by the name
/// on it, and a reader that concluded anything else has said the agent is past
/// this screen or never reached it.
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

/// What runs this agent, which is what finds both the document its screen was
/// read against and the entry amx would answer a folder-trust screen from. A
/// record naming no command falls back the way every other reader falls back —
/// see [`crate::rules::of`].
fn runs(view: &View) -> &str {
    view.meta.agent.as_deref().unwrap_or_default()
}

/// Why amx cannot use `root`, when it cannot.
///
/// An agent's directory is made with all of its missing parents at once, so
/// the directory that has to take that write is the nearest ancestor already
/// on disk: the root itself once amx has run here before, the directory above
/// it on a machine where it has not. The root is read as well as written once
/// it exists, because listing it is how every reader finds the agents.
///
/// The failure this exists for is quiet: a root that cannot be made and a
/// machine that has simply never run an agent both list as no agents at all,
/// and the difference only shows up as a spawn failing later.
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

/// Run the verb against the machine, and against `dir` when there is one.
pub fn from_env(config: &Config, fix: bool, dir: Option<&Path>) -> Result<i32> {
    // A directory that is not there is a different fault from a screen, and
    // an agent could not be started in it whatever the store says.
    if let Some(dir) = dir
        && !dir.is_dir()
    {
        anyhow::bail!(
            "{} is not a directory an agent could start in",
            dir.display()
        );
    }
    let found = gather(config, dir)?;
    let mut out = std::io::stdout().lock();
    run(&found, fix, crate::store::now(), &mut out)
}

/// The program a configured command runs, without its arguments.
fn program(command: &str) -> &str {
    command.split_whitespace().next().unwrap_or(command)
}

/// Where a program resolves to, given a `PATH`.
fn on_path(program: &str, path: Option<&OsStr>) -> Option<PathBuf> {
    // A command with a separator in it is a path, and a shell would not search
    // for it either.
    if program.contains('/') {
        let named = PathBuf::from(program);
        return runnable(&named).then_some(named);
    }

    let path = path?;
    std::env::split_paths(path)
        .map(|dir| dir.join(program))
        .find(|candidate| runnable(candidate))
}

/// Every program called `program` a `PATH` would run, in the order it looks,
/// each file once: a file reached by two names is one install, and one
/// install answering to two names is how the fault above is mended.
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

/// Whether this is a file that could be run.
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

    /// The screens a vendor's document marks as gates in front of the work,
    /// which is the list this check reads instead of holding one of its own.
    fn gates(agent: &str) -> Vec<&'static Rule> {
        crate::rules::of(agent)
            .rules()
            .iter()
            .filter(|rule| rule.setup)
            .collect()
    }

    /// One screen out of a vendor's document: what it means, and whether it is
    /// one of that vendor's gates.
    ///
    /// No screen is spelled out in this file, for the reason none is spelled
    /// out in the code it tests: the names are the vendors' own, and a test
    /// holding a copy of one is the same list in a second place.
    fn screen(agent: &str, means: Phase, gate: bool) -> &'static str {
        crate::rules::of(agent)
            .rules()
            .iter()
            .find(|rule| rule.state == means && rule.setup == gate)
            .map(|rule| rule.name.as_str())
            .unwrap_or_else(|| panic!("{agent:?} draws no such screen"))
    }

    /// One agent's wiring, whole or missing, where its own entry puts it.
    fn wiring(vendor: &'static str, there: bool) -> VendorWiring {
        let hooks = registry::entry(vendor).and_then(|v| v.hooks.as_ref());
        VendorWiring {
            vendor,
            hooks,
            wire: hooks.map_or_else(
                || PathBuf::from("/home/dev"),
                |h| install::wire_path(&h.wire, Path::new("/home/dev")),
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
                            install::wire_path(wire, Path::new("/home/dev")),
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
        }
    }

    /// An agent as a reader hands it over. The record is deserialised rather
    /// than built field by field, because that is how a real one arrives and a
    /// field added to `Meta` tomorrow should not land here.
    ///
    /// `agent` is the command in the pane, which is what decides whose screens
    /// this record is read against. `None` is a record that names none — a
    /// shell command, or one an older amx wrote — and reads the vendor every
    /// reader falls back to.
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

    /// A directory nobody but its owner may write to is the whole of these
    /// tests, and root is exempt from the permission bits.
    fn not_root() -> bool {
        if nix::unistd::Uid::effective().is_root() {
            eprintln!("skipping: running as root, which every directory lets in");
            return false;
        }
        true
    }

    /// git as the tests run it: none of the developer's own configuration,
    /// and an identity of its own.
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
            9,
            "tmux, the vendor, the config, the hooks, amx, the state root, env, setup, the store"
        );

        let (code, printed) = said(&healthy(), false);
        assert_eq!(code, exit::OK);
        assert!(printed.contains("tmux"), "{printed}");
    }

    /// A server standing in `path`, deleted or not.
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
        // No server running, or a platform with no way to look: either way
        // there is nothing here to report and nothing to repair.
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
        // The failure this check exists for: doctor was green while every
        // agent died in under a second, because nothing asked this.
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
        // A remedy that said `-L default` for a server reached by path would
        // send somebody to restart the wrong one.
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
        // Hooks are a vendor's own doing, and one that has none is not a
        // machine with something missing from it: there is nothing to wire and
        // nothing to fix, and what amx has instead is the pane.
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

        // A vendor that does report is judged, and told which line wires it.
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
    fn a_wrapper_somebody_wrote_is_judged_as_the_vendor_underneath_it_is() {
        // A command amx has no entry for loads the files the first vendor
        // loads, so the machine is asked about that vendor rather than about
        // nothing at all.
        let asked = wirings("my-claude", Path::new("/home/dev"), None);
        assert_eq!(
            asked.iter().map(|w| w.vendor).collect::<Vec<_>>(),
            ["claude"],
            "and only that one, since no other agent is on this PATH"
        );
    }

    #[test]
    fn an_agent_this_machine_has_not_got_is_not_asked_about() {
        // Nothing is missing from a machine that never installed pi, so
        // doctor says nothing about pi at all. The configured agent is the
        // exception: its absence is the `agent` check's to report, and a
        // hooks line about it is what names the line that wires it.
        let dir = TempDir::new().unwrap();
        let pi = dir.path().join("pi");
        std::fs::write(&pi, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&pi, std::fs::Permissions::from_mode(0o755)).unwrap();

        let home = Path::new("/home/dev");
        assert_eq!(
            wirings("claude", home, None)
                .iter()
                .map(|w| w.vendor)
                .collect::<Vec<_>>(),
            ["claude"]
        );
        assert_eq!(
            wirings("claude", home, Some(dir.path().as_os_str()))
                .iter()
                .map(|w| w.vendor)
                .collect::<Vec<_>>(),
            ["claude", "pi"],
            "pi is installed here, so it is asked about too"
        );
    }

    /// pi, wired the way its hooks say it will be: the entry in the table
    /// does not carry them yet, so the check is asked about a vendor built
    /// here that does.
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
        // The verb, naming this check's own agent, so that a person with two
        // of them types the right line.
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

        // A command that is a path is not looked for on the PATH at all.
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
        // The machine this check exists for: two installs, and `amx doctor
        // --fix` run under the stale one judged the stale extension against
        // its own body, said ok, and the build carrying the fix never ran.
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

        // Run under the fresh one instead, the PATH still reaches the stale
        // one first, and that is the one a pi started by hand reports to.
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

        // Started by its path, with nothing on the PATH by that name: a pi
        // somebody started by hand has no amx to report to.
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
        // The same file under a second name, which is how one install is
        // meant to answer both.
        std::os::unix::fs::symlink(&real, second.path().join("amx")).unwrap();
        // A file that happens to be called amx and that no shell would run.
        program(&third, 0o644);
        // And another program of the name, which is the fault.
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
        // A state root nobody can write to and a machine that has simply never
        // run an agent look the same from a listing: both say "no agents".
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

        // The root is two levels below a directory that will not take it, so
        // the name in the answer is the directory that actually refused.
        let why = usable(&closed.join("amx/agents")).expect("nowhere to make it");
        assert!(why.contains(&closed.display().to_string()), "{why}");
    }

    /// An agent record, with a handoff written the way `spawn` writes one —
    /// only `meta.json` matters to `store::list`, so it need not parse.
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

    /// Somebody's store, with the repository they work in, a key of their own,
    /// and one key per tree amx cut in there — the shape a store that nobody
    /// has ever pruned arrives in.
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
        // The store grows a key for every directory the vendor is started in,
        // and amx cuts a tree per agent: a store nobody prunes carries one key
        // per agent that ever ran, long after the tree it names has gone.
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
        // pi's answer to the same screen is a flag on the argv, spent the
        // moment the run ends: there is no file of the vendor's for amx to
        // have left keys in, so there is nothing here to prune.
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

    /// A directory doctor was asked about, judged.
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
        // Asked only with --dir, so that a caller about to start a reader it
        // cannot see finds out here instead of losing it to a screen nobody
        // can attach to in time. Every answer is on the exit code.
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
        // The folder-trust question, read as the vendor's document marks it
        // rather than as a name this file knows.
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
        // The gates are the vendor's own, and pi's are screens claude never
        // draws: a check that knew one vendor's rule by name said nothing at
        // all about an agent that never got past any of these.
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

            // The offer is the folder-trust question's alone. On a gate that
            // asks anything else the config key answers nothing, whoever drew
            // the screen.
            if gate.kind != Some(Kind::Trust) {
                assert!(!remedy.contains("trust = true"), "{remedy}");
            }
        }
    }

    #[test]
    fn the_offer_to_answer_a_gate_is_made_for_the_vendors_amx_answers_it_for() {
        // The config key stands behind a write into one vendor's own store,
        // and amx makes that write only for a command it has an entry for.
        // These two are read against that vendor's screens — a wrapper is that
        // program underneath, and a record naming nothing falls back to it —
        // but the key would answer for neither, so the screen is still named
        // and the question is left to whoever is at the keyboard.
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
        // What a login prompt looks like from out here, and amx says only what
        // it can see: a screen no rule claims, from an agent that has never
        // reported anything.
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
        // Every screen here is read out of the document of the vendor the
        // record names, and most of these name none — what an older amx wrote,
        // and what a shell command still writes — so they are read against the
        // vendor every reader falls back to.
        let working = screen("", Phase::Working, false);
        let idle = screen("", Phase::Idle, false);
        let asking = screen("", Phase::Waiting, false);
        let gate = screen("", Phase::Waiting, true);
        // A gate of pi's that asks something other than whether to trust the
        // folder, so that what it is offered turns on the screen and not on
        // which vendor is standing at it.
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
            // Interrupted mid-turn onto a screen no rule claims. The vendor let
            // this one start, so it is not stopped at a gate.
            view(None, "lost-f6g", Phase::Working, Phase::Unknown, None),
            // Started, drawn, and sitting at its prompt with nothing to do.
            view(None, "fresh-g7h", Phase::Starting, Phase::Idle, Some(idle)),
            // Another vendor's gate, on a record that says so. Read against
            // claude's screens this is a name no rule has, and the agent is
            // one nobody would have been told about.
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
        // Write and search but no read: a listing of it fails outright, which
        // is the other way a full state root passes for an empty one.
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o300)).unwrap();

        let why = usable(&root);
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(why.is_some(), "a root amx cannot list is a root to report");
    }
}
