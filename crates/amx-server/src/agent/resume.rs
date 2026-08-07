//! Resume: a captured ref back into a running conversation.
//!
//! 04 §5 keeps herdr's rigor wholesale (K5) — "refs validated, argv as data,
//! allowlisted sources, dedupe reservations with rollback — but the tables come
//! from the registry". D-M2-7 is the whole of it, and the parts land in three
//! places: the ref types are in
//! [`amx_core::agent::refs`](amx_core::agent::refs) (shape validation, on
//! construction *and* on deserialization), the templates and the allowed ref
//! kinds are stanza data in [`registry`](super::registry), and the planning,
//! reservation and injection are here.
//!
//! What this module owes, per D-M2-7:
//!
//! - **Source allowlisting, at all three gates.** A ref is only accepted from
//!   the hook path of the agent it claims — `source == "amx:<agent-id>"`,
//!   cross-checked against the stanza — and the check runs at report time, at
//!   snapshot-read time, and again at plan time. Three, because a `session.json`
//!   is user-editable and V15's acceptance test hand-edits one.
//! - **argv is data.** [`plan`] substitutes the ref into the stanza template's
//!   single `{ref}` slot and returns a `Vec<String>`. Nothing is interpolated
//!   into a shell string, ever; quoting happens only at the injection boundary,
//!   in [`command_line`].
//! - **Dedupe reservations with rollback.** Restore reserves
//!   `source\0agent\0kind\0value` before spawning; a second pane claiming the
//!   same conversation restores as a plain shell; a failed spawn releases the
//!   reservation so a later pane can claim it. [`Reservations`] is that table.
//! - **Launch is type-in, not exec.** Restore spawns the pane's saved shell,
//!   waits (bounded by a condition, never a sleep) for the shell's first
//!   damage — the prompt painting *is* the readiness signal — then injects the
//!   planned argv through the same submit path `pane.run` uses. The pane stays
//!   a shell, so it survives the agent's eventual exit, and the user sees what
//!   was run in their own history. [`type_in`] is that wait and that write.
//!
//! One thing the plan did not know and V01 §3 M8 measured: **the ref must be
//! taken from every `SessionStart`, not just the first.** `/clear` mints a new
//! conversation inside one process, `SessionStart.source` is the only warning,
//! and a ref captured before it resumes the wrong conversation. Nothing here
//! has to do anything about that — the fusion machine takes the ref off every
//! report and `Core`'s mirror carries the latest one into the capture — but it
//! is why the ref this module plans from is never assumed to be the first.
//!
//! Every degraded outcome goes through the restore report. A conversation that
//! did not resume is a loss the user is told about, never a log line (04 §6),
//! which is why every refusal below is a typed value with a sentence a user can
//! read rather than a `None`.
//!
//! # What runs where
//!
//! [`super`]'s rule still holds: nothing here reads a file or owns a task.
//! [`plan`] is a pure function of a registry and a snapshot, and [`type_in`] is
//! a future over handles its caller already owns — `core/restore.rs` reads the
//! registry off disk and drives the future, because it is the half that has a
//! `Ctx`, a pane and a report to write into.
//!
//! # Task ownership
//!
//! **V15** fills this, together with the capture side in `core/persist.rs` and
//! the restore side in `core/restore.rs`.
//!
//! V02 planted the file, froze the two additive `PaneSnapshot` fields the
//! capture writes, and implemented the ref shape validation — a validating
//! constructor left as `todo!()` would have panicked in three earlier waves.

use std::collections::HashSet;
use std::time::Duration;

use amx_core::agent::{AgentKind, RefKind, RefSource, SessionRef};
use thiserror::Error;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use super::registry::{AgentStanza, REF_SLOT, Registry};
use crate::actor::{Drive, DriveError, Driven, PaneCommand, PaneHandle, SnapshotFeed};
use crate::persist::AgentSnapshot;

/// How long a restored pane's shell is given to paint before its resume is
/// typed anyway.
///
/// The wait is on the pane's published frames, never on a clock — the clock is
/// only the bound, and reaching it is not the normal path. Typing into a shell
/// that has painted nothing is still correct (the bytes queue in the pty and
/// the shell reads them when it starts), so the bound resolves to *type it
/// late* rather than to *lose the conversation*: a slow machine must not cost a
/// user their session.
pub const FIRST_PAINT_BUDGET: Duration = Duration::from_secs(5);

