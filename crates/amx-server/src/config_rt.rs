//! The running configuration: read at startup, re-read when the file changes.
//!
//! `amx-core`'s [`config`](amx_core::config) module is pure — types and a fold
//! from text into a running [`Config`] (D-M1-8). This module is the half that
//! touches the world: it reads [`Ctx::config_path`], holds the result where
//! every actor can see it, re-reads the file when U04's watcher says it moved,
//! and announces each reload on the bus.
//!
//! # How a setting reaches an actor
//!
//! Through a [`tokio::sync::watch`] channel, not a message: an actor holds a
//! receiver and reads the current value at the moment it needs it, so nobody
//! has to be told a setting changed and no actor can miss a change it was not
//! subscribed for. Two consumers exist in M1 — `Core` reads `[terminal] shell`
//! when it spawns a pane, `Persist` reads `[persist] history` when it saves —
//! and both consult the receiver per operation rather than caching, which is
//! what makes "hot" mean hot.
//!
//! Nothing here restarts anything. A reload changes what the *next* pane spawns
//! and what the *next* save writes; a running process is never replaced because
//! its config line changed.
//!
//! # What a reload announces
//!
//! [`Event::ConfigReloaded`] carries the number of sections that kept their
//! running values because the file's version of them did not parse. The
//! diagnostics behind that count name the section and quote the parser, and
//! stay on this side of the bus ([`ConfigRuntime::diagnostics`]) — the wire
//! surface M1 froze (§3 of the plan) has a count, not a list.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use amx_core::config::{self, SECTIONS};
use amx_core::{Bus, Config, ConfigDiagnostic, Ctx, Event};
use tokio::sync::watch;

use crate::platform::watch::ConfigWatcher;

/// What one read of the config file had to complain about.
///
/// Shared rather than cloned: a diagnostic list is read by whoever asks and
/// written once per reload, which is exactly what an `Arc` behind a watch
/// channel is for. Empty means the file applied whole.
pub type Diagnostics = Arc<[ConfigDiagnostic]>;

/// The configuration this server is running on, and the file behind it.
///
/// Cloneable: the watcher task holds one, the serve path keeps one to hand
/// receivers out from. Every clone publishes to the same channels.
#[derive(Clone, Debug)]
pub struct ConfigRuntime {
    /// The file this runtime re-reads — [`Ctx::config_path`], carried as a
    /// value like every other path in the server, so two servers in one test
    /// process read two different files (04 §2).
    path: PathBuf,
    config: watch::Sender<Config>,
    diagnostics: watch::Sender<Diagnostics>,
}

impl ConfigRuntime {
    /// Read `ctx.config_path` once and hold the result.
    ///
    /// A missing file is not an error: an absent file says the same thing an
    /// empty one does — every section is absent, so every section is its
    /// default (D-M1-8). Any other read failure, and any file that does not
    /// parse, leaves the defaults in place and files a diagnostic; a server
    /// that refused to start over a stray character in `config.toml` would be
    /// a worse answer than a server running the documented defaults.
    #[must_use]
    pub fn load(ctx: &Ctx) -> Self {
        let (config, _) = watch::channel(Config::default());
        let (diagnostics, _) = watch::channel(Diagnostics::from(Vec::new()));
        let runtime = Self {
            path: ctx.config_path.clone(),
            config,
            diagnostics,
        };
        runtime.fold(read(&runtime.path));
        runtime.report("the config file was not applied whole");
        runtime
    }

    /// A receiver for the running configuration.
    ///
    /// What an actor is built with. The value is current the moment it is
    /// borrowed, so a consumer that reads per operation needs no subscription
    /// bookkeeping of its own.
    #[must_use]
    pub fn subscribe(&self) -> watch::Receiver<Config> {
        self.config.subscribe()
    }

    /// The configuration in force right now.
    #[must_use]
    pub fn config(&self) -> Config {
        self.config.borrow().clone()
    }

    /// What the last read of the file rejected, in file order.
    ///
    /// The queryable half of a reload: 04 §6's rule that failures are never
    /// log-only applies to configuration for the same reason it applies to
    /// restore — a warning nobody can ask for is a warning nobody reads.
    #[must_use]
    pub fn diagnostics(&self) -> Diagnostics {
        self.diagnostics.borrow().clone()
    }

