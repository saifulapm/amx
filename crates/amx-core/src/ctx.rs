//! The session context: every path and handle passed explicitly.
//!
//! 04 §2: "Session/socket paths live in a `Ctx` struct passed explicitly — no
//! env-var globals, no test mutexes (fixes W9)." The rule has teeth only if the
//! environment is also a value, so [`Env`] is read once at process start and
//! then threaded; nothing below this line calls `std::env::var`.

use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::error::{CtxError, SessionNameError};
use crate::event::Bus;

/// Longest permitted session name, in bytes.
pub const MAX_SESSION_NAME: usize = 64;

/// The directory amx owns under every root it is given.
///
/// Both roots are shared with every other program on the system, so neither
/// the runtime root nor the state root is ever written to directly: a session
/// lives at `<root>/amx/<session>`.
pub const AMX_DIR: &str = "amx";

/// The socket's file name inside a session's runtime directory.
///
/// One socket per session, not two, not three (04 §1) — so the name is a
/// constant rather than a parameter, and a session directory that holds
/// anything else holds it beside this.
pub const SOCKET_NAME: &str = "sock";

/// The name of a session, validated to be usable as a directory component.
///
/// A session name selects a server instance: `--session work` or `AMX_SESSION`
/// pick the socket and state directory (04 §1). Validation happens here, once,
/// because the name is concatenated into filesystem paths.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct SessionName(String);

impl SessionName {
    /// The session used when none is named.
    pub const DEFAULT: &'static str = "default";

    /// Validate `name` as a session name.
    ///
    /// Rejects the empty string, `.`, `..`, anything containing `/` or a NUL
    /// byte, and anything over [`MAX_SESSION_NAME`] bytes.
    pub fn new(name: impl Into<String>) -> Result<Self, SessionNameError> {
        let name = name.into();
        if name.is_empty() {
            return Err(SessionNameError::Empty);
        }
        if name.len() > MAX_SESSION_NAME {
            return Err(SessionNameError::TooLong {
                len: name.len(),
                max: MAX_SESSION_NAME,
            });
        }
        if name == "." || name == ".." || name.contains(['/', '\\', '\0']) {
            return Err(SessionNameError::NotAComponent { name });
        }
        Ok(Self(name))
    }

    /// The name as a string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for SessionName {
    fn default() -> Self {
        Self(Self::DEFAULT.to_owned())
    }
}

