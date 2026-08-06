//! Typed errors for the core vocabulary.

use std::path::PathBuf;

use thiserror::Error;

/// A string that was expected to be one of the UUID ids was not.
#[derive(Debug, Error)]
#[error("invalid {kind}: {source}")]
pub struct IdParseError {
    /// The id type that was being parsed, e.g. `PaneId`.
    pub kind: &'static str,
    /// The underlying UUID parse failure.
    #[source]
    pub source: uuid::Error,
}

/// A session name was rejected.
///
/// Session names become directory components under `$XDG_RUNTIME_DIR/amx/` and
/// `$XDG_STATE_HOME/amx/`, so they are validated at construction rather than at
/// use: a name that cannot be a directory component is never allowed to exist.
#[derive(Debug, Error)]
pub enum SessionNameError {
    /// The name was empty.
    #[error("session name is empty")]
    Empty,
    /// The name contained a path separator, `.`, `..`, or a NUL byte.
    #[error("session name {name:?} is not a valid directory component")]
    NotAComponent {
        /// The rejected name.
        name: String,
    },
    /// The name was longer than the platform allows for a directory component.
    #[error("session name is {len} bytes, over the {max} byte limit")]
    TooLong {
        /// Length of the rejected name.
        len: usize,
        /// The limit.
        max: usize,
    },
}

/// Building a [`Ctx`](crate::ctx::Ctx) failed.
///
/// Every variant names the directory or variable at fault: paths come from the
/// explicitly passed [`Env`](crate::ctx::Env), never from process globals
/// (04 §2), so a failure here is always attributable.
#[derive(Debug, Error)]
pub enum CtxError {
    /// Neither `XDG_RUNTIME_DIR` nor a usable fallback was available.
    #[error("no runtime directory: {reason}")]
    NoRuntimeDir {
        /// Why the environment could not supply one.
        reason: String,
    },
    /// Neither `XDG_STATE_HOME` nor a usable fallback was available.
    #[error("no state directory: {reason}")]
    NoStateDir {
        /// Why the environment could not supply one.
        reason: String,
    },
    /// The session name was rejected.
    #[error(transparent)]
    SessionName(#[from] SessionNameError),
    /// A directory could not be created or inspected.
    #[error("{}: {source}", path.display())]
    Io {
        /// The path being created or inspected.
        path: PathBuf,
        /// The underlying failure.
        #[source]
        source: std::io::Error,
    },
}
