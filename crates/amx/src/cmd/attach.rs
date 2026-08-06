//! `amx` and `amx attach`: probe, daemonize if absent, attach (04 §1).
//!
//! The three steps are one function, [`ensure_running`], because they are one
//! decision: "is this session running, and if not, start it". A socket that
//! answers means attach; a socket nothing answers on is a file that outlived
//! its process and is removed; no socket at all and a removed one lead to the
//! same place, a detached `amx server` and a poll until it answers.
//!
//! [`full`] is the ordinary client — chrome, layout, every visible pane — and
//! [`crate::cmd::viewport`] is the degenerate one-pane form of it.
//!
//! **What the loop here does not do yet.** It reads stdin only to recognise the
//! detach chord (see [`crate::cmd::detach`] for why that one sequence cannot
//! wait) and forwards nothing to panes: `App::handle_input` is T14's `todo!()`,
//! and the raw pane I/O stream it would write to has no encoding yet
//! (`amx_proto::stream::raw` is `todo!()` too). When T14 lands, the stdin arm
//! calls `App::handle_input` and this file's chord goes away.

use std::process::ExitCode;

use amx_client::app::{App, RESIZE_DEBOUNCE};
use amx_client::term::{self, Sigwinch};
use amx_core::{Ctx, PaneId};
use amx_proto::ClientInfo;
use amx_server::session::daemon::{self, READY_TIMEOUT};
use amx_server::session::probe::clear_if_stale;
use anyhow::Context as _;
use clap::ArgMatches;
use tokio::io::AsyncReadExt as _;

use crate::cmd::detach::{Chord, DETACH};

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
    daemon::spawn_detached(&exe, &args).context("start the session server")?;

    // The spawn may lose the bind race with another `amx` that started at the
    // same moment; that is not a failure, because what this waits for is a
    // server answering, not *this* server answering.
    daemon::await_ready(&ctx.socket, READY_TIMEOUT).await?;
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

/// The ordinary client: chrome, layout, every visible pane.
async fn full(ctx: &Ctx) -> anyhow::Result<ExitCode> {
    let mut app = App::attach(
        &ctx.socket,
        std::io::stdin(),
        std::io::stdout(),
        client_info(),
    )
    .await
    .context("attach to the session")?;

    let mut out = std::io::stdout();
    let mut sigwinch = Sigwinch::install().context("watch for terminal resizes")?;
    let mut stdin = tokio::io::stdin();
    let mut chord = Chord::new(DETACH);
    let mut input = [0_u8; 1024];

    app.repaint();
    flush(&mut out, app.frame())?;

    loop {
        tokio::select! {
            read = stdin.read(&mut input) => {
                // End of input is a detach too: whatever was driving this
                // terminal has gone, and the panes are not its to take along.
                let Ok(n @ 1..) = read else { break };
                if chord.feed(&input[..n]) {
                    break;
                }
            }
            signal = sigwinch.recv() => {
                let Some(()) = signal else { break };
                let size = term::window_size(std::io::stdin())
                    .context("read the terminal size")?;
                app.note_resize(size);
            }
            () = tokio::time::sleep(RESIZE_DEBOUNCE), if app.has_pending_resize() => {
                app.settle_resize(&mut |_size| {});
                flush(&mut out, app.frame())?;
            }
        }
    }

    // Dropping the app restores the terminal: raw mode off, alt screen left,
    // exactly as the guard found it. The session keeps running.
    drop(app);
    Ok(ExitCode::SUCCESS)
}

/// Write one frame to the terminal.
pub fn flush(out: &mut impl std::io::Write, frame: &[u8]) -> anyhow::Result<()> {
    out.write_all(frame).context("write to the terminal")?;
    out.flush().context("flush the terminal")?;
    Ok(())
}
