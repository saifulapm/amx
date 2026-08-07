//! Codex CLI's side of the integration, and the caveat it has to carry.
//!
//! # Where the file is
//!
//! `$CODEX_HOME/hooks.json`, or `~/.codex/hooks.json` when that variable is
//! unset. V01 §4 measured the shape by writing one and watching Codex 0.147.0
//! offer to review exactly the eleven hooks it held
//! (`scripts/spike/lib/scratch.py`, `build_codex_home`): a JSON object with a
//! `hooks` key, PascalCase event names inside it, and the same
//! `{matcher?, hooks: [{type, command}]}` entries Claude Code's settings block
//! takes. Entries are written with no matcher, which is the configuration every
//! Codex event in §4's matrix was recorded firing from.
//!
//! # Three things the plan said about Codex that the measurement did not
//!
//! 08 §5's V10 entry asks for "the `[features] codex_hooks = true` line-edit
//! (the flag V01 confirmed — not `hooks`, R-M2-2)". V01 confirmed the opposite,
//! and this module writes no feature flag at all:
//!
//! - `codex features list` on 0.147.0 prints `hooks  stable  true`. Hooks are
//!   **on by default and there is no flag to set**.
//! - There is no `codex_hooks` feature. The name exists in the shipped binary
//!   only as the Rust crate path `codex_hooks::engine::command_runner`, which
//!   is the likeliest source of the third-party claim the plan quoted. R-M2-2
//!   should be withdrawn; 04 §5's `[features] hooks` spelling was the closer
//!   one, and neither needs writing.
//! - Writing `[features] codex_hooks = true` into a user's `config.toml` would
//!   therefore add a key Codex does not read, which is the kind of debris an
//!   installer is judged by years later. amx writes one file, `hooks.json`, and
//!   `config.toml` is never opened.
//!
//! # The trust gate, which is the honest part
//!
//! Hooks are **inert until a human approves them interactively**, and the
//! approval is keyed to the file's content hash (the binary's own config struct
//! is `HookStateToml { enabled, trusted_hash }`). V01 measured what that means
//! in practice, and every clause is a measurement rather than a reading:
//!
//! - A fresh `hooks.json` raises a blocking startup prompt — *"Hooks need
//!   review · 11 hooks are new or changed"* — before the session begins.
//! - Declining is silent and total: a full turn after "Continue without
//!   trusting" produced **zero** hook invocations.
//! - `codex exec` with an untrusted `hooks.json` fired nothing **and warned
//!   nothing**; an authenticated turn ran to completion with the hooks inert.
//! - Re-writing the file re-arms the prompt, because the hash changed.
//! - No user-readable trust record was found. The spike looked; where
//!   `trusted_hash` is persisted is V01 §9's leftover L8.
//!
//! An installer that reported success here without saying any of that would be
//! herdr's silent no-op in a new costume — the exact failure D-M2-4 exists to
//! delete. So [`plan`] carries the caveat into `install`'s output, and
//! [`Trust::Unreadable`] makes `status` say it cannot see whether the hooks are
//! live rather than implying they are.

use std::path::PathBuf;

use amx_core::Env;
use amx_server::agent::registry::AgentStanza;
use anyhow::Context as _;

use super::hooks::Subscription;
use super::{Plan, Trust, var};

/// The agent's own override for where its configuration lives.
const CODEX_HOME: &str = "CODEX_HOME";

/// The configuration directory under `$HOME` when it does not.
const HOME_DIR: &str = ".codex";

/// The hooks file inside it — not `config.toml`, which amx never opens.
const HOOKS_FILE: &str = "hooks.json";

/// What amx will do to Codex's configuration.
pub fn plan(env: &Env, stanza: &AgentStanza) -> anyhow::Result<Plan> {
    Ok(Plan {
        agent: stanza.id.clone(),
        label: stanza.label.clone(),
        path: hooks_path(env)?,
        // No matchers: the configuration V01 measured firing wrote none, for
        // any of the eleven events Codex lists.
        subs: stanza
            .hook_events
            .iter()
            .map(|event| Subscription {
                event: event.clone(),
                matcher: None,
            })
            .collect(),
        notes: vec![
            "Codex will not run these hooks until you approve them: your next `codex` run \
             opens a \"Hooks need review\" prompt, and answering it is the only way in — \
             `codex exec` runs with the hooks inert and says nothing.",
            "the approval is pinned to this file's contents, so installing again, or \
             uninstalling, asks again.",
        ],
        trust: Trust::Unreadable,
    })
}

/// `$CODEX_HOME/hooks.json`, or `~/.codex/hooks.json`.
fn hooks_path(env: &Env) -> anyhow::Result<PathBuf> {
    if let Some(dir) = var(CODEX_HOME) {
        return Ok(PathBuf::from(dir).join(HOOKS_FILE));
    }
    let home = env
        .home
        .clone()
        .context("$HOME is not set, and Codex keeps its configuration under it")?;
    Ok(home.join(HOME_DIR).join(HOOKS_FILE))
}
