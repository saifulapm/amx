//! `amx` and `amx attach`: probe, daemonize if absent, attach (04 §1).
//!
//! The three steps are one function, [`ensure_running`], because they are one
//! decision: "is this session running, and if not, start it". A socket that
//! answers means attach; a socket nothing answers on is a file that outlived
//! its process and is removed; no socket at all and a removed one lead to the
//! same place, a detached `amx server` and a poll until it answers.
//!
//! [`full`] is the ordinary client — chrome, layout, every visible pane — and
//! [`crate::cmd::viewport`] is the degenerate one-pane form of it. Both run
//! the wired `App` loop: stdin through the modal input machine, frames off
//! the bound streams, detach on prefix `d` (the input machine's own verb).

use std::process::ExitCode;

use amx_client::app::App;
use amx_client::term::{self, Sigwinch};
use amx_core::{Ctx, PaneId};
use amx_proto::ClientInfo;
use amx_server::session::daemon::{self, READY_TIMEOUT};
use amx_server::session::probe::clear_if_stale;
use anyhow::Context as _;
use clap::ArgMatches;

/// What `amx attach` was asked for.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Options {
    /// Attach to one pane, full-screen and without chrome.
    pub pane: Option<PaneId>,
    /// Take size authority for that pane.
    pub takeover: bool,
}

impl Options {
    /// Read the options out of `matches`.
    pub fn parse(matches: &ArgMatches) -> anyhow::Result<Self> {
        let pane = match matches.get_one::<String>("pane") {
            // Pane ids only, for now: the short numbers 04 §6 puts in the UI
            // are `amx_core::ShortNumbers`, whose `resolve` is still `todo!()`
            // and which no wire method exposes yet.
            Some(target) => Some(
                target
                    .parse::<PaneId>()
                    .with_context(|| format!("--pane wants a pane id, which {target:?} is not"))?,
            ),
            None => None,
        };
        Ok(Self {
            pane,
            takeover: matches.get_flag("takeover"),
        })
    }
}

/// How this attach found its server.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Started {
    /// A server was already answering.
    AlreadyRunning,
    /// One was started for this attach.
    Daemonized,
}

/// Attach to the session `ctx` names, starting its server if need be.
///
/// The terminal is checked first, before anything is started: attaching a
/// pipe to a session cannot work, and finding that out after daemonizing would
/// leave a server running for a command that failed.
pub async fn run(ctx: &Ctx, options: Options) -> anyhow::Result<ExitCode> {
    anyhow::ensure!(
        term::window_size(std::io::stdin()).is_ok(),
        "amx attaches a terminal, and stdin is not one"
    );
    ensure_running(ctx).await?;
    match options.pane {
        Some(pane) => crate::cmd::viewport::one_pane(ctx, pane, options.takeover).await,
        None => full(ctx).await,
    }
}

/// Make sure a server is answering on `ctx`'s socket, starting one if not.
pub async fn ensure_running(ctx: &Ctx) -> anyhow::Result<Started> {
    if clear_if_stale(&ctx.socket)
        .context("probe the session socket")?
        .is_running()
    {
        return Ok(Started::AlreadyRunning);
    }

    let exe = daemon::current_exe()?;
    let args = [
        "server".into(),
        "--session".into(),
        ctx.session.to_string().into(),
    ];
    let mut started = daemon::spawn_detached(&exe, &args).context("start the session server")?;

    // The spawn may lose the bind race with another `amx` that started at the
    // same moment; that is not a failure, because what this waits for is a
    // server answering, not *this* server answering.
    daemon::await_ready(&ctx.socket, READY_TIMEOUT).await?;
    // ...and if it did lose, it has already exited, so reap it here rather
    // than leaving a zombie for as long as this client stays attached.
    let _ = started.try_wait();
    Ok(Started::Daemonized)
}

/// How this client identifies itself in the handshake.
///
/// `term` is `None`: `$TERM` is a process environment variable and this binary
/// reads the environment exactly once, into `Env`, which has no field for it
/// (T01's contract). It is not used by anything server-side in M0.
#[must_use]
pub fn client_info() -> ClientInfo {
    ClientInfo {
        name: "amx".to_owned(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
        term: None,
    }
}

/// The ordinary client: chrome, layout, every visible pane, live.
async fn full(ctx: &Ctx) -> anyhow::Result<ExitCode> {
    let app = App::attach(
        &ctx.socket,
        std::io::stdin(),
        std::io::stdout(),
        client_info(),
    )
    .await
    .context("attach to the session")?;

    let mut out = std::io::stdout();
    let sigwinch = Sigwinch::install().context("watch for terminal resizes")?;
    let stdin = tokio::io::stdin();

    // The loop ends on prefix `d` (the input machine's detach verb) or on
    // stdin closing; dropping the app then restores the terminal — raw mode
    // off, alt screen left — and the session keeps running.
    app.run(sigwinch, stdin, |bytes| {
        use std::io::Write as _;
        out.write_all(bytes)?;
        out.flush()
    })
    .await
    .context("run the attached client")?;
    Ok(ExitCode::SUCCESS)
}

/// Write one frame to the terminal.
pub fn flush(out: &mut impl std::io::Write, frame: &[u8]) -> anyhow::Result<()> {
    out.write_all(frame).context("write to the terminal")?;
    out.flush().context("flush the terminal")?;
    Ok(())
}
