//! Root supervisor: structured shutdown over a `JoinSet` (04 §2).
//!
//! "The server is a set of tokio actors with typed mailboxes, supervised by a
//! root task with `CancellationToken` + `JoinSet` (structured shutdown;
//! nothing detached, everything joined)." `Runtime` is that root task's state:
//! every actor is spawned through [`Runtime::spawn`], and there is no other
//! path to `tokio::spawn` in this crate — a task spawned any other way is
//! exactly W9's "detached threads never joined" reintroduced by hand.
//!
//! # Why the tasks have names
//!
//! [`Runtime::shutdown`] cancels and then joins in completion order, so a
//! shutdown that never returns is a shutdown waiting on a task nobody can
//! name. That has been the whole difficulty with the drain wedge (R-M1-2,
//! `docs/09-m3-plan.md` §2): the corpses show a main thread parked in the
//! drain, and nothing anywhere says *which* actor it is parked on. So every
//! spawn carries a static name, the set remembers which names are still
//! outstanding, and a drain that overruns [`CENSUS_INTERVAL`] says what it is
//! still waiting for — repeatedly, so a log also shows whether the set is
//! moving at all.
//!
//! The census goes to two places because the process that needs it most cannot
//! use the first: a daemonized server's stderr is `/dev/null` by construction
//! (`session/daemon.rs`), so a wedge that only logged would be a wedge nobody
//! could read. [`CENSUS_FILE`] under the session's runtime directory is the
//! other copy — best effort, never created recursively, and never a reason for
//! shutdown to fail.

use std::collections::HashMap;
use std::future::Future;
use std::time::{Duration, Instant};

use amx_core::Ctx;
use tokio::task::{Id, JoinSet};

/// How long the drain goes without emptying before it reports what it holds.
///
/// Well above a healthy stop — cancelling a token and joining five actors is
/// milliseconds even on a loaded runner — and well below the patience of any
/// caller that would rather see a diagnosis than a hang.
pub const CENSUS_INTERVAL: Duration = Duration::from_secs(5);

/// Where the census is written, under [`Ctx::runtime_dir`].
///
/// Truncated and rewritten each time, so the file holds the drain's *current*
/// state rather than a history of it: what a wedged process is waiting for now
/// is the question being asked of it. Removed again if the drain does finish,
/// and stamped with the pid either way, so a file found beside a live socket
/// is never mistaken for a report about the process answering it.
pub const CENSUS_FILE: &str = "drain-census";

/// What happened while draining the `JoinSet` at shutdown.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct ShutdownReport {
    /// How many tasks the `JoinSet` yielded before it went empty.
    ///
    /// This is the "no leaks" evidence: it equals the number of tasks spawned
    /// since the last shutdown began, because [`Runtime::shutdown`] does not
    /// return until the set is empty.
    pub joined: usize,
    /// How many of those tasks panicked rather than returning normally.
    pub panicked: usize,
    /// How many times the drain overran [`CENSUS_INTERVAL`] without emptying.
    ///
    /// Zero on every healthy stop. A non-zero count on a server that did
    /// eventually exit is the near miss the wedge is made of, and it is worth
    /// more than the wedge itself because the process is still alive to be
    /// asked about it.
    pub census_reports: usize,
}

impl ShutdownReport {
    /// Whether every joined task returned normally.
    #[must_use]
    pub const fn clean(&self) -> bool {
        self.panicked == 0
    }
}

/// The root supervisor: one [`Ctx`], one `JoinSet`.
///
/// All paths and the cancellation signal come from the `Ctx` passed to
/// [`Runtime::new`] — nothing here reads the process environment, so two
/// `Runtime`s built from two independently constructed `Ctx` values never
/// share state (04 §2, fixes W9).
#[derive(Debug)]
pub struct Runtime {
    ctx: Ctx,
    tasks: JoinSet<()>,
    /// What each task in the set is, by the id the set knows it as. An entry
    /// is removed the moment its task is joined, so this *is* the outstanding
    /// list rather than a log of one.
    names: HashMap<Id, &'static str>,
    census_interval: Duration,
}

impl Runtime {
    /// A runtime with no tasks yet, over `ctx`.
    #[must_use]
    pub fn new(ctx: Ctx) -> Self {
        Self {
            ctx,
            tasks: JoinSet::new(),
            names: HashMap::new(),
            census_interval: CENSUS_INTERVAL,
        }
    }