impl fmt::Display for SessionName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for SessionName {
    type Err = SessionNameError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

impl TryFrom<String> for SessionName {
    type Error = SessionNameError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<SessionName> for String {
    fn from(value: SessionName) -> Self {
        value.0
    }
}

/// The process environment, read once and then passed as a value.
///
/// Tests construct one pointing at a tempdir and need no mutex: two sessions in
/// one test process are two `Env` values, not two mutations of a global.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Env {
    /// `$XDG_RUNTIME_DIR` — where the socket lives.
    pub runtime_dir: Option<PathBuf>,
    /// `$XDG_STATE_HOME` — where snapshots live.
    pub state_home: Option<PathBuf>,
    /// `$HOME`, the fallback root for `state_home`.
    pub home: Option<PathBuf>,
    /// `$AMX_SESSION`, the session selected without a `--session` flag.
    pub session: Option<SessionName>,
    /// `$TMPDIR`, the last-resort root when there is no runtime directory.
    pub tmpdir: Option<PathBuf>,
}

impl Env {
    /// Read the environment.
    ///
    /// Call this exactly once, at process start, and pass the result down. It
    /// is the only function in the crate that reads process state.
    ///
    /// An empty variable counts as unset — an exported-but-empty
    /// `XDG_RUNTIME_DIR` is a shell artifact, not a directory named `""` — and
    /// an `AMX_SESSION` that is not a valid [`SessionName`] is dropped rather
    /// than carried as an unusable value, since nothing downstream could do
    /// anything with it but fail.
    #[must_use]
    pub fn from_process() -> Self {
        Self {
            runtime_dir: read_path("XDG_RUNTIME_DIR"),
            state_home: read_path("XDG_STATE_HOME"),
            home: read_path("HOME"),
            session: std::env::var("AMX_SESSION")
                .ok()
                .and_then(|name| SessionName::new(name).ok()),
            tmpdir: read_path("TMPDIR"),
        }
    }
}

/// One environment variable as a path, treating unset and empty alike.
fn read_path(key: &str) -> Option<PathBuf> {
    std::env::var_os(key)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// Everything a task needs to find the session it belongs to.
///
/// Cloneable and cheap: the bus is shared behind an `Arc` and the cancellation
/// token is a handle, so passing a `Ctx` into a spawned task is the normal way
/// to give it both its paths and its shutdown signal. 04 §2's structured
/// shutdown ("nothing detached, everything joined") depends on that token being
/// reachable everywhere the paths are.
#[derive(Clone, Debug)]
pub struct Ctx {
    /// Which named session this is.
    pub session: SessionName,
    /// `$XDG_RUNTIME_DIR/amx/<session>`.
    pub runtime_dir: PathBuf,
    /// `<runtime_dir>/sock`, created mode 0600 — one socket per session, not
    /// two, not three (04 §1).
    pub socket: PathBuf,
    /// `$XDG_STATE_HOME/amx/<session>` — snapshots and history sidecars.
    pub state_dir: PathBuf,
    /// The session's event bus.
    pub bus: Arc<Bus>,
    /// The session's shutdown signal.
    pub cancel: CancellationToken,
}

impl Ctx {
    /// Derive the context for `session` from `env`.
    ///
    /// Derives the paths, creates a bus with
    /// [`DEFAULT_REPLAY_CAPACITY`](crate::event::bus::DEFAULT_REPLAY_CAPACITY)
    /// and a fresh cancellation token. Paths come from the passed `env` only:
    /// nothing here consults the process environment, which is what makes two
    /// sessions in one test process independent.
    pub fn for_session(session: SessionName, env: &Env) -> Result<Self, CtxError> {
        let runtime_dir = runtime_root(env)?.join(AMX_DIR).join(session.as_str());
        let state_dir = state_root(env)?.join(AMX_DIR).join(session.as_str());
        Ok(Self {
            session,
            socket: runtime_dir.join(SOCKET_NAME),
            runtime_dir,
            state_dir,
            bus: Arc::new(Bus::new(crate::event::bus::DEFAULT_REPLAY_CAPACITY)),
            cancel: CancellationToken::new(),
        })
    }
}

/// The directory a session's socket lives under, before `amx/<session>`.
///
/// `$XDG_RUNTIME_DIR` is the right answer and `$TMPDIR` is the fallback for
/// the systems that do not set one (a bare `ssh` session on macOS, most
/// containers). Both must be absolute: a relative root would resolve against
/// whatever directory the process happened to start in, so two invocations of
/// the same session name would find two different sockets.
fn runtime_root(env: &Env) -> Result<PathBuf, CtxError> {
    let root = env
        .runtime_dir
        .clone()
        .or_else(|| env.tmpdir.clone())
        .ok_or_else(|| CtxError::NoRuntimeDir {
            reason: "neither XDG_RUNTIME_DIR nor TMPDIR is set".to_owned(),
        })?;
    if root.is_relative() {
        return Err(CtxError::NoRuntimeDir {
            reason: format!("{} is not an absolute path", root.display()),
        });
    }
    Ok(root)
}

/// The directory a session's snapshots live under, before `amx/<session>`.
///
/// `$XDG_STATE_HOME`, or the `$HOME/.local/state` the XDG base directory
/// specification defines it to default to.
fn state_root(env: &Env) -> Result<PathBuf, CtxError> {
    let root = env
        .state_home
        .clone()
        .or_else(|| env.home.as_ref().map(|home| home.join(".local/state")))
        .ok_or_else(|| CtxError::NoStateDir {
            reason: "neither XDG_STATE_HOME nor HOME is set".to_owned(),
        })?;
    if root.is_relative() {
        return Err(CtxError::NoStateDir {
            reason: format!("{} is not an absolute path", root.display()),
        });
    }
    Ok(root)
}
