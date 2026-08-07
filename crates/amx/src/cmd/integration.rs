//! `amx integration install|uninstall|status`.
//!
//! 04 §8: "Integrations have a lifecycle, not just an install: `amx integration
//! install|uninstall|status` with version markers in installed hook assets, and
//! `status` runs after self-update — a stale `amx _hook` is worse for amx than
//! for herdr, since hooks feed the fusion tier."
//!
//! What V01 measured, and what **V10** must therefore say out loud:
//!
//! - **Claude Code** needs no hook-approval step. A settings file written
//!   seconds before launch ran its hooks. But the *folder-trust* dialog is a
//!   hard gate: in a directory Claude Code had never seen, no hook fired at all
//!   for the eight seconds the dialog was up, and the dialog does not even
//!   mention hooks. So install is genuinely non-interactive, and honest status
//!   wording is: installed hooks run in any workspace the user has already
//!   trusted; in a brand new one their next interactive launch asks once.
//! - **Codex 0.147.0 is not the Codex the plan described.** `codex features
//!   list` prints `hooks  stable  true` — enabled by default, no flag to set;
//!   there is no `codex_hooks` feature (R-M2-2 should be withdrawn, and 04 §5's
//!   spelling was right). Config is `$CODEX_HOME/hooks.json`, a JSON file with
//!   PascalCase event keys shaped like Claude Code's settings block. And the
//!   trust gate is real and stronger than described: a fresh `hooks.json`
//!   raises a blocking startup prompt, declining is silent and total (a full
//!   turn ran with zero hook invocations), `codex exec` fires nothing and warns
//!   nothing, and the gate is keyed to the file's content hash. **No
//!   user-readable trust record was found**, so `status` must report that it
//!   cannot see trust state rather than implying the hooks are live.
//!
//! Two rules for the editing itself, both from herdr's scars:
//!
//! - **Preserve foreign entries.** Only amx-owned entries, matched by command
//!   shape, are ever touched, and reinstalling over a current install is a
//!   no-op write.
//! - **`status` verifies the thing that actually breaks.** herdr's check greps
//!   a version comment and reports `current` for an installation that silently
//!   does nothing; amx's checks the marker *and* that the referenced binary
//!   exists and executes.
//!
//! This module is the verb: it resolves which agents to act on from the
//! registry, calls [`crate::integration`], and writes the result for a human.
//! Everything it prints is a fact about a file — nothing here says "done".
//!
//! # Task ownership
//!
//! **V10** fills this and `crates/amx/src/integration/{mod,claude,codex,
//! command,edit,hooks}.rs` beside it. The event list per agent is registry
//! data (V03's stanzas), not a list here — an agent's name anywhere but in its
//! stanza is W6 growing back. `status` after self-update is M3's wiring: the
//! function stays callable, and nothing here calls it.
//!
//! V02 planted the file and the clap tree so no wave task edits `cli.rs`.

use std::path::Path;
use std::process::ExitCode;

use amx_core::Env;
use amx_server::agent::registry::Registry;
use anyhow::{Context as _, bail};
use clap::ArgMatches;

use crate::integration::{self, Outcome, Plan, State, Trust};

/// The clap id of the optional positional naming one agent.
const AGENT: &str = "agent";

/// Run the integration verb `matches` names.
pub async fn run(env: &Env, matches: &ArgMatches, sub: &ArgMatches) -> anyhow::Result<ExitCode> {
    let ctx = crate::ctx_of(env, matches, None)?;
    let registry = registry(&ctx.config_path);
    let (verb, args) = sub
        .subcommand()
        .context("`amx integration` needs install, uninstall or status")?;
    let only = args.get_one::<String>(AGENT).map(String::as_str);
    let plans = integration::plans(env, &registry, only)?;
    if plans.is_empty() {
        bail!("no agent in the registry has a hook integration amx can install");
    }

    let program = integration::program()?;
    let rows = match verb {
        "install" => install(&plans, &program)?,
        "uninstall" => uninstall(&plans)?,
        "status" => return status(&plans, &program).await,
        other => bail!("unknown integration verb `{other}`"),
    };
    print!("{}", render(&rows));
    Ok(ExitCode::SUCCESS)
}

/// The registry this machine runs on: the builtin, plus the user's override.
///
/// The same two-step the session's hub performs, for the same reason — an
/// installer that read only the compiled-in stanzas would refuse to install for
/// an agent the user has added, and would install a different event list than
/// the running server subscribes to.
fn registry(config_path: &Path) -> Registry {
    let builtin = Registry::builtin();
    let path = Registry::override_path(config_path);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return builtin;
    };
    let (merged, diagnostics) = builtin.with_override(&text);
    for diagnostic in diagnostics {
        eprintln!("{}: {diagnostic}", path.display());
    }
    merged
}

/// Write every plan's entries.
fn install(plans: &[Plan], program: &Path) -> anyhow::Result<Vec<Row>> {
    plans
        .iter()
        .map(|plan| {
            let outcome = plan.install(program)?;
            Ok(Row {
                agent: plan.agent.as_str().to_owned(),
                verdict: verdict(outcome).to_owned(),
                detail: format!("{} · {}", plan.label, plan.path.display()),
                // The caveats print on every install, including the no-op one:
                // "nothing to do" is not the same as "the hooks are running",
                // and for Codex the difference is a prompt nobody has answered.
                notes: plan.notes.iter().map(|note| (*note).to_owned()).collect(),
            })
        })
        .collect()
}

