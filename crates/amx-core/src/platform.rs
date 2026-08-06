//! The platform seam (04 §9).
//!
//! "One `platform` trait seam (`Pty`, `Ipc`, `ProcessTree`) with the Unix
//! implementation first; a future Windows port implements traits and runs the
//! same conformance suite (fixes W11's approach, defers its cost)."
//!
//! The seam is deliberately narrow and deliberately *not* async: it wraps the
//! three places where amx touches the operating system, and the actor that owns
//! each resource decides how to drive it. Nothing here mentions a file
//! descriptor, a handle, or a syscall, because that is exactly what a Windows
//! port would have to redefine.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use thiserror::Error;

/// A window size in character cells.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct WinSize {
    /// Rows.
    pub rows: u16,
    /// Columns.
    pub cols: u16,
}

/// An operating-system process identifier.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct ProcessId(pub u32);

/// What to run on a freshly opened pty.
#[derive(Clone, Debug)]
pub struct PtyCommand {
    /// The program to execute.
    pub program: OsString,
    /// Arguments, not including the program name.
    pub args: Vec<OsString>,
    /// Environment entries to set on top of the inherited environment.
    pub env: Vec<(OsString, OsString)>,
    /// Working directory for the child.
    ///
    /// A split inherits the *foreground process* cwd of the source pane where
    /// one is readable (04 §7), which is what this field carries.
    pub cwd: Option<PathBuf>,
    /// Initial grid size.
    pub size: WinSize,
}

/// Opening pseudo-terminals and spawning children on them.
pub trait Pty: Send + Sync + 'static {
    /// The platform's pty session type.
    type Session: PtySession;

    /// Open a pty and spawn `command` on its slave side.
    ///
    /// The implementation owns the invariant that exactly one parent-side
    /// handle survives the spawn: the child gets the slave as its controlling
    /// terminal and the parent keeps the master, with no stray inherited
    /// descriptors on either side.
    fn spawn(&self, command: &PtyCommand) -> Result<Self::Session, PlatformError>;
}

/// One open pty with a child on the far end.
///
/// The methods are non-blocking and partial by design: `write` reports how many
/// bytes it took so the caller can resume at the correct offset, and `read`
/// reports `WouldBlock` rather than parking a thread that also has to service a
/// wake-up channel.
pub trait PtySession: Send {
    /// Read available output; `Ok(0)` means the child closed the terminal.
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, PlatformError>;

    /// Write input, returning how much was accepted.
    fn write(&mut self, buf: &[u8]) -> Result<usize, PlatformError>;

    /// Resize the terminal, which notifies the child.
    fn resize(&mut self, size: WinSize) -> Result<(), PlatformError>;

    /// The child process this session spawned.
    fn child(&self) -> ProcessId;

    /// The process group currently in the foreground of the terminal.
    ///
    /// This is what "the process the user is actually looking at" means for cwd
    /// inheritance; it lives on the session because only the session holds the
    /// terminal handle the answer comes from.
    fn foreground_group(&self) -> Result<ProcessId, PlatformError>;

    /// Collect the child's exit status, or `None` if it is still running.
    ///
    /// `Some(None)` is a child that ended without a normal exit status — it was
    /// signalled.
    fn try_wait(&mut self) -> Result<Option<Option<i32>>, PlatformError>;

    /// Ask the child to terminate.
    fn kill(&mut self) -> Result<(), PlatformError>;
}

/// The session socket.
///
/// 04 §1: one socket per session at `$XDG_RUNTIME_DIR/amx/<session>/sock`, mode
/// 0600 — and stale-socket disambiguation is by *connect probe*, not a lock
/// file, which is why [`probe`](Ipc::probe) is part of the seam rather than
/// something the caller improvises.
pub trait Ipc: Send + Sync + 'static {
    /// The platform's listener type.
    type Listener;

    /// Bind a listener at `path`, restricted to the current user.
    ///
    /// Implementations create the socket with mode 0600 (or the platform's
    /// equivalent) atomically — never bind-then-chmod, which is a window in
    /// which another user can connect.
    fn bind(&self, path: &Path) -> Result<Self::Listener, PlatformError>;

    /// Ask whether a server is listening at `path`.
    ///
    /// A socket file that nothing answers on is stale and may be replaced; a
    /// socket that answers means a server is already running and this process
    /// must not become one.
    fn probe(&self, path: &Path) -> Result<Probe, PlatformError>;
}

/// The result of a connect probe against a socket path.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Probe {
    /// Nothing is at that path.
    Absent,
    /// A socket file exists but nothing accepted the connection: it is stale.
    Stale,
    /// A server accepted the connection.
    Listening,
}

/// Reading the process tree behind a pane.
pub trait ProcessTree: Send + Sync + 'static {
    /// The working directory of a process.
    ///
    /// Used for "split and land in the same directory" (04 §7). Callers have a
    /// defined fallback — the pane's own cwd — for when this is unreadable,
    /// because it routinely is: the process may exit between the two calls, and
    /// on some platforms it is simply not exposed.
    fn cwd(&self, process: ProcessId) -> Result<PathBuf, PlatformError>;

    /// The direct children of a process.
    fn children(&self, process: ProcessId) -> Result<Vec<ProcessId>, PlatformError>;

    /// Whether the process still exists.
    fn is_alive(&self, process: ProcessId) -> bool;
}

/// A platform operation failed.
#[derive(Debug, Error)]
pub enum PlatformError {
    /// The operation would block; retry when the resource is ready.
    #[error("would block")]
    WouldBlock,
    /// The process, path or terminal is gone.
    #[error("not found")]
    NotFound,
    /// The platform does not offer this operation.
    #[error("unsupported on this platform: {0}")]
    Unsupported(&'static str),
    /// Anything the operating system reported directly.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