/// The byte that separates a dedupe key's four fields.
///
/// NUL, because it is the one byte none of the four can hold: an
/// [`AgentKind`] is `[a-z0-9_-]`, a [`RefSource`] is `amx:` plus one of those,
/// a ref kind is a fixed word, and a [`SessionRef`]'s value is checked free of
/// control characters. So the key is unambiguous by construction rather than by
/// escaping.
const KEY_SEPARATOR: char = '\0';

/// One planned resume: what to type, and the conversation it claims.
///
/// [`argv`](Self::argv) is data all the way down — a vector of words, one of
/// which is the ref, with no quoting anywhere in it. Quoting happens once, at
/// the injection boundary, in [`command_line`].
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Planned {
    /// The stanza this resumes, by id — the alias a snapshot may have been
    /// written with is already resolved.
    pub agent: AgentKind,
    /// The argv, with the ref substituted into the template's one slot.
    pub argv: Vec<String>,
    /// The dedupe key this resume must hold before it is launched.
    pub key: String,
}

/// Why a captured conversation did not become an argv.
///
/// Typed and specific because each one reaches a user through the restore
/// report: 04 §6 requires restore loss to be queryable rather than logged, and
/// "could not resume" would not tell anybody which of these happened.
#[derive(Clone, PartialEq, Eq, Debug, Error)]
pub enum ResumeRefusal {
    /// The pane's agent was known but no conversation was ever captured.
    ///
    /// Not a loss: there was nothing to lose. V01 §4 measured Codex emitting no
    /// `SessionStart` until the user's first prompt, so a pane holding an idle
    /// Codex reaches a restart in exactly this state.
    #[error("no conversation had been captured for this agent yet")]
    NoConversation,
    /// A ref was stored with no source beside it.
    #[error("the stored conversation names no hook path it came from")]
    NoSource,
    /// The snapshot names an agent this build's registry does not have.
    #[error("`{agent}` is not an agent this build knows; its stanza may have been removed")]
    UnknownAgent {
        /// What the snapshot said.
        agent: AgentKind,
    },
    /// The ref's source claims a different agent than the ref does.
    ///
    /// D-M2-7's allowlist at its third gate. A hand-edited `session.json` that
    /// moved a `codex` conversation under a `claude` stanza lands here, and
    /// lands here *before* an argv is built.
    #[error("the conversation was recorded from `{hook_path}`, which is not `{agent}`'s")]
    ForeignSource {
        /// The source the snapshot carried.
        ///
        /// Not spelled `source`: `thiserror` reads a field of that name as the
        /// error's *cause*, and a [`RefSource`] is a value, not an error.
        hook_path: RefSource,
        /// The stanza it was filed under.
        agent: AgentKind,
    },
    /// A `path` ref for an `id` agent, or the other way round.
    #[error("`{agent}` resumes from a {wanted} ref, and this conversation is a {found}")]
    WrongRefKind {
        /// The stanza.
        agent: AgentKind,
        /// What the ref is.
        found: &'static str,
        /// What the stanza accepts.
        wanted: &'static str,
    },
    /// The stanza declares no resume template at all.
    #[error("`{agent}` declares no resume command")]
    NoTemplate {
        /// The stanza.
        agent: AgentKind,
    },
    /// Another pane restored into this conversation first.
    ///
    /// Two panes cannot hold one conversation: the second agent would either
    /// refuse to start or write into a transcript the first one is already
    /// writing. So the second pane restores as a plain shell and is told so.
    #[error("another pane resumed this conversation; this one restored as a plain shell")]
    Claimed,
}

