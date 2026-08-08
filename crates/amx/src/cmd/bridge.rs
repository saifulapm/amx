//! `amx _bridge` — the byte splice behind an SSH remote (D-M3-9).
//!
//! A splice and nothing more: resolve the session, connect to its socket, copy
//! bytes both ways, and exit with the connect error *before the first protocol
//! byte* if there is one. Nothing here parses a frame, and nothing here knows
//! what a method is — the protocol is the client's and the server's business,
//! and this process is the pipe between them.
//!
//! That is what keeps the remote story free: the local side hands one end of a
//! socketpair to the ssh child as stdin+stdout and gives the other to
//! [`amx_client::net::Session::attach`], which takes a `UnixStream` and never
//! learns that its peer is a subprocess. No transport trait, no client edit,
//! and version negotiation happens over the wire exactly as it does locally —
//! which is how a remote inside the N/N−1 window works with no reinstall
//! (`crate::remote` is the other half).
//!
//! # Half-close, and why the reader is the one that ends the process
//!
//! The two directions do not end together. Stdin closing means the ssh channel
//! went away — the local client detached or died — and the right answer is to
//! *shut down the write half only*: the server sees the end-of-input it needs
//! to retire the connection, while replies already in flight still have
//! somewhere to land. So the socket→stdout copy is the one this function
//! awaits, and it returns when the server closes, which is when a bridged
//! session is genuinely over.

use std::process::ExitCode;

use amx_core::Env;
use anyhow::Context as _;
use clap::ArgMatches;
use tokio::io::AsyncWriteExt as _;
use tokio::net::UnixStream;

/// Run `amx _bridge`.
///
/// # Errors
///
/// If the session cannot be resolved, if `--daemonize` was asked for and no
/// server could be started, if nothing is listening on the session socket, or
/// if the splice itself fails on i/o.
pub async fn run(env: &Env, root: &ArgMatches, sub: &ArgMatches) -> anyhow::Result<ExitCode> {
    let ctx = crate::ctx_of(env, root, None)?;

    // Auto-detect-or-daemonize, the same decision `amx` makes for a local
    // attach — asked for explicitly here, because a one-shot verb bridged over
    // ssh must not silently start a session the caller did not ask for.
    if sub.get_flag("daemonize") {
        crate::cmd::attach::ensure_running(&ctx)
            .await
            .context("start the session server on this host")?;
    }

    let stream = amx_client::net::connect(&ctx.socket)
        .await
        .with_context(|| format!("connect to session {}", ctx.session))?;

    splice(stream).await?;
    Ok(ExitCode::SUCCESS)
}

/// Copy bytes between this process's stdio and `stream` until the session ends.
async fn splice(stream: UnixStream) -> anyhow::Result<()> {
    let (mut from_session, mut to_session) = stream.into_split();

    // The stdin half runs beside the reader rather than in a `select!` arm:
    // its end is not the session's end, and folding the two would make one
    // hang up the other. See the module header.
    let upstream = tokio::spawn(async move {
        let mut stdin = tokio::io::stdin();
        let copied = tokio::io::copy(&mut stdin, &mut to_session).await;
        let _ = to_session.shutdown().await;
        copied
    });

    let mut stdout = tokio::io::stdout();
    let served = tokio::io::copy(&mut from_session, &mut stdout).await;
    // Whatever the outcome, what has been read is owed to the terminal on the
    // far side before this process goes away.
    let _ = stdout.flush().await;
    // A blocking stdin read that nobody will answer is not worth waiting on.
    upstream.abort();

    served.context("splice the session onto stdio")?;
    Ok(())
}
