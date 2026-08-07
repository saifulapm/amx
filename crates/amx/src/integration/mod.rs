//! The agent hook integrations: what amx writes, where, and how it checks.
//!
//! 04 §8: "Integrations have a lifecycle, not just an install: `amx integration
//! install|uninstall|status` with version markers in installed hook assets, and
//! `status` runs after self-update — a stale `amx _hook` is worse for amx than
//! for herdr, since hooks feed the fusion tier."
//!
//! # What gets installed
//!
//! One entry per event, running `amx _hook <agent> --marker <N>` — the emitter
//! of D-M2-4, which is the amx binary itself rather than a shell script that
//! shells out to an interpreter. herdr's assets are `sh` plus a `python3`
//! heredoc; when `python3` is missing they no-op in silence and
//! `integration status` still reports `current`, because it only greps a
//! version comment out of the installed file. Two things follow for amx, and
//! both are the point of this module:
//!
//! - the installed asset is a command line, not a program, so there is no
//!   interpreter to be missing; and
//! - [`Plan::status`] verifies the referenced binary **runs and is an amx**,
//!   not merely that a marker is present. A marker says which shape was
//!   written. It cannot say whether the thing it names still exists.
//!
//! Which events an agent subscribes is registry data — `hook_events` in its
//! `agents.toml` stanza, measured in V01 §5 — never a list here. An agent's
//! name appearing anywhere but its stanza is W6 growing back
//! (`docs/02-herdr-critique.md`); the one thing this module does key on an id
//! is [`plan`]'s choice of *which config file format* to edit, which is program
//! structure rather than data.
//!
//! # The two agents differ in the one way that matters
//!
//! Both take the same JSON block ([`hooks`]), so the editing is shared. What is
//! not shared is whether installing is enough to make the hooks run: Claude
//! Code runs them (subject to its own folder trust), and Codex does not until a
//! human approves them interactively, per content hash, with no record amx can
//! read. [`Trust`] carries that difference into what `status` is allowed to
//! claim, and it is why this verb prints notes at all.
//!
//! # Task ownership
//!
//! **V10** owns this directory and `crates/amx/src/cmd/integration.rs`. The
//! `status`-after-self-update wiring is M3's: [`Plan::status`] is callable and
//! nothing here calls it on a schedule.

pub mod claude;
pub mod codex;
pub mod command;
pub mod edit;
pub mod hooks;

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use amx_core::Env;
use amx_core::agent::AgentKind;
use amx_server::agent::registry::{AgentStanza, Registry};
use anyhow::{Context as _, bail};

use edit::Json;
use hooks::Subscription;

/// The version of the entries this build writes.
///
/// It rides in the installed command line as `--marker <N>` and is inert at
/// run time — `amx _hook` accepts the flag and ignores it. Its only reader is
/// [`Plan::status`], and its only job is to make "installed by an older amx"
/// a state you can see. Bump it when the shape of an installed entry changes:
/// a different command line, a different set of events per stanza is already
/// covered by the comparison, but a changed *convention* is not.
pub const MARKER: u32 = 1;

/// How long the referenced binary gets to identify itself.
///
/// A bound rather than a plain wait because `status` runs a program named by a
/// file amx does not own. Generous: this is a local exec of a static binary
/// printing one line.
const PROBE_BUDGET: Duration = Duration::from_secs(5);

/// What `<program> --version` must start with to be an amx.
const VERSION_PREFIX: &str = "amx ";

/// Whether an installed integration is enough to make hooks run.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Trust {
    /// The agent runs installed hooks without approving them, so "installed"
    /// means "will run". Claude Code, measured in V01 §3 M7 — subject to its
    /// own folder-trust dialog, which [`Plan::notes`] says out loud.
    Implicit,
    /// The agent gates hooks behind an interactive approval whose state amx
    /// cannot read. Codex, measured in V01 §4: declining is silent and total,
    /// `codex exec` fires nothing and warns nothing, and no user-readable trust
    /// record was found. `status` must not imply the hooks are live.
    Unreadable,
}

