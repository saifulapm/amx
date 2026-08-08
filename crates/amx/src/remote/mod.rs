//! `amx --remote HOST` — the local half of the SSH bridge (D-M3-9).
//!
//! One sentence describes the whole mechanism: **the local side creates a
//! socketpair, hands one end to an ssh child as stdin+stdout, and gives the
//! other to the ordinary client.** [`amx_client::net::Session::attach`] takes a
//! `UnixStream` and asks it nothing about its provenance, so every byte above
//! this file — the handshake, the version negotiation, the grid streams, the
//! keystrokes — is the same code that runs against a local socket. There is no
//! transport trait and no remote-aware branch in the client, which is why a
//! remote inside the N/N−1 window works with no reinstall and no restart.
//!
//! # Why `--remote` is taken out of argv rather than declared in the tree
//!
//! `--remote` selects *which machine parses the rest of the command line*, and
//! that decision has to be made before a parse happens at all. So [`split`]
//! lifts it off `argv` in `main`, and what is left is handed to the same
//! [`crate::cli`] tree it would have got — the flag is a routing decision, not
//! an argument, and it stays out of the generated surface. It is also what
//! keeps `cli.rs` a single-owner file across M3's wave 4.
//!
//! # The remote command, and shell quoting
//!
//! ssh(1) is explicit that "the arguments will be appended to the command,
//! separated by spaces, before it is sent to the server to be executed" — one
//! string, parsed by the remote login shell. Every argument amx sends is
//! therefore single-quoted here ([`sq`]), because a session name is validated
//! as a *path component* (`amx_core::SessionName`) and a path component may
//! hold spaces, quotes and `$`. Nothing reaches a remote shell unquoted.
//!
//! # What happens when the far side has no amx
//!
//! [`seed`] answers that, honestly and same-platform only: see its module docs.

pub mod seed;
pub mod ssh;

use std::ffi::OsString;
use std::process::ExitCode;

use amx_client::app::App;
use amx_client::net::Session;
use amx_client::term::{self, Sigwinch, TerminalGuard};
use amx_core::{Env, SessionName};
use anyhow::Context as _;

pub use ssh::{Missing, Remote};

/// Take `--remote HOST` off `argv`, leaving the command line clap will parse.
///
/// Accepts both spellings a user reaches for, `--remote host` and
/// `--remote=host`, and refuses a second one rather than picking a winner: two
/// hosts is a typo with no correct reading. Everything after a bare `--` is the
/// caller's data and is copied through untouched.
///
/// # Errors
///
/// If `--remote` names no host, or if it appears twice.
pub fn split<I, T>(argv: I) -> anyhow::Result<(Option<Remote>, Vec<OsString>)>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString>,
{
    const FLAG: &str = "--remote";

    let mut rest = Vec::new();
    let mut host: Option<String> = None;
    let mut args = argv.into_iter().map(Into::into);
    let mut literal = false;

    while let Some(arg) = args.next() {
        let text = arg.to_str().unwrap_or_default();
        if literal || (text != FLAG && !text.starts_with("--remote=")) {
            literal |= text == "--";
            rest.push(arg);
            continue;
        }
        let found = if let Some(inline) = text.strip_prefix("--remote=") {
            inline.to_owned()
        } else {
            let next = args.next().context("--remote wants a host to connect to")?;
            next.to_str()
                .context("--remote's host must be valid UTF-8")?
                .to_owned()
        };
        anyhow::ensure!(!found.is_empty(), "--remote wants a host to connect to");
        anyhow::ensure!(
            host.is_none(),
            "--remote was given twice ({} and {found}); one command reaches one host",
            host.unwrap_or_default(),
        );
        host = Some(found);
    }

    Ok((host.map(Remote::new), rest))
}

/// Attach to `remote`, driving it through an `amx _bridge` on the far side.
///
/// The runtime is built here rather than in [`crate::run`] because this path
/// never enters the local dispatch at all: nothing on this machine is probed,
/// no local server is started, and the only session that matters is the one on
/// the other end of the ssh child.
///
/// # Errors
///
/// If stdin is not a terminal, if the rest of the command line does not parse,
/// if it names a subcommand (see below), if ssh cannot be started, or if the
/// far side has no amx and seeding does not resolve it.
pub fn run(remote: &Remote, argv: Vec<OsString>) -> anyhow::Result<ExitCode> {
    let matches = crate::cli::cli().try_get_matches_from(argv)?;
    let env = Env::from_process();

    // A remote *verb* would need this file to re-implement the one-shot call
    // path over a stream instead of a socket path, and M3 does not ship that.
    // Saying so beats a call that silently ran on the wrong machine.
    if let Some((name, sub)) = matches.subcommand() {
        anyhow::ensure!(
            name == "attach",
            "--remote drives an attached session; `amx {name}` runs on this \
             machine. Run it over ssh yourself, or attach with `amx --remote \
             {}` and drive the session from there.",
            remote.host(),
        );
        // The chrome-free one-pane client reaches its session by path too, and
        // over a bridge there is no path. Refusing beats attaching to the whole
        // session while the user asked for one pane of it.
        anyhow::ensure!(
            sub.get_one::<String>("pane").is_none(),
            "--remote does not carry `--pane` yet; attach to the remote session \
             and zoom the pane there.",
        );
    }

    let session = crate::session_of(&env, &matches, None)?;
    anyhow::ensure!(
        term::window_size(std::io::stdin()).is_ok(),
        "amx attaches a terminal, and stdin is not one"
    );

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("start the tokio runtime")?;
    let outcome = runtime.block_on(attach(remote, &session));
    // The attach loop leaves a blocking stdin read behind by construction; see
    // `crate::run`, which shuts its runtime down the same way and for the same
    // reason.
    runtime.shutdown_background();
    outcome
}

/// Bridge to `session` on `remote` and run the ordinary client over it.
async fn attach(remote: &Remote, session: &SessionName) -> anyhow::Result<ExitCode> {
    let (bridge, local) = remote.open(session)?;

    // The handshake is where a remote with no amx first shows up: the far side
    // exits, the socketpair reaches end of input, and the negotiation fails on
    // a closed connection. Which is why the child is asked *why* before this
    // error is believed at face value.
    let negotiated = Session::attach(local, crate::cmd::attach::client_info(), true, None).await;
    let (session, _welcome) = match negotiated {
        Ok(pair) => pair,
        Err(err) => {
            let cause = anyhow::Error::new(err).context("negotiate with the remote session");
            let missing = bridge.diagnose(&cause).await?;
            return seed::offer(remote, &missing).await;
        }
    };

    // The tail of `App::attach`, spelled out because that constructor takes a
    // socket path and this connection has none. Every step below is the same
    // call in the same order, including the contract the ordering encodes:
    // state as of `seq`, deliveries from `seq + 1`.
    let term = TerminalGuard::enter(std::io::stdin(), std::io::stdout())
        .context("put this terminal into raw mode")?;
    let mut app = App::assemble(session, term).context("size the client to this terminal")?;
    let seq = app.sync_state().await.context("read the session's state")?;
    app.subscribe_events(Some(seq))
        .await
        .context("subscribe to the session's events")?;
    app.report_viewport()
        .await
        .context("declare this terminal's viewport")?;

    let mut out = std::io::stdout();
    let sigwinch = Sigwinch::install().context("watch for terminal resizes")?;
    let stdin = tokio::io::stdin();
    app.run(sigwinch, stdin, |bytes| {
        use std::io::Write as _;
        out.write_all(bytes)?;
        out.flush()
    })
    .await
    .context("run the attached client")?;

    bridge.finish().await;
    Ok(ExitCode::SUCCESS)
}
