//! Typed errors over `GhosttyResult`.
//!
//! Every fallible C entry point returns the same five-value result code
//! (`include/ghostty/vt/types.h:74`). Mapping it once, here, through an
//! exhaustive match is what keeps the rest of the crate free of raw integers:
//! a re-vendor that renumbers or removes a code breaks this file and nothing
//! else.

use thiserror::Error;

use crate::sys;

/// The result of a libghostty-vt call.
pub type Result<T> = std::result::Result<T, Error>;

/// A libghostty-vt call reported failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum Error {
    /// The library could not allocate.
    #[error("libghostty-vt could not allocate")]
    OutOfMemory,
    /// An argument was NULL, out of range, or not a value the library knows.
    #[error("libghostty-vt rejected an argument as invalid")]
    InvalidValue,
    /// A caller-provided buffer was too small.
    ///
    /// The library reports the size it needs through the same out-parameter it
    /// would have written into; callers that can grow retry rather than
    /// surfacing this.
    #[error("the buffer supplied to libghostty-vt was too small")]
    OutOfSpace,
    /// The value asked for is not set — an unset colour, an empty selection, a
    /// coordinate outside the requested space.
    #[error("libghostty-vt has no value for that query")]
    NoValue,
    /// A result code this build does not know about.
    ///
    /// `vt.h` opens by warning the API is unstable, so a re-vendor really can
    /// introduce one. It is reported rather than absorbed into another variant.
    #[error("libghostty-vt returned unknown result code {0}")]
    Unknown(i32),
}

/// A wrapper enum could not represent a value libghostty-vt produced.
///
/// The wrapper maps `sys::` constants through exhaustive matches (D-M0-2), so
/// this is the shape of "the vendored library grew a variant we do not know":
/// it is data, not a panic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("libghostty-vt produced {value}, which is not a known {enumeration}")]
pub struct UnknownVariant {
    /// The Rust enum that could not represent it.
    pub enumeration: &'static str,
    /// The raw value.
    pub value: i64,
}

/// Turn a `GhosttyResult` into a `Result`.
pub(crate) fn check(result: sys::GhosttyResult::Type) -> Result<()> {
    match result {
        sys::GhosttyResult::GHOSTTY_SUCCESS => Ok(()),
        sys::GhosttyResult::GHOSTTY_OUT_OF_MEMORY => Err(Error::OutOfMemory),
        sys::GhosttyResult::GHOSTTY_INVALID_VALUE => Err(Error::InvalidValue),
        sys::GhosttyResult::GHOSTTY_OUT_OF_SPACE => Err(Error::OutOfSpace),
        sys::GhosttyResult::GHOSTTY_NO_VALUE => Err(Error::NoValue),
        other => Err(Error::Unknown(other)),
    }
}

/// `check`, but `GHOSTTY_NO_VALUE` is an absent value rather than a failure.
///
/// Several getters use `NO_VALUE` for "this colour is unset" or "the cursor is
/// not in the viewport", which is not an error at any call site we have.
pub(crate) fn check_optional(result: sys::GhosttyResult::Type) -> Result<bool> {
    match check(result) {
        Ok(()) => Ok(true),
        Err(Error::NoValue) => Ok(false),
        Err(other) => Err(other),
    }
}