/// What amx will write into one agent's configuration.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Plan {
    /// The registry id of the agent.
    pub agent: AgentKind,
    /// Its human label, from the same stanza.
    pub label: String,
    /// The configuration file amx edits, and the only file it touches.
    pub path: PathBuf,
    /// One subscription per event in the stanza's measured `hook_events`.
    pub subs: Vec<Subscription>,
    /// What the user has to be told when this is installed, in their words.
    pub notes: Vec<&'static str>,
    /// Whether installing is enough to make the hooks run.
    pub trust: Trust,
}

/// What an install or uninstall did.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Outcome {
    /// The file already said exactly this; nothing was written.
    ///
    /// Reinstalling over a current install lands here, which is the promise
    /// that makes `amx integration install` safe to run from a script.
    Unchanged,
    /// Entries were written.
    Installed {
        /// How many.
        entries: usize,
    },
    /// Entries were removed.
    Removed {
        /// How many.
        entries: usize,
    },
}

/// What `status` found.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum State {
    /// No amx entries in the agent's configuration.
    Absent,
    /// Installed, current, and the binary it names is an amx that runs.
    Current,
    /// Installed by a different amx, or subscribing a different event set.
    Outdated,
    /// Installed, but the binary the entries name cannot run.
    ///
    /// The state herdr cannot report and amx exists to report: hooks that are
    /// configured, look installed, and do nothing.
    Broken,
}

/// One agent's answer to `amx integration status`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Status {
    /// Which of the four it is.
    pub state: State,
    /// One line of evidence for that verdict, written for a human.
    pub detail: String,
}

impl Plan {
    /// Write the entries, or report that they were already there.
    ///
    /// The file is only written when its *parsed content* changes, so a
    /// reinstall neither reformats the user's settings nor — for Codex — re-arms
    /// a trust prompt that was already answered.
    pub fn install(&self, program: &Path) -> anyhow::Result<Outcome> {
        let mut document = edit::load(&self.path)?.unwrap_or_else(Json::object);
        let before = document.clone();
        let command = command::build(program, &self.agent, MARKER);
        hooks::apply(&mut document, &self.agent, &command, &self.subs)
            .with_context(|| format!("edit {}", self.path.display()))?;
        if document == before {
            return Ok(Outcome::Unchanged);
        }
        edit::store(&self.path, &document)?;
        Ok(Outcome::Installed {
            entries: self.subs.len(),
        })
    }

    /// Remove amx's entries, and nothing else.
    pub fn uninstall(&self) -> anyhow::Result<Outcome> {
        let Some(mut document) = edit::load(&self.path)? else {
            return Ok(Outcome::Unchanged);
        };
        let entries = hooks::strip(&mut document, &self.agent)
            .with_context(|| format!("edit {}", self.path.display()))?;
        if entries == 0 {
            return Ok(Outcome::Unchanged);
        }
        edit::store(&self.path, &document)?;
        Ok(Outcome::Removed { entries })
    }

    /// Report whether the installed integration is the one this build writes,
    /// and whether it can run at all.
    ///
    /// The order of the checks is the lesson: the binary is verified **before**
    /// the marker is believed. An entry whose program has been deleted or
    /// replaced is [`State::Broken`] no matter how current its marker looks,
    /// because a hook that cannot exec is a hook that reports nothing while
    /// every surface says the agent is integrated.
    pub async fn status(&self, program: &Path) -> anyhow::Result<Status> {
        let Some(document) = edit::load(&self.path)? else {
            return Ok(Status {
                state: State::Absent,
                detail: format!("no {}", self.path.display()),
            });
        };
        let installed = hooks::installed(&document, &self.agent);
        let Some(first) = installed.first() else {
            return Ok(Status {
                state: State::Absent,
                detail: "no amx entries".to_owned(),
            });
        };

        if let Err(reason) = runs(&first.program).await {
            return Ok(Status {
                state: State::Broken,
                detail: format!("{}: {reason}", first.program.display()),
            });
        }

        // "Current" is defined as "installing again would change nothing" —
        // one comparison that covers the marker, the event list, the matchers
        // and the path of the binary at once, and that cannot drift from what
        // `install` actually writes.
        let mut desired = document.clone();
        let command = command::build(program, &self.agent, MARKER);
        hooks::apply(&mut desired, &self.agent, &command, &self.subs)
            .with_context(|| format!("read {}", self.path.display()))?;
        if desired == document {
            return Ok(Status {
                state: State::Current,
                detail: format!(
                    "marker {MARKER}, {} events, {}",
                    installed.len(),
                    first.program.display()
                ),
            });
        }
        Ok(Status {
            state: State::Outdated,
            detail: format!(
                "marker {}, {} events; `amx integration install` updates it",
                first
                    .marker
                    .map_or_else(|| "none".to_owned(), |marker| marker.to_string()),
                installed.len()
            ),
        })
    }
}