    /// Watch the config file until `watcher` stops, reloading on every change.
    ///
    /// The task shape [`Runtime::spawn`](crate::runtime::Runtime::spawn) wants:
    /// it returns when the watcher does, and the watcher returns `None` when
    /// the session's cancellation token fires, so nothing here needs its own
    /// shutdown branch.
    pub async fn run(self, bus: Arc<Bus>, mut watcher: ConfigWatcher) {
        while watcher.changed().await.is_some() {
            self.reload(&bus).await;
        }
    }

    /// Re-read the file, publish the result, and say how much was rejected.
    ///
    /// The read runs on the blocking pool: it is one small file, but it is a
    /// file on whatever filesystem the user's home is on, and an async worker
    /// is not the place to find out that it is a stalled network mount.
    pub async fn reload(&self, bus: &Bus) -> u32 {
        let path = self.path.clone();
        let text = match tokio::task::spawn_blocking(move || read(&path)).await {
            Ok(text) => text,
            Err(err) => Err(io::Error::other(err)),
        };
        let rejected = self.fold(text);
        self.report("the config file was reloaded but not applied whole");
        bus.publish(Event::ConfigReloaded {
            rejected_sections: rejected,
        });
        rejected
    }

    /// Fold `text` into the running configuration and publish both halves.
    ///
    /// Returns the number of sections that kept their running values. A file
    /// that does not parse at all rejects every section at once and is counted
    /// that way: a whole-file failure is the case where *nothing* the user
    /// wrote took effect, and an indicator that stayed dark through it would be
    /// the one lie a reload count must not tell.
    fn fold(&self, text: io::Result<String>) -> u32 {
        let current = self.config.borrow().clone();
        let (next, diagnostics) = match text {
            Ok(text) => config::reload(&current, &text),
            Err(err) => (
                current,
                vec![ConfigDiagnostic::whole_file(format!(
                    "{}: {err}",
                    self.path.display()
                ))],
            ),
        };
        let rejected = if diagnostics.iter().any(|entry| entry.section.is_none()) {
            SECTIONS.len()
        } else {
            diagnostics.len()
        };
        self.config.send_replace(next);
        self.diagnostics
            .send_replace(Diagnostics::from(diagnostics));
        u32::try_from(rejected).unwrap_or(u32::MAX)
    }

    /// Log whatever the last fold rejected, one line per diagnostic.
    ///
    /// Logging is the *second* place a diagnostic goes, never the only one —
    /// it is here because a user editing a config file is usually watching a
    /// terminal, not calling an API.
    fn report(&self, headline: &str) {
        for entry in self.diagnostics.borrow().iter() {
            tracing::warn!(
                section = entry.section.unwrap_or("<file>"),
                error = %entry.message,
                "{headline}",
            );
        }
    }
}

/// The shell a pane with no command of its own runs.
///
/// Three sources in order (D-M1-8): `[terminal] shell`, then `$SHELL`, then
/// `/bin/sh`. Configuration wins because it is the one of the three the user
/// stated on purpose; `$SHELL` is the same default every terminal multiplexer
/// uses absent one; `/bin/sh` exists everywhere amx runs.
///
/// An empty value at either of the first two sources counts as unset, the same
/// rule [`amx_core::Env`] applies to the paths it reads: an exported-but-empty
/// variable is a shell artifact, and `shell = ""` in a config file is a
/// half-finished edit, not a request to exec the empty string.
#[must_use]
pub fn shell(config: &Config) -> OsString {
    config
        .terminal
        .shell
        .as_deref()
        .filter(|shell| !shell.is_empty())
        .map(OsString::from)
        .or_else(|| std::env::var_os("SHELL").filter(|shell| !shell.is_empty()))
        .unwrap_or_else(|| OsString::from("/bin/sh"))
}

/// Read the config file, treating an absent one as an empty one.
///
/// The equivalence is the whole reason a fresh machine needs no config file:
/// an empty document has no sections, and a section that is absent is a
/// section at its defaults (D-M1-8). It also makes deleting the file mean
/// exactly what emptying it means, which is what keeps reload idempotent with
/// what is on disk.
fn read(path: &Path) -> io::Result<String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(String::new()),
        Err(err) => Err(err),
    }
}