/// Plan the argv that puts `agent`'s conversation back, or say why not.
///
/// The gate order is the order the sentences want: what was captured, then
/// whether this build still knows the agent, then whether the ref is entitled
/// to name it, then whether the stanza can resume it at all.
///
/// # Errors
///
/// The [`ResumeRefusal`] that stopped it — every one of which is a restore
/// report entry, never a log line.
pub fn plan(registry: &Registry, agent: &AgentSnapshot) -> Result<Planned, ResumeRefusal> {
    let Some(session_ref) = &agent.session_ref else {
        return Err(ResumeRefusal::NoConversation);
    };
    let Some(source) = &agent.source else {
        return Err(ResumeRefusal::NoSource);
    };
    let Some(stanza) = registry.resolve(&agent.kind) else {
        return Err(ResumeRefusal::UnknownAgent {
            agent: agent.kind.clone(),
        });
    };
    if !claims(stanza, source) {
        return Err(ResumeRefusal::ForeignSource {
            hook_path: source.clone(),
            agent: stanza.id.clone(),
        });
    }
    if session_ref.kind() != stanza.ref_kind {
        return Err(ResumeRefusal::WrongRefKind {
            agent: stanza.id.clone(),
            found: ref_kind_name(session_ref.kind()),
            wanted: ref_kind_name(stanza.ref_kind),
        });
    }
    if stanza.resume.is_empty() {
        return Err(ResumeRefusal::NoTemplate {
            agent: stanza.id.clone(),
        });
    }
    Ok(Planned {
        key: dedupe_key(source, &stanza.id, session_ref),
        argv: substitute(&stanza.resume, session_ref),
        agent: stanza.id.clone(),
    })
}

/// Whether a ref carrying `source` is entitled to claim `stanza`.
///
/// D-M2-7's allowlist, as one predicate, so that the gate at plan time and the
/// gate at report time can be the same sentence rather than two readings of it.
/// `AgentHub`'s ingestion runs the identical rule on the way in; this is the
/// rule again on the way out of a file a user could have edited in between.
#[must_use]
pub fn claims(stanza: &AgentStanza, source: &RefSource) -> bool {
    source.agent() == stanza.id
}

/// The reservation key for one conversation: `source\0agent\0kind\0value`.
///
/// All four fields, not just the value: two agents can hand out the same
/// opaque id, and a `path` ref and an `id` ref that happen to spell alike are
/// not the same conversation.
#[must_use]
pub fn dedupe_key(source: &RefSource, agent: &AgentKind, session_ref: &SessionRef) -> String {
    let mut key = String::with_capacity(source.as_str().len() + session_ref.value().len() + 16);
    for field in [
        source.as_str(),
        agent.as_str(),
        ref_kind_name(session_ref.kind()),
        session_ref.value(),
    ] {
        if !key.is_empty() {
            key.push(KEY_SEPARATOR);
        }
        key.push_str(field);
    }
    key
}

/// Every conversation this restore has already handed to a pane.
///
/// Reserve-then-spawn, release-on-failure: the order matters because a pane
/// whose shell will not start must not take its conversation down with it —
/// a later pane in the same snapshot has to be able to claim it (D-M2-7).
#[derive(Clone, Debug, Default)]
pub struct Reservations {
    held: HashSet<String>,
}

impl Reservations {
    /// A table holding nothing.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Claim `key`, answering whether this caller got it.
    ///
    /// `false` means somebody else holds it, which is the second-pane case:
    /// that pane restores as a plain shell and the loss is reported.
    #[must_use]
    pub fn reserve(&mut self, key: &str) -> bool {
        self.held.insert(key.to_owned())
    }

    /// Give `key` back, so a later pane may claim it.
    pub fn release(&mut self, key: &str) {
        self.held.remove(key);
    }

    /// Whether `key` is currently held.
    #[must_use]
    pub fn holds(&self, key: &str) -> bool {
        self.held.contains(key)
    }

    /// How many conversations are claimed.
    #[must_use]
    pub fn len(&self) -> usize {
        self.held.len()
    }

    /// Whether nothing is claimed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.held.is_empty()
    }
}

/// The shell command line that runs `argv`, quoted once at the boundary.
///
/// This is the *only* place in the resume path where quoting happens, which is
/// what "argv is data" buys: everything upstream is a vector of words, and the
/// single transformation into shell text is one function with one rule. A word
/// made only of characters no shell reads as syntax is passed through; anything
/// else — including a ref holding a space, a quote or a semicolon — is wrapped
/// in single quotes, with any single quote in it spelled `'\''`, the one
/// escape POSIX sh gives a single-quoted string.
#[must_use]
pub fn command_line(argv: &[String]) -> String {
    let mut line = String::new();
    for word in argv {
        if !line.is_empty() {
            line.push(' ');
        }
        quote_into(word, &mut line);
    }
    line
}