/// Whether `program` is an amx that runs.
///
/// Executing it is the check that means something. A path test would pass for a
/// dangling symlink's target, for a file without its executable bit, and for a
/// binary built for another architecture — every one of which is a hook that
/// fires and does nothing, which is the failure this verb exists to surface.
async fn runs(program: &Path) -> Result<(), String> {
    let spawned = tokio::process::Command::new(program)
        .arg("--version")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output();
    let output = match tokio::time::timeout(PROBE_BUDGET, spawned).await {
        Err(_) => return Err("did not answer `--version`".to_owned()),
        Ok(Err(err)) => return Err(err.to_string()),
        Ok(Ok(output)) => output,
    };
    if !output.status.success() {
        return Err("`--version` failed".to_owned());
    }
    if !String::from_utf8_lossy(&output.stdout).starts_with(VERSION_PREFIX) {
        return Err("is not an amx".to_owned());
    }
    Ok(())
}

/// The amx binary an installed hook should run.
///
/// The running executable, resolved: an installed entry has to name a path,
/// and the only path amx can be sure of is its own. After a self-update moves
/// or replaces that binary the entries go stale, which is exactly what
/// [`Plan::status`] is for and what 04 §8 wires it to run after (in M3).
pub fn program() -> anyhow::Result<PathBuf> {
    std::env::current_exe().context("find this amx binary")
}

/// Every agent amx has an integration for, or the one `only` names.
///
/// The default set is registry-driven twice over: a stanza with no measured
/// `hook_events` has nothing to subscribe and is skipped, and a stanza amx has
/// no config-file format for is skipped too — a user override may add an agent
/// long before amx knows how to install for it. Naming such an agent explicitly
/// is an error rather than a silent no-op, because a user who typed it wants an
/// answer about it.
pub fn plans(env: &Env, registry: &Registry, only: Option<&str>) -> anyhow::Result<Vec<Plan>> {
    let Some(name) = only else {
        return registry
            .stanzas()
            .iter()
            .filter(|stanza| !stanza.hook_events.is_empty())
            .filter_map(|stanza| plan(env, stanza))
            .collect();
    };

    let kind = AgentKind::new(name).with_context(|| format!("`{name}` is not an agent id"))?;
    let stanza = registry
        .resolve(&kind)
        .with_context(|| format!("no agent `{name}` in the registry"))?;
    match plan(env, stanza) {
        Some(plan) => Ok(vec![plan?]),
        None => bail!(
            "amx has no hook integration for `{}`; its stanza names no events, \
             or amx does not know its configuration format",
            stanza.id
        ),
    }
}

/// The plan for one stanza, or `None` when amx has no installer for it.
fn plan(env: &Env, stanza: &AgentStanza) -> Option<anyhow::Result<Plan>> {
    match stanza.id.as_str() {
        "claude" => Some(claude::plan(env, stanza)),
        "codex" => Some(codex::plan(env, stanza)),
        _ => None,
    }
}

/// One environment variable, treating unset and empty alike.
///
/// `amx`'s rule is that the environment is read once, in `lib::run`, and passed
/// down as an [`Env`] (04 §2, fixes W9). These reads are outside it for the
/// reason `cmd/hook.rs` states for its own: the variables are not amx's.
/// `CLAUDE_CONFIG_DIR` and `CODEX_HOME` belong to the agents, they select the
/// file this verb edits, and carrying them in `Env` would put two other
/// programs' configuration vocabulary into the type every amx path takes. The
/// one variable that *is* amx's — `$HOME`, which both agents fall back to —
/// comes from [`Env::home`], which is why the acceptance suite can point a whole
/// install at a temporary directory.
fn var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}