    /// Change how long the drain stays quiet before reporting what it holds.
    ///
    /// [`CENSUS_INTERVAL`] is the default and the one the server uses; this
    /// exists so a test can prove the census works without spending five
    /// seconds of every CI run doing it (the shape `Core::set_capture_budget`
    /// established).
    pub const fn set_census_interval(&mut self, interval: Duration) {
        self.census_interval = interval;
    }

    /// The session context every spawned task is expected to share.
    #[must_use]
    pub const fn ctx(&self) -> &Ctx {
        &self.ctx
    }

    /// How many tasks the `JoinSet` is currently tracking.
    #[must_use]
    pub fn task_count(&self) -> usize {
        self.tasks.len()
    }

    /// What the `JoinSet` is still tracking, by name, in a stable order.
    ///
    /// The order is sorted rather than spawn order: two censuses of the same
    /// stuck set have to be comparable at a glance, and completion order is
    /// exactly the thing that is not stable here.
    #[must_use]
    pub fn outstanding(&self) -> Vec<&'static str> {
        let mut names: Vec<&'static str> = self.names.values().copied().collect();
        names.sort_unstable();
        names
    }

    /// Spawn a named task under this runtime's `JoinSet`.
    ///
    /// `name` is what a stuck drain will call this task; it costs a `&'static
    /// str` and is the difference between "the drain did not finish" and "the
    /// drain did not finish because `core` has not returned".
    ///
    /// The task is expected to select on `ctx().cancel.cancelled()` and
    /// return once it fires. `Runtime` only tracks membership and joins; it
    /// does not itself force a task to stop, so a task that ignores the
    /// signal makes [`Runtime::shutdown`] hang rather than leaking silently —
    /// visible, not swallowed, and now named as well.
    pub fn spawn<F>(&mut self, name: &'static str, task: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let handle = self.tasks.spawn(task);
        self.names.insert(handle.id(), name);
    }

    /// Cancel every task and wait for all of them to finish.
    ///
    /// Consumes the runtime: nothing may be spawned once shutdown has begun.
    /// Returns only once the `JoinSet` reports empty, which is the "nothing
    /// detached, everything joined" guarantee made a runtime property instead
    /// of a convention.
    ///
    /// Still unbounded, deliberately: bounding it here would turn a wedge into
    /// a silent leak of whatever the unjoined task owns, which is the trade
    /// 04 §2 refuses. What is bounded is the *silence* — every
    /// [`CENSUS_INTERVAL`] without an empty set publishes what is left.
    pub async fn shutdown(mut self) -> ShutdownReport {
        self.ctx.cancel.cancel();
        let mut report = ShutdownReport::default();
        let began = Instant::now();
        loop {
            match tokio::time::timeout(self.census_interval, self.tasks.join_next_with_id()).await {
                Ok(None) => break,
                Ok(Some(Ok((id, ())))) => {
                    self.names.remove(&id);
                    report.joined += 1;
                }
                Ok(Some(Err(err))) => {
                    self.names.remove(&err.id());
                    report.joined += 1;
                    report.panicked += 1;
                }
                Err(_elapsed) => {
                    report.census_reports += 1;
                    self.census(began.elapsed());
                }
            }
        }
        if report.census_reports > 0 {
            // The drain finished after all, so the census on disk is now a
            // false statement about a process that is about to exit.
            let _ = std::fs::remove_file(self.ctx.runtime_dir.join(CENSUS_FILE));
        }
        report
    }

    /// Say what the drain is still waiting for, to the log and to the session
    /// directory.
    fn census(&self, waited: Duration) {
        let outstanding = self.outstanding();
        let listed = outstanding.join(", ");
        tracing::warn!(
            waited_ms = waited.as_millis(),
            remaining = outstanding.len(),
            tasks = %listed,
            "the shutdown drain has not emptied; these tasks have not returned",
        );
        // Best effort in every direction: no directory is created, a failure
        // is not reported, and nothing about the drain depends on the write.
        // A diagnostic that could fail a shutdown would be worse than no
        // diagnostic at all.
        let _ = std::fs::write(
            self.ctx.runtime_dir.join(CENSUS_FILE),
            format!(
                "pid {}\nwaiting {} ms\n{} task(s) not returned: {listed}\n",
                std::process::id(),
                waited.as_millis(),
                outstanding.len(),
            ),
        );
    }
}
