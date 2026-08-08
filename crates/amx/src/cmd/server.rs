//! `amx server`: the daemon entry point, and the successor's entry point.
//!
//! Runs one session's actors in the foreground and returns when they are all
//! joined. `amx` starts this detached (04 §1) and a user may run it directly to
//! watch a session; either way it is the same code path, because there is no
//! second "monolithic" mode for the server to also be (fixes W4).
//!
//! `--handoff-import <socket>` is the same verb reached from the other
//! direction: instead of binding a socket and restoring from disk, the process
//! connects to a running server's private handoff socket, takes its panes as
//! descriptors and its state as a manifest, and becomes it
//! (`docs/09-m3-plan.md` §3). Hidden surface — an exporter spawns it, nobody
//! types it — and the token that authenticates it arrives on stdin, never on
//! the command line, because `/proc/*/cmdline` is world-readable (D-M3-6
//! point 1).

use std::path::PathBuf;
use std::process::ExitCode;

use amx_core::Ctx;
use amx_server::actor::gateway::GatewayError;
use amx_server::handoff::protocol::{Timeouts, Token};
use amx_server::session::import::{ImportOptions, import};
use amx_server::session::serve::{ServeError, ServeReport, StopOn, serve};
use clap::ArgMatches;

use crate::cli::HANDOFF_IMPORT;

/// Run the session server until it is stopped.
///
/// Losing the socket to another server is **not** an error exit. Two `amx`
/// invocations racing each other both spawn a server, both servers try to bind,
/// and exactly one wins; the loser has nothing to report and nothing to clean
/// up, and its client is about to attach to the winner. Failing loudly here
/// would turn the ordinary outcome of a race into a visible error in the
/// client that lost it.
pub async fn run(ctx: Ctx, args: &ArgMatches) -> anyhow::Result<ExitCode> {
    init_tracing();
    if let Some(socket) = args.get_one::<String>(HANDOFF_IMPORT) {
        return take_over(ctx, PathBuf::from(socket)).await;
    }
    let session = ctx.session.clone();
    match serve(ctx, StopOn::Signals).await {
        Ok(report) => Ok(exit_code(&report)),
        Err(ServeError::Gateway(GatewayError::AlreadyRunning { path })) => {
            tracing::info!(session = %session, socket = %path.display(), "already running");
            Ok(ExitCode::SUCCESS)
        }
        Err(err) => Err(err.into()),
    }
}

/// Become the server that is already running (§3).
///
/// Two exits and no third, which is the strict abort rule as the process sees
/// it: this server owned the session and served it, or it owned nothing and
/// says so with a non-zero status. There is no "started but did not take over"
/// — the exporter is still serving in that case, and its own report carries the
/// reason (D-M3-6).
async fn take_over(ctx: Ctx, socket: PathBuf) -> anyhow::Result<ExitCode> {
    // On the blocking pool, though nothing else is running yet: the exporter
    // writes the token as soon as it has spawned this process, and a read that
    // parked a runtime thread would be a habit worth not having on a path that
    // goes on to do all of §3 the same way.
    let token =
        tokio::task::spawn_blocking(|| Token::read_from(&mut std::io::stdin().lock())).await??;
    let options = ImportOptions {
        socket,
        token,
        timeouts: Timeouts::DEFAULT,
        stop: StopOn::Signals,
    };
    match import(ctx, options).await {
        Ok(report) => Ok(exit_code(&report)),
        Err(err) => {
            // Not an `Err` out of `run`: the failure has already been handled
            // exactly as §3 says to handle it — nothing bound, nothing served —
            // and what is left to do is tell the exporter's operator, who is
            // reading this process's stderr through the log.
            tracing::error!(error = %err, ending = ?err.ending(), "the handoff did not commit");
            Ok(ExitCode::FAILURE)
        }
    }
}

/// A session that shut down with panicked tasks is a failed run, whichever
/// entrance it came in through.
fn exit_code(report: &ServeReport) -> ExitCode {
    if report.clean() {
        return ExitCode::SUCCESS;
    }
    tracing::error!(?report, "the session shut down with panicked tasks");
    ExitCode::FAILURE
}

/// Install the log subscriber.
///
/// A daemonized server's stdio is `/dev/null` (that is what makes it a daemon),
/// so this reaches a human only when the server is run in the foreground. A log
/// file under the session's runtime directory is the obvious next step and is
/// not M0 scope.
///
/// `$RUST_LOG` is the one environment variable read outside `Env`. It is not
/// the W9 hazard the rule is about: it selects a log level, never a path or an
/// identity, so it cannot make two sessions share state or make one test's
/// environment reach another's.
fn init_tracing() {
    use tracing_subscriber::EnvFilter;

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    // A second `amx server` in one process would be the only way this fails,
    // and there is no such thing: the binary runs one command.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}