/// Wait for a restored pane's shell to paint, then type `line` into it.
///
/// The wait is on the pane's own published frames — a condition, not a nap —
/// because the prompt painting *is* the readiness signal (D-M2-7). The write
/// goes through [`Drive::Run`], the same bracketed-paste-aware submit
/// `pane.run` uses, so the invocation lands in the user's shell history and the
/// pane stays a shell that survives the agent's eventual exit.
///
/// Cancellation wins over everything: this future is driven by a task the
/// session's shutdown does not join, so it must let go of its pane handle the
/// instant the session is cancelled (R-M1-2 — a task still holding a mailbox
/// sender is a mailbox that has not closed).
///
/// # Errors
///
/// [`LaunchError`], which the caller can only log: by the time this resolves,
/// the restore report the user reads has already been answered.
pub async fn type_in(
    frames: SnapshotFeed,
    handle: PaneHandle,
    line: String,
    budget: Duration,
    cancel: CancellationToken,
) -> Result<Driven, LaunchError> {
    tokio::select! {
        () = cancel.cancelled() => Err(LaunchError::Cancelled),
        outcome = after_first_paint(frames, handle, line, budget) => outcome,
    }
}

/// Why a planned resume never reached its pane.
#[derive(Clone, PartialEq, Eq, Debug, Error)]
pub enum LaunchError {
    /// The session was cancelled before the shell was ready.
    #[error("the session stopped before the conversation could be typed back")]
    Cancelled,
    /// The pane went away.
    #[error("the pane was gone before the conversation could be typed back")]
    Gone,
    /// The pane refused the write.
    #[error(transparent)]
    Drive(#[from] DriveError),
}

/// The body of [`type_in`], less the cancellation arm.
async fn after_first_paint(
    mut frames: SnapshotFeed,
    handle: PaneHandle,
    line: String,
    budget: Duration,
) -> Result<Driven, LaunchError> {
    match tokio::time::timeout(budget, frames.changed()).await {
        // The shell painted: its prompt is on the grid and it is reading.
        Ok(true) => {}
        // The pane's frames ended, which means the pane did.
        Ok(false) => return Err(LaunchError::Gone),
        // Nothing painted inside the budget. The bytes still reach the child
        // through the pty, so type it late rather than lose it; see
        // `FIRST_PAINT_BUDGET`.
        Err(_) => {}
    }
    let (tx, rx) = oneshot::channel();
    handle
        .send(PaneCommand::Drive {
            what: Drive::Run(line),
            reply: tx,
        })
        .await
        .map_err(|_| LaunchError::Gone)?;
    rx.await.map_err(|_| LaunchError::Gone)?.map_err(Into::into)
}

/// The template with its one `{ref}` element replaced by the ref's value.
///
/// Whole elements only: [`AgentStanza::check`] refuses a template that embeds
/// the slot inside a larger argument, so there is no fragment to substitute
/// into and no quoting question to answer here.
fn substitute(template: &[String], session_ref: &SessionRef) -> Vec<String> {
    template
        .iter()
        .map(|word| {
            if word == REF_SLOT {
                session_ref.value().to_owned()
            } else {
                word.clone()
            }
        })
        .collect()
}

/// How a ref kind is spelled in a dedupe key and in a refusal.
const fn ref_kind_name(kind: RefKind) -> &'static str {
    match kind {
        RefKind::Id => "id",
        RefKind::Path => "path",
    }
}

/// Append `word` to `out`, quoted if a shell would read anything in it.
fn quote_into(word: &str, out: &mut String) {
    if !word.is_empty() && word.chars().all(is_bare) {
        out.push_str(word);
        return;
    }
    out.push('\'');
    for ch in word.chars() {
        if ch == '\'' {
            // Close the quoted run, emit an escaped quote, reopen: the only
            // way POSIX sh admits a single quote inside single quotes.
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
}

/// Whether `ch` can stand in a shell word without quoting.
///
/// Deliberately a short allowlist rather than a list of metacharacters: a
/// character nobody thought about is quoted, which is the failure direction
/// that costs nothing.
const fn is_bare(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | '/' | ':' | '=' | '@' | '%' | '+')
}
