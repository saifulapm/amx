//! `amx keys` — print the resolved keybinding table.
//!
//! 04 §7 promises the bindings are introspectable and for three milestones
//! there was nothing to introspect: the prefix was a `const` and the table was
//! a `match` on byte literals (`docs/11-m4-plan.md` D-M4-8). X07 turned both
//! into data read from `[keys]`, and this is what makes the result visible —
//! including *which* bindings came from the file, because a table that cannot
//! show you what your own config did is a table you debug by guessing.
//!
//! It reads configuration and talks to no server: the bindings live entirely
//! client-side, so this answers from [`Ctx::config_path`] alone and works with
//! no session running. That is also what makes it the thing to run when a
//! rebound prefix has left you unable to reach the prefix layer.
//!
//! It is the **only** surface a `[keys]` diagnostic reaches a person through.
//! An attaching client is a keystroke away from the alternate screen, so a
//! warning printed there is a warning nobody sees; `amx attach` logs the
//! diagnostics and runs the shipped bindings, and this command prints them
//! where they can be read. Exit status stays zero either way: a rejected
//! binding is news about a file, not a failed command.
//!
//! [`Ctx::config_path`]: amx_core::Ctx::config_path

use std::process::ExitCode;

use amx_client::config::{self, Binding, Bindings};
use amx_core::{ConfigDiagnostic, Env};
use clap::ArgMatches;
use serde_json::json;

use crate::cli::JSON;
use crate::ctx_of;

/// Run `amx keys`.
///
/// # Errors
///
/// Only what deriving the session's paths can fail with: the config file
/// itself never fails the command, because an unreadable one is a diagnostic
/// and the shipped bindings.
pub async fn run(env: &Env, root: &ArgMatches, sub: &ArgMatches) -> anyhow::Result<ExitCode> {
    let ctx = ctx_of(env, root, None)?;
    let (settings, diagnostics) = config::load(&ctx.config_path);

    if sub.get_flag(JSON) {
        println!(
            "{}",
            serde_json::to_string_pretty(&as_json(
                &ctx.config_path.display().to_string(),
                &settings.bindings,
                &diagnostics,
            ))?
        );
        return Ok(ExitCode::SUCCESS);
    }

    print!("{}", table(&settings.bindings));
    for entry in &diagnostics {
        eprintln!(
            "{}: {}",
            entry.section.unwrap_or("config.toml"),
            entry.message
        );
    }
    Ok(ExitCode::SUCCESS)
}

/// The whole answer as one JSON object.
///
/// The path is in it because "which file did you read" is the second question
/// anybody asks when a binding did not take, and answering it costs a string.
fn as_json(path: &str, bindings: &Bindings, diagnostics: &[ConfigDiagnostic]) -> serde_json::Value {
    json!({
        "config": path,
        "prefix": {
            "key": config::key_name(bindings.prefix()),
            "source": bindings.prefix_source().name(),
        },
        "bindings": bindings
            .rows()
            .map(|(key, Binding { action, source })| json!({
                "key": config::key_name(key),
                "action": action.name(),
                "source": source.name(),
            }))
            .collect::<Vec<_>>(),
        "rejected": diagnostics
            .iter()
            .map(|entry| json!({
                "section": entry.section,
                "message": entry.message,
            }))
            .collect::<Vec<_>>(),
    })
}

/// The human form: the prefix on its own line, then one row per binding.
///
/// Columns are padded to the widest row rather than to fixed widths, the same
/// rule `session report` renders by: a rebound table's keys are whatever the
/// user's keyboard can emit, and a column sized for `next-attention` would
/// leave a shipped table full of trailing space.
fn table(bindings: &Bindings) -> String {
    let rows: Vec<(String, &str, &str)> = bindings
        .rows()
        .map(|(key, Binding { action, source })| {
            (config::key_name(key), action.name(), source.name())
        })
        .collect();
    let key_width = rows
        .iter()
        .map(|(key, ..)| key.chars().count())
        .chain(std::iter::once(KEY.len()))
        .max()
        .unwrap_or(0);
    let action_width = rows
        .iter()
        .map(|(_, action, _)| action.len())
        .chain(std::iter::once(ACTION.len()))
        .max()
        .unwrap_or(0);

    let mut out = format!(
        "prefix {}, from {}\n\n{KEY:key_width$}  {ACTION:action_width$}  {SOURCE}\n",
        config::key_name(bindings.prefix()),
        bindings.prefix_source().name(),
    );
    for (key, action, source) in rows {
        out.push_str(&format!(
            "{key:key_width$}  {action:action_width$}  {source}\n"
        ));
    }
    out
}

/// The table's column headings, named once so the padding and the header
/// cannot disagree about how wide a column is.
const KEY: &str = "key";
/// See [`KEY`].
const ACTION: &str = "action";
/// See [`KEY`].
const SOURCE: &str = "source";
