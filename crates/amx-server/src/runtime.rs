//! Root supervisor: structured shutdown over a `JoinSet` (04 §2).
//!
//! "The server is a set of tokio actors with typed mailboxes, supervised by a
//! root task with `CancellationToken` + `JoinSet` (structured shutdown;
//! nothing detached, everything joined)." `Runtime` is that root task's state:
//! every actor is spawned through [`Runtime::spawn`], and there is no other
//! path to `tokio::spawn` in this crate — a task spawned any other way is
//! exactly W9's "detached threads never joined" reintroduced by hand.

use std::future::Future;

use amx_core::Ctx;
use tokio::task::JoinSet;

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
}

impl Runtime {
    /// A runtime with no tasks yet, over `ctx`.
    #[must_use]
    pub fn new(ctx: Ctx) -> Self {
        Self {
            ctx,
            tasks: JoinSet::new(),
        }
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

    /// Spawn a task under this runtime's `JoinSet`.
    ///
    /// The task is expected to select on `ctx().cancel.cancelled()` and
    /// return once it fires. `Runtime` only tracks membership and joins; it
    /// does not itself force a task to stop, so a task that ignores the
    /// signal makes [`Runtime::shutdown`] hang rather than leaking silently —
    /// visible, not swallowed.
    pub fn spawn<F>(&mut self, task: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.tasks.spawn(task);
    }

    /// Cancel every task and wait for all of them to finish.
    ///
    /// Consumes the runtime: nothing may be spawned once shutdown has begun.
    /// Returns only once the `JoinSet` reports empty, which is the "nothing
    /// detached, everything joined" guarantee made a runtime property instead
    /// of a convention.
    pub async fn shutdown(mut self) -> ShutdownReport {
        self.ctx.cancel.cancel();
        let mut report = ShutdownReport::default();
        while let Some(result) = self.tasks.join_next().await {
            report.joined += 1;
            if result.is_err() {
                report.panicked += 1;
            }
        }
        report
    }
}
