//! `amx session list|attach|stop|delete|report` (04 §1).
//!
//! "A daemon that outlives terminals must be discoverable and stoppable from
//! the CLI." The lifecycle verbs are thin fronts for one function each in
//! [`amx_server::session::registry`]; what lives here is the output format and
//! the exit codes, which are the parts a script depends on.
//!
//! `report` is the exception: it is a real control call (`session.report`,
//! a row in the shared method table like any other), and it lives here rather
//! than on the generated JSON path because it is the one verb whose output is
//! meant to be read. 04 §6 requires restore loss to be "shown in the status
//! line and queryable via `amx session report` — never log-only", and a wall
//! of JSON is queryable but not readable. `--json` gives scripts the reply
//! verbatim.

use std::process::ExitCode;
use std::time::Duration;

use amx_core::Env;
use amx_proto::control::Method;
use amx_proto::control::session::{RestoreEntity, RestoreLoss, RestoreReport, RestoreSeverity};
use amx_server::session::registry::{self, StopOutcome};
use anyhow::Context as _;
use clap::ArgMatches;

use crate::cli::JSON;
use crate::cmd::{attach, call};
use crate::ctx_of;

/// How long `stop` waits for the server to go away after signalling it.
pub const STOP_TIMEOUT: Duration = Duration::from_secs(10);

/// Run one `amx session …` verb.
pub async fn run(env: &Env, root: &ArgMatches, matches: &ArgMatches) -> anyhow::Result<ExitCode> {
    let (verb, sub) = matches.subcommand().context("amx session needs a verb")?;
    // `list` is the one verb with no name argument, so the lookup is per-arm:
    // asking clap for an argument a subcommand never declared is a panic, not
    // a `None`.
    let named = |sub: &ArgMatches| sub.get_one::<String>("name").cloned();

    match verb {
        "list" => list(env),
        "attach" => {
            let ctx = ctx_of(env, root, named(sub).as_deref())?;
            attach::run(&ctx, attach::Options::default()).await
        }
        "stop" => stop(env, root, named(sub).as_deref()).await,
        "delete" => delete(env, root, named(sub).as_deref()),
        "report" => report(env, root, sub).await,
        other => anyhow::bail!("unknown session verb: {other}"),
    }
}

/// Print the sessions with a server answering, one per line.
///
/// Name and socket, tab separated and nothing else: a list a script can cut is
/// worth more than a table a human can admire, and the human can read this too.
/// Sessions whose server is gone are not listed at all — a runtime directory is
/// evidence that a session *ran*, and `amx session delete` is what removes one.
fn list(env: &Env) -> anyhow::Result<ExitCode> {
    let running = registry::list(env).context("enumerate sessions")?;
    for entry in &running {
        println!("{}\t{}", entry.session, entry.socket.display());
    }
    Ok(ExitCode::SUCCESS)
}

/// Stop a session's server and wait for its socket to go quiet.
async fn stop(env: &Env, root: &ArgMatches, named: Option<&str>) -> anyhow::Result<ExitCode> {
    let ctx = ctx_of(env, root, named)?;
    match registry::stop(&ctx, STOP_TIMEOUT)
        .await
        .with_context(|| format!("stop session {}", ctx.session))?
    {
        StopOutcome::Stopped { pid } => {
            println!("stopped {} (pid {pid})", ctx.session);
            Ok(ExitCode::SUCCESS)
        }
        StopOutcome::NotRunning => {
            eprintln!("amx: session {} is not running", ctx.session);
            Ok(ExitCode::SUCCESS)
        }
    }
}

/// Remove a stopped session's directories.
fn delete(env: &Env, root: &ArgMatches, named: Option<&str>) -> anyhow::Result<ExitCode> {
    let ctx = ctx_of(env, root, named)?;
    registry::delete(&ctx).with_context(|| format!("delete session {}", ctx.session))?;
    println!("deleted {}", ctx.session);
    Ok(ExitCode::SUCCESS)
}

/// Print what this server's startup restore could not do faithfully.
///
/// Exit status is success either way: the report answers a question, and a
/// session that lost a pane last night is not a failed command today.
async fn report(env: &Env, root: &ArgMatches, matches: &ArgMatches) -> anyhow::Result<ExitCode> {
    let reply = call::one_shot(
        env,
        root,
        Method::SessionReport.wire_name(),
        serde_json::json!({}),
    )
    .await?;

    if matches.get_flag(JSON) {
        println!(
            "{}",
            serde_json::to_string_pretty(&reply).context("format the reply")?
        );
        return Ok(ExitCode::SUCCESS);
    }

    let reply: amx_proto::control::session::ReportReply =
        serde_json::from_value(reply).context("decode the restore report")?;
    print!("{}", format_report(&reply.report));
    Ok(ExitCode::SUCCESS)
}

