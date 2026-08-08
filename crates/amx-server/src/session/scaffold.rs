//! The scaffolding both assemblies need, in the one place both read it.
//!
//! [`serve`](super::serve) builds a session from disk and [`import`](super::import)
//! builds the same session from a manifest, and two of the things they do are
//! identical because they are not about where the session came from: hearing
//! `SIGTERM`, and watching `config.toml`.
//!
//! W07 spelled both a second time on purpose — `serve.rs` was W06's file that
//! wave, and a live upgrade that silently stopped reloading its configuration
//! or stopped answering signals would have been a capability lost to a merge
//! conflict — and named the fold as a W14 seam. This is that fold. It is worth
//! doing rather than leaving: the two copies were byte-identical, so the next
//! change to either would have been a change to one.

use std::path::PathBuf;
use std::sync::Arc;

use amx_core::Ctx;
use tokio::signal::unix::{Signal, SignalKind, signal};
use tokio_util::sync::CancellationToken;

use crate::config_rt::ConfigRuntime;
use crate::platform::watch::watch_config;
use crate::runtime::Runtime;

/// Install the signal watch, before anything a signal could arrive during.
///
/// **Order is the point.** W01 saw a server killed by `SIGTERM`'s *default*
/// disposition — no drain, no final capture, the socket left behind — because
/// the signal arrived after the socket was bound and before this task existed.
/// Both assemblies bind early (the bind is what claims the session, and losing
/// that race must cost an error rather than a teardown) and both then do work
/// that takes real time: `serve` restores every pane from disk, `import` adopts
/// every descriptor. So this goes in first, and the window closes.
///
/// It costs nothing to install early. The task only cancels a token, every
/// stage of both assemblies already watches that token, and a cancellation that
/// arrives mid-assembly is answered by the ordinary shutdown the moment the
/// assembly finishes.
///
/// **Spawning is not installing, and neither is the runtime the first thing to
/// happen.** Two orderings had to change for the window to actually close:
///
/// - `tokio::signal::unix::signal` registers the process's handler when it is
///   *called*, and a task that calls it in its own body registers nothing until
///   the scheduler first polls that task — which neither assembly gives it a
///   chance to do, because nothing between the spawn and the end of the
///   assembly awaits (`Core::restore_from_disk` is synchronous, and so is the
///   adoption). So [`SignalWatch::install`] is a plain function that returns
///   already-listening streams;
/// - and it runs **before the socket is bound**, because the socket file
///   appearing is what makes this process findable, and a `SIGTERM` that
///   arrives in the microseconds between `UnixListener::bind` creating the file
///   and any later statement is a signal no reordering inside the assembly can
///   catch. Measured: a test that spins on the path's existence and signals
///   immediately kills the server every time until the install moves ahead of
///   the bind.
#[derive(Debug)]
pub struct SignalWatch(Option<(Signal, Signal)>);

impl SignalWatch {
    /// Register `SIGTERM` and `SIGINT` handlers for this process, now.
    ///
    /// Must be called inside a tokio runtime, and should be called before the
    /// process does anything that could make it a signal's target.
    #[must_use]
    pub fn install() -> Self {
        match (
            signal(SignalKind::terminate()),
            signal(SignalKind::interrupt()),
        ) {
            (Ok(terminate), Ok(interrupt)) => Self(Some((terminate, interrupt))),
            (terminate, interrupt) => {
                // Nothing to do but say so: a server that cannot hear `SIGTERM`
                // is still a working server, it just has to be stopped another
                // way.
                let err = terminate.err().or(interrupt.err());
                tracing::error!(error = ?err, "could not install signal handlers");
                Self(None)
            }
        }
    }

    /// Put the watch under the supervisor, cancelling `cancel` when it fires.
    pub fn spawn(self, runtime: &mut Runtime, cancel: CancellationToken) {
        runtime.spawn("signals", watch_signals(self.0, cancel));
    }
}

/// Cancel `cancel` on `SIGTERM` or `SIGINT`, and return when it is cancelled.
///
/// Returning on cancellation is what makes this a [`Runtime`] task like any
/// other: the shutdown that this task may itself have started still joins it.
async fn watch_signals(listening: Option<(Signal, Signal)>, cancel: CancellationToken) {
    let Some((mut terminate, mut interrupt)) = listening else {
        cancel.cancelled().await;
        return;
    };

    tokio::select! {
        _ = terminate.recv() => {
            tracing::info!("SIGTERM: shutting the session down");
            cancel.cancel();
        }
        _ = interrupt.recv() => {
            tracing::info!("SIGINT: shutting the session down");
            cancel.cancel();
        }
        () = cancel.cancelled() => {}
    }
}

/// Put the config watcher under the supervisor, if the watch can be
/// established.
///
/// A watch that cannot be set up (no permission to create the config
/// directory, an inotify instance limit already reached) costs hot reloading
/// and nothing else: the configuration read at startup stays in force for the
/// life of the server, which is what every pre-M1 amx did. Refusing to serve
/// the session over it would trade a working multiplexer for a missing
/// convenience.
pub fn spawn_config_watcher(runtime: &mut Runtime, ctx: &Ctx, config: ConfigRuntime) {
    match watch_config(ctx, ctx.cancel.clone()) {
        Ok(watcher) => {
            let bus = Arc::clone(&ctx.bus);
            runtime.spawn("config-watch", config.run(bus, watcher));
        }
        Err(err) => {
            tracing::warn!(
                path = %ctx.config_path.display(),
                error = %err,
                "config changes will not be picked up until this session restarts",
            );
        }
    }
}

/// Where a restored pane whose saved directory has vanished respawns.
///
/// One of the two places the server reads its own environment rather than a
/// `Ctx` — `$SHELL` for a pane's command is the other — because `$HOME` is a
/// property of the user running the server, not of the session. Everything
/// below this line takes it as a value, so a test degrades into a tempdir.
/// A `$HOME` that is unset or is not a directory falls back to the directory
/// the server itself was started in, and to `/` behind that: restore needs
/// *somewhere* to put a pane, and refusing to restore it would turn a missing
/// directory into a lost pane.
#[must_use]
pub fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|home| home.is_dir())
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("/"))
}