/// Remove every plan's entries.
fn uninstall(plans: &[Plan]) -> anyhow::Result<Vec<Row>> {
    plans
        .iter()
        .map(|plan| {
            let outcome = plan.uninstall()?;
            Ok(Row {
                agent: plan.agent.as_str().to_owned(),
                verdict: verdict(outcome).to_owned(),
                detail: format!("{} · {}", plan.label, plan.path.display()),
                notes: Vec::new(),
            })
        })
        .collect()
}

/// Report on every plan, and fail the command if any is broken.
///
/// The exit code is the machine-readable half: a `status` that printed
/// `broken` and exited 0 would be discovered by nobody, and the state it
/// reports — hooks configured, binary gone, agent silently unmonitored — is
/// precisely the one herdr cannot see.
async fn status(plans: &[Plan], program: &Path) -> anyhow::Result<ExitCode> {
    let mut rows = Vec::with_capacity(plans.len());
    let mut broken = false;
    for plan in plans {
        let status = plan.status(program).await?;
        broken |= status.state == State::Broken;
        let mut notes = Vec::new();
        if plan.trust == Trust::Unreadable
            && matches!(status.state, State::Current | State::Outdated)
        {
            notes.push(
                "amx cannot see whether you have approved these hooks; the agent records \
                 that where nothing else can read it, and until you do they do not run."
                    .to_owned(),
            );
        }
        rows.push(Row {
            agent: plan.agent.as_str().to_owned(),
            verdict: state(&status.state).to_owned(),
            detail: status.detail,
            notes,
        });
    }
    print!("{}", render(&rows));
    Ok(if broken {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

/// One agent's line, and whatever has to be said under it.
#[derive(Clone, PartialEq, Eq, Debug)]
struct Row {
    /// The agent's registry id.
    agent: String,
    /// What happened, or what is true.
    verdict: String,
    /// The evidence for it.
    detail: String,
    /// Caveats, printed indented beneath.
    notes: Vec<String>,
}

/// The word for an outcome.
fn verdict(outcome: Outcome) -> &'static str {
    match outcome {
        Outcome::Unchanged => "unchanged",
        Outcome::Installed { .. } => "installed",
        Outcome::Removed { .. } => "removed",
    }
}

/// The word for a state.
fn state(state: &State) -> &'static str {
    match state {
        State::Absent => "not installed",
        State::Current => "current",
        State::Outdated => "outdated",
        State::Broken => "broken",
    }
}

/// Render rows as lines a human reads.
///
/// Columns are padded to the widest entry, the way `session report` does it:
/// with two agents and short verdicts a fixed width would waste half an
/// 80-column terminal, and the detail column is last so nothing after it needs
/// padding at all.
fn render(rows: &[Row]) -> String {
    let width = |pick: fn(&Row) -> &str| {
        rows.iter()
            .map(|row| pick(row).chars().count())
            .max()
            .unwrap_or(0)
    };
    let agent = width(|row| &row.agent);
    let verdict = width(|row| &row.verdict);

    let mut out = String::new();
    for row in rows {
        pad(&mut out, &row.agent, agent);
        pad(&mut out, &row.verdict, verdict);
        out.push_str(&row.detail);
        out.push('\n');
        for note in &row.notes {
            out.push_str("  note: ");
            out.push_str(note);
            out.push('\n');
        }
    }
    out
}

/// One padded column, with the two spaces that separate it from the next.
fn pad(out: &mut String, cell: &str, width: usize) {
    out.push_str(cell);
    let fill = width.saturating_sub(cell.chars().count()) + 2;
    out.extend(std::iter::repeat_n(' ', fill));
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

    use super::{Outcome, Row, State, render, state, verdict};

    fn row(agent: &str, verdict: &str, notes: &[&str]) -> Row {
        Row {
            agent: agent.to_owned(),
            verdict: verdict.to_owned(),
            detail: "/home/x/.claude/settings.json".to_owned(),
            notes: notes.iter().map(|note| (*note).to_owned()).collect(),
        }
    }

    #[test]
    fn rows_line_up_and_notes_sit_under_the_agent_they_belong_to() {
        let out = render(&[
            row("claude", "current", &[]),
            row("codex", "not installed", &["trust is not readable"]),
        ]);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 3, "{out}");
        let at = |line: &str| line.find("/home").expect("the detail column");
        assert_eq!(at(lines[0]), at(lines[1]), "columns do not line up: {out}");
        assert_eq!(lines[2], "  note: trust is not readable");
    }

    #[test]
    fn every_outcome_and_state_has_a_word_of_its_own() {
        let words = [
            verdict(Outcome::Unchanged),
            verdict(Outcome::Installed { entries: 1 }),
            verdict(Outcome::Removed { entries: 1 }),
        ];
        assert_eq!(
            words.len(),
            words
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
        );

        assert_eq!(state(&State::Absent), "not installed");
        assert_eq!(state(&State::Broken), "broken");
    }
}
