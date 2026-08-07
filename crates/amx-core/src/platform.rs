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

/// The variables amx sets in every pane child's environment (D-M2-4).
///
/// Named here, beside [`PtyCommand`], because both ends of the identity chain
/// read the same list: the server writes them at spawn, and `amx _hook` reads
/// them back out of the environment a hook process inherited. A name spelled
/// twice is a name that can be spelled differently twice.
///
/// V01 §3 M6 measured the inheritance the scheme rides on. The driver planted
/// these six in an interactive *shell*, typed `claude` at it by hand, and all
/// 185 hook invocations in the run carried all six verbatim — hook process,
/// agent, shell, in that order up the `/proc` ancestry. So a hand-typed agent
/// is attributed exactly like one `agent start` launched, and the degraded
/// "identity by `session_id` plus process tree" branch the plan held in reserve
/// is not needed.
pub mod pane_env {
    /// `AMX_ENV=1` — set in a pane child and nowhere else.
    ///
    /// The gate an agent skill checks before offering amx's verbs at all: its
    /// presence is what distinguishes "running inside a pane" from "running in
    /// the user's own terminal", and it is deliberately a marker rather than
    /// data so nothing is tempted to parse it.
    pub const MARKER: &str = "AMX_ENV";

    /// The only value [`MARKER`] is ever set to.
    pub const MARKER_VALUE: &str = "1";

    /// `AMX_SESSION` — which named session this pane belongs to.
    ///
    /// The same variable a user sets to pick a session (`Env::session`), which
    /// is the point: an `amx` command typed inside a pane addresses the server
    /// that pane lives in, with no flag.
    pub const SESSION: &str = "AMX_SESSION";

    /// `AMX_SOCKET` — the absolute path of that session's socket.
    ///
    /// Carried explicitly rather than re-derived from `$XDG_RUNTIME_DIR`,
    /// because a hook process inherits the *terminal's* environment and a
    /// terminal's runtime dir may not be the server's.
    pub const SOCKET: &str = "AMX_SOCKET";

    /// `AMX_PANE_ID` — this pane's UUID.
    pub const PANE: &str = "AMX_PANE_ID";

    /// `AMX_WORKSPACE_ID` — the UUID of the workspace holding this pane.
    ///
    /// The pane's workspace *at spawn*. A pane moved between workspaces keeps
    /// the value it started with; nothing reads it as authority (the server
    /// resolves a pane's workspace from its own state), it is there so a script
    /// inside a pane can name its neighbours.
    pub const WORKSPACE: &str = "AMX_WORKSPACE_ID";

    /// `AMX_HOOK_TOKEN` — the per-spawn value a hook report must carry back.
    ///
    /// See [`HookToken`](crate::agent::HookToken): a misattribution guard, not
    /// a security boundary.
    pub const TOKEN: &str = "AMX_HOOK_TOKEN";

    /// Every name above, for a caller that wants to clear or inspect the set.
    pub const ALL: [&str; 6] = [MARKER, SESSION, SOCKET, PANE, WORKSPACE, TOKEN];
}

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

    /// The argument vector a process was started with.
    ///
    /// In kernel order, `argv[0]` first, with nothing unquoted or re-split:
    /// argv is data (D-M2-7), and this is the reading half of that promise.
    /// Elements are [`OsString`] because a Unix argv is bytes — a token that is
    /// not UTF-8 is still a token, and dropping it would silently shorten the
    /// vector the identification walk reasons about.
    ///
    /// An empty vector is a legitimate `Ok`: a zombie and a kernel thread both
    /// have no argv, and "this process has no argv" is an answer. A process
    /// that exited between the call and the read, or one this user may not
    /// inspect, is [`NotFound`](PlatformError::NotFound) — the caller's defined
    /// fallback is [`exe`](Self::exe), then giving up.
    fn argv(&self, process: ProcessId) -> Result<Vec<OsString>, PlatformError>;

    /// The path of the executable a process is running.
    ///
    /// The corroborating half of [`argv`](Self::argv), and the reason both are
    /// on the seam: `argv[0]` is whatever the parent chose to write there and
    /// can say anything, while this is the file the kernel actually mapped.
    /// Read when argv is unreadable or empty.
    fn exe(&self, process: ProcessId) -> Result<PathBuf, PlatformError>;

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
