//! `amx server`: the daemon entry point.
//!
//! Runs one session's actors in the foreground and returns when they are all
//! joined. `amx` starts this detached (04 §1) and a user may run it directly to
//! watch a session; either way it is the same code path, because there is no
//! second "monolithic" mode for the server to also be (fixes W4).

use std::process::ExitCode;

use amx_core::Ctx;
use amx_server::actor::gateway::GatewayError;
use amx_server::session::serve::{ServeError, StopOn, serve};

/// Run the session server until it is stopped.
///
/// Losing the socket to another server is **not** an error exit. Two `amx`
/// invocations racing each other both spawn a server, both servers try to bind,
/// and exactly one wins; the loser has nothing to report and nothing to clean
/// up, and its client is about to attach to the winner. Failing loudly here
/// would turn the ordinary outcome of a race into a visible error in the
/// client that lost it.
pub async fn run(ctx: Ctx) -> anyhow::Result<ExitCode> {
    init_tracing();
    let session = ctx.session.clone();
    match serve(ctx, StopOn::Signals).await {
        Ok(report) if report.clean() => Ok(ExitCode::SUCCESS),
        Ok(report) => {
            tracing::error!(?report, "the session shut down with panicked tasks");
            Ok(ExitCode::FAILURE)
        }
        Err(ServeError::Gateway(GatewayError::AlreadyRunning { path })) => {
            tracing::info!(session = %session, socket = %path.display(), "already running");
            Ok(ExitCode::SUCCESS)
        }
        Err(err) => Err(err.into()),
    }
}

/// Install the log subscriber.
///
/// A daemonized server's stdio is `/dev/null` (that is what makes it a daemon),
/// so this reaches a human only when the server is run in the foreground. A log
/// file under the session's runtime directory is the obvious next step and is
/// not M0 scope.
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
