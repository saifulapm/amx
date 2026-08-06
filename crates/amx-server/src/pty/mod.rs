//! The pty actor: one thread per pane, owning the master descriptor.
//!
//! 04 §3 fixes the shape. The thread that reads the terminal is the thread that
//! owns it; everyone else speaks to it through [`PtyActorHandle`], which queues
//! work and writes a byte to a wake pipe. The thread parks in one `poll()` over
//! the master and that pipe, with an idle timeout that is a fallback for a
//! missed wake and nothing more.
//!
//! The ordering guarantee lives here: a `response_order` lock is held across
//! the read callback, so a terminal reply produced by parsing a read is queued
//! before any out-of-band reply another thread hands in afterwards. Without it
//! a substituted OSC 10/11 answer could overtake a device-attributes answer the
//! parser produced first, and the child would read replies in an order it never
//! asked for.

mod handle;
mod runner;

use std::collections::VecDeque;
use std::os::fd::AsFd;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use amx_core::platform::PtySession;
use bytes::Bytes;

pub use handle::{PtyActorError, PtyActorHandle};
use handle::{SharedControls, State};
use runner::Runner;

use crate::platform::fd::wake_pipe;

/// How long the actor parks in `poll()` when nothing is ready.
///
/// Handle methods wake the actor explicitly, so this only bounds the damage
/// from a wake that never arrived.
const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(1);

/// How long a quiesce may spend draining queued writes before it gives up.
const DEFAULT_QUIESCE_TIMEOUT: Duration = Duration::from_secs(1);

/// How much output one read takes off the terminal.
const DEFAULT_READ_BUFFER: usize = 8192;

/// How many input chunks may be queued before a writer has to wait.
///
/// The queue is a buffer, not a backlog: a client that outruns its pty is
/// pushed back on rather than allowed to grow the server's memory.
const INPUT_QUEUE: usize = 1024;

/// What the parser does with a chunk of terminal output.
///
/// The replies it produces are pushed into the caller-supplied buffer in the
/// order the terminal asked for them; the buffer is reused across reads, so a
/// read that answers nothing allocates nothing.
pub type ReadCallback = Box<dyn FnMut(&[u8], &mut Vec<Bytes>) + Send + 'static>;

/// What runs once, on the way out, with whatever became of the child.
pub type ExitCallback = Box<dyn FnOnce(ChildExit) + Send + 'static>;

/// How the child ended.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ChildExit {
    /// It exited with this status code.
    Code(i32),
    /// It ended without a normal exit status: it was signalled.
    Signalled,
    /// The actor stopped for its own reasons, or the status could not be
    /// collected — the child may well still be running.
    Unknown,
}

/// Everything one pty actor needs to start.
pub struct PtyActorConfig<S> {
    /// The pty this actor owns for its whole life.
    pub session: S,
    /// Thread name, which is what shows up in a backtrace or in `top`.
    pub name: String,
    /// The `poll()` fallback timeout.
    pub idle_timeout: Duration,
    /// How long [`PtyActorHandle::quiesce`] may drain for.
    pub quiesce_timeout: Duration,
    /// Read buffer size.
    pub read_buffer: usize,
    /// The parser.
    pub on_read: ReadCallback,
    /// Called once when the actor stops.
    pub on_exit: Option<ExitCallback>,
}

impl<S> std::fmt::Debug for PtyActorConfig<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PtyActorConfig")
            .field("name", &self.name)
            .field("idle_timeout", &self.idle_timeout)
            .field("quiesce_timeout", &self.quiesce_timeout)
            .field("read_buffer", &self.read_buffer)
            .finish_non_exhaustive()
    }
}

impl<S> PtyActorConfig<S> {
    /// A config with the defaults, for a session and a parser.
    pub fn new(session: S, on_read: ReadCallback) -> Self {
        Self {
            session,
            name: "amx-pty".to_owned(),
            idle_timeout: DEFAULT_IDLE_TIMEOUT,
            quiesce_timeout: DEFAULT_QUIESCE_TIMEOUT,
            read_buffer: DEFAULT_READ_BUFFER,
            on_read,
            on_exit: None,
        }
    }
}

/// Starting pty actors.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct PtyActor;

impl PtyActor {
    /// Start the actor on its own thread.
    ///
    /// The join handle is returned rather than detached: 04 §2 wants nothing
    /// detached and everything joined, and the caller is the one that knows
    /// which supervisor the thread belongs to.
    pub fn spawn<S>(
        config: PtyActorConfig<S>,
    ) -> Result<(PtyActorHandle, JoinHandle<()>), PtyActorError>
    where
        S: PtySession + AsFd + Send + 'static,
    {
        let PtyActorConfig {
            session,
            name,
            idle_timeout,
            quiesce_timeout,
            read_buffer,
            on_read,
            on_exit,
        } = config;

        let pipe = wake_pipe()?;
        let (input_tx, input_rx) = tokio::sync::mpsc::channel(INPUT_QUEUE);
        let (control_tx, control_rx) = std::sync::mpsc::channel();
        let controls = Arc::new(Mutex::new(SharedControls::default()));
        let response_order = Arc::new(Mutex::new(()));
        let accepting = Arc::new(AtomicBool::new(true));

        let handle = PtyActorHandle::new(
            input_tx,
            control_tx,
            pipe.writer,
            Arc::clone(&controls),
            Arc::clone(&response_order),
            Arc::clone(&accepting),
            quiesce_timeout,
        );

        let mut runner = Runner {
            session,
            state: State::Running,
            input_rx,
            control_rx,
            wake: pipe.reader,
            pending: VecDeque::new(),
            offset: 0,
            controls,
            response_order,
            accepting,
            on_read,
            on_exit,
            idle_timeout,
            quiesce_timeout,
            read_buf: vec![0; read_buffer.max(1)],
            responses: Vec::new(),
        };
        let thread = std::thread::Builder::new()
            .name(name)
            .spawn(move || runner.run())?;

        Ok((handle, thread))
    }
}
