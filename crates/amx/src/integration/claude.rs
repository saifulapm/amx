//! Claude Code's side of the integration.
//!
//! # Where the file is
//!
//! `$CLAUDE_CONFIG_DIR/settings.json`, or `~/.claude/settings.json` when that
//! variable is unset. Read from the shipped 2.1.224 binary, which resolves its
//! configuration directory as `process.env.CLAUDE_CONFIG_DIR ||
//! join(homedir(), ".claude")` and names `~/.claude/settings.json` as the
//! user-scope settings file throughout its own help text.
//!
//! **User scope, not project scope.** Claude Code reads a cascade —
//! `~/.claude/settings.json`, then the project's `.claude/settings.json` — and
//! the spike used the project file because a scratch project was the subject.
//! An installer must use the user file: a pane's agent is started wherever the
//! user happens to be, an amx session outlives any one repository, and a
//! per-project install would have to be repeated for every checkout while
//! leaving amx's hooks committed into other people's repositories.
//!
//! # What V01 measured about running them
//!
//! Freshly written project hooks ran with no approval step of their own
//! (§3 M7): the settings file was written seconds before launch and
//! `SessionStart` arrived 0.57 s after the folder-trust dialog was answered. So
//! `amx integration install` is genuinely non-interactive.
//!
//! But the *folder*-trust dialog is a hard gate, and it is worth being precise
//! about, because it is the one thing that can make a correct installation look
//! broken: in a directory Claude Code had never seen, **no hook fired at all**
//! for the eight seconds the dialog was up, and the dialog does not mention
//! hooks — it enumerates pre-approved tool permissions. The honest wording is
//! the note below: installed hooks run in any folder already trusted; in a new
//! one, the next interactive launch asks once.
//!
//! # Matchers
//!
//! Six of Claude Code's events are scoped to a tool and their entries take a
//! `matcher`; the rest take none. The set here is the one V01's measured
//! configuration used (`scripts/spike/lib/scratch.py`, `TOOL_SCOPED`) — the
//! configuration every event in §2's matrix was recorded firing from — so amx
//! installs the shape that was measured working rather than one derived from
//! prose.

use std::path::PathBuf;

use amx_core::Env;
use amx_server::agent::registry::AgentStanza;
use anyhow::Context as _;

use super::hooks::Subscription;
use super::{Plan, Trust, var};

/// The agent's own override for where its configuration lives.
const CONFIG_DIR: &str = "CLAUDE_CONFIG_DIR";

/// The configuration directory under `$HOME` when it does not.
const HOME_DIR: &str = ".claude";

/// The user-scope settings file inside it.
const SETTINGS: &str = "settings.json";

/// The events whose entries carry a tool-name matcher.
const TOOL_SCOPED: &[&str] = &[
    "PreToolUse",
    "PostToolUse",
    "PostToolUseFailure",
    "PostToolBatch",
    "PermissionRequest",
    "PermissionDenied",
];

/// The matcher amx writes for them: every tool.
///
/// amx forwards, it does not filter (D-M2-4) — which tools matter is the fusion
/// machine's judgement, made in a binary that ships, not in a config file that
/// would need reinstalling on every machine to change.
const EVERY_TOOL: &str = "*";

/// What amx will do to Claude Code's configuration.
pub fn plan(env: &Env, stanza: &AgentStanza) -> anyhow::Result<Plan> {
    Ok(Plan {
        agent: stanza.id.clone(),
        label: stanza.label.clone(),
        path: settings_path(env)?,
        subs: stanza
            .hook_events
            .iter()
            .map(|event| Subscription {
                matcher: TOOL_SCOPED
                    .contains(&event.as_str())
                    .then(|| EVERY_TOOL.to_owned()),
                event: event.clone(),
            })
            .collect(),
        notes: vec![
            "hooks run in every folder you have already trusted; in a folder Claude Code \
             has not seen before, no hook fires until you answer its trust prompt once.",
        ],
        trust: Trust::Implicit,
    })
}

/// `$CLAUDE_CONFIG_DIR/settings.json`, or `~/.claude/settings.json`.
fn settings_path(env: &Env) -> anyhow::Result<PathBuf> {
    if let Some(dir) = var(CONFIG_DIR) {
        return Ok(PathBuf::from(dir).join(SETTINGS));
    }
    let home = env
        .home
        .clone()
        .context("$HOME is not set, and Claude Code keeps its settings under it")?;
    Ok(home.join(HOME_DIR).join(SETTINGS))
}
