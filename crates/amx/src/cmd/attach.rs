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
use amx_client::net::{self, Session};
use amx_client::term::{self, Sigwinch};
use amx_core::{Ctx, PaneId, ShortNumber};
use amx_proto::ClientInfo;
use amx_proto::control::session;
use amx_server::session::daemon::{self, READY_TIMEOUT};
use amx_server::session::probe::clear_if_stale;
use anyhow::Context as _;
use clap::ArgMatches;
use serde_json::json;

/// What `--pane` named, before there is a session to ask.
///
/// A UUID needs nobody: it *is* the pane's identity. A short number is the
/// display mapping of 04 §6 and only the session it was issued in knows which
/// pane holds it, so the two arrive here as two cases and are collapsed by
/// [`resolve_pane`] once the server is up.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PaneRef {
    /// A pane UUID, exactly as `session.state` reports it.
    Id(PaneId),
    /// A user-visible short number, as the picker and the status line show it.
    Short(ShortNumber),
}

impl PaneRef {
    /// Read what the user typed after `--pane`.
    ///
    /// # Errors
    ///
    /// A target that is neither spelling.
    pub fn parse(target: &str) -> anyhow::Result<Self> {
        if let Ok(pane) = target.parse::<PaneId>() {
            return Ok(Self::Id(pane));
        }
        ShortNumber::parse(target)
            .map(Self::Short)
            .with_context(|| {
                format!("--pane wants a pane id or a short number, which {target:?} is not")
            })
    }
}

/// What `amx attach` was asked for.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Options {
    /// Attach to one pane, full-screen and without chrome.
    pub pane: Option<PaneRef>,
    /// Take size authority for that pane.
    pub takeover: bool,
}

impl Options {
    /// Read the options out of `matches`.
    ///
    /// # Errors
    ///
    /// A `--pane` target that is neither a pane id nor a short number.
    pub fn parse(matches: &ArgMatches) -> anyhow::Result<Self> {
        let pane = match matches.get_one::<String>("pane") {
            Some(target) => Some(PaneRef::parse(target)?),
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
        Some(target) => {
            let pane = resolve_pane(ctx, target).await?;
            crate::cmd::viewport::one_pane(ctx, pane, options.takeover).await
        }
        None => full(ctx).await,
    }
}

/// Turn what the user typed into the pane it names.
///
/// A UUID costs nothing and asks nobody, which is the same promise
/// `dispatch/pane.rs` makes server-side. A short number costs one
/// `session.state` — the reply that carries the mapping — and it is only paid
/// by the caller who typed a number.
async fn resolve_pane(ctx: &Ctx, target: PaneRef) -> anyhow::Result<PaneId> {
    let number = match target {
        PaneRef::Id(pane) => return Ok(pane),
        PaneRef::Short(number) => number,
    };
    let socket = net::connect(&ctx.socket)
        .await
        .context("connect to the session")?;
    // A verb connection: it declares no viewport and binds no stream, so
    // asking which pane `2` is does not make this terminal the size authority
    // before the real attach below decides whether to be.
    let (mut session, _welcome) = Session::attach(socket, client_info(), false, None)
        .await
        .context("negotiate with the session")?;
    let state = session
        .call("session.state", json!({}))
        .await
        .context("read the session's state")?;
    let state: session::StateReply =
        serde_json::from_value(state).context("decode the session's state")?;
    state
        .panes
        .iter()
        .find(|pane| pane.short == number)
        .map(|pane| pane.pane)
        .with_context(|| format!("no pane in this session is numbered {number}"))
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
///
/// The configuration is read *before* the terminal is touched, for the reason
/// [`run`] checks the terminal first: everything this function can refuse it
/// refuses while a person can still read the refusal. Nothing here does refuse
/// — a `[keys]` row this build cannot resolve is a diagnostic and the shipped
/// binding (D-M1-8's leniency, one level below where the config module can
/// enforce it) — but the diagnostics are logged here rather than printed,
/// because the next thing this function does is enter the alternate screen.
/// `amx keys` is where a person reads them.
async fn full(ctx: &Ctx) -> anyhow::Result<ExitCode> {
    let (settings, diagnostics) = amx_client::config::load(&ctx.config_path);
    for entry in &diagnostics {
        tracing::warn!(
            section = entry.section.unwrap_or("<file>"),
            error = %entry.message,
            "the config file was not applied whole; run `amx keys` to see what took effect",
        );
    }

    let mut app = App::attach(
        &ctx.socket,
        std::io::stdin(),
        std::io::stdout(),
        client_info(),
    )
    .await
    .context("attach to the session")?;
    app.input().set_bindings(settings.bindings);
    // `[client] narrow_cols`, the D14 threshold. Its reader lands in the same
    // milestone the field did (D-M4-10), and this line is the whole of the
    // path between the two.
    app.set_narrow_cols(settings.narrow_cols);

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

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used, reason = "test")]

    use amx_core::{PaneId, ShortNumber};

    use super::PaneRef;

    #[test]
    fn a_pane_target_is_a_uuid_or_a_short_number_and_nothing_else() {
        let pane = PaneId::new_v4();
        assert_eq!(
            PaneRef::parse(&pane.to_string()).expect("a pane id"),
            PaneRef::Id(pane),
        );
        assert_eq!(
            PaneRef::parse("2").expect("a short number"),
            PaneRef::Short(ShortNumber::new(2)),
        );

        // A label is not a target here. `--pane` is answered before the
        // terminal is touched and the whole point of the mode is that a typo
        // is a refusal rather than a blank screen, so the parse stays a
        // question about the string.
        for bogus in ["not-a-pane", "", "dev2", " 2"] {
            let err = PaneRef::parse(bogus).expect_err("neither spelling");
            assert!(
                err.to_string().contains("pane id or a short number"),
                "{bogus:?}: {err}",
            );
        }
    }
}