/// What a column shows when the entry does not carry that field.
const ABSENT: &str = "-";

/// Render a restore report as lines a human reads: a count, then one line per
/// entry — severity, what it was, its label, the path it was about, and why.
///
/// Columns are padded to the widest entry rather than to fixed widths: the
/// paths in a real report are session-specific, and a column sized for the
/// worst case would push the reason off an 80-column terminal for every report
/// that has none.
#[must_use]
pub fn format_report(report: &RestoreReport) -> String {
    if report.entries.is_empty() {
        return "no losses to report\n".to_owned();
    }

    let count = |severity| {
        report
            .entries
            .iter()
            .filter(|entry| entry.severity == severity)
            .count()
    };
    let mut out = format!(
        "{} lost, {} degraded\n",
        count(RestoreSeverity::Lost),
        count(RestoreSeverity::Degraded)
    );

    let rows: Vec<[String; 5]> = report.entries.iter().map(columns).collect();
    // The reason is last, so nothing after it needs padding.
    let widths: Vec<usize> = (0..4)
        .map(|col| {
            rows.iter()
                .map(|row| row[col].chars().count())
                .max()
                .unwrap_or(0)
        })
        .collect();
    for row in &rows {
        for (col, cell) in row.iter().enumerate().take(4) {
            let pad = widths[col].saturating_sub(cell.chars().count());
            out.push_str(cell);
            out.extend(std::iter::repeat_n(' ', pad + 2));
        }
        out.push_str(&row[4]);
        out.push('\n');
    }
    out
}

/// One entry's five columns.
fn columns(entry: &RestoreLoss) -> [String; 5] {
    let severity = match entry.severity {
        RestoreSeverity::Lost => "lost",
        RestoreSeverity::Degraded => "degraded",
    };
    let entity = match entry.entity {
        RestoreEntity::Session => "session",
        RestoreEntity::Workspace => "workspace",
        RestoreEntity::Pane => "pane",
    };
    [
        severity.to_owned(),
        entity.to_owned(),
        entry.label.clone().unwrap_or_else(|| ABSENT.to_owned()),
        entry
            .path
            .as_deref()
            .map_or_else(|| ABSENT.to_owned(), |path| path.display().to_string()),
        entry.reason.clone(),
    ]
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use amx_core::{PaneId, WorkspaceId};

    use super::{RestoreEntity, RestoreLoss, RestoreReport, RestoreSeverity, format_report};

    fn entry(severity: RestoreSeverity, entity: RestoreEntity) -> RestoreLoss {
        RestoreLoss {
            severity,
            entity,
            workspace: None,
            pane: None,
            label: None,
            path: None,
            reason: String::new(),
        }
    }

    #[test]
    fn session_report_formats_entries_human_readably() {
        let report = RestoreReport {
            entries: vec![
                RestoreLoss {
                    pane: Some(PaneId::new_v4()),
                    label: Some("editor".to_owned()),
                    path: Some(PathBuf::from("/home/x/project")),
                    reason: "spawn failed: no such file or directory".to_owned(),
                    ..entry(RestoreSeverity::Lost, RestoreEntity::Pane)
                },
                RestoreLoss {
                    workspace: Some(WorkspaceId::new_v4()),
                    label: Some("build".to_owned()),
                    reason: "every pane was pruned".to_owned(),
                    ..entry(RestoreSeverity::Degraded, RestoreEntity::Workspace)
                },
            ],
        };

        let out = format_report(&report);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 3, "a count and one line per entry: {out}");
        assert_eq!(lines[0], "1 lost, 1 degraded");

        // Everything the entry knows is on its line, and nothing of the wire
        // shape is: this is the one verb whose output is meant to be read.
        assert!(lines[1].starts_with("lost      pane"), "{}", lines[1]);
        assert!(lines[1].contains("editor"));
        assert!(lines[1].contains("/home/x/project"));
        assert!(lines[1].ends_with("spawn failed: no such file or directory"));
        assert!(lines[2].starts_with("degraded  workspace"), "{}", lines[2]);
        assert!(lines[2].ends_with("every pane was pruned"));
        assert!(!out.contains('{'), "the human form is not JSON: {out}");

        // The columns line up: both entries put their label at the same
        // offset, which is what makes a report of a dozen panes scannable.
        let at = |line: &str, needle: &str| line.find(needle).expect("column present");
        assert_eq!(at(lines[1], "editor"), at(lines[2], "build"));

        // A field the entry does not carry is a placeholder, not a blank that
        // silently shifts the column after it.
        assert!(lines[2].contains(" -  "), "{}", lines[2]);
    }

    #[test]
    fn a_report_with_no_entries_says_so_in_one_line() {
        let out = format_report(&RestoreReport::default());
        assert_eq!(out, "no losses to report\n");
    }
}
