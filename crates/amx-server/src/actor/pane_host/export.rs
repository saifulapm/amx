//! What a frozen pane is, and the ways freezing one can be refused.
//!
//! The pane's half of `docs/09-m3-plan.md` D-M3-4: [`super::mailbox`] holds the
//! vocabulary the parser thread speaks, and this file holds the vocabulary the
//! *handoff* speaks to it. Split out for the same reason — the module budget
//! working as a forcing function — and along the same seam: these two types
//! change when a live upgrade learns something new about a pane, not when a
//! pane learns a new trick.
//!
//! The capture itself lives on the parser thread ([`super::parser`]), because
//! everything it reads does.

use std::os::fd::OwnedFd;

use thiserror::Error;

use crate::handoff::manifest::{ManifestError, PaneManifest};
use crate::pty::PtyActorError;

/// One pane, frozen: what it was, and the terminal it was on.
///
/// The two halves travel differently and deliberately so — the entry goes into
/// the manifest's JSON line, the descriptor goes over SCM_RIGHTS in a message
/// of its own (§3 step 8) — but they are produced together, in one command, on
/// the thread that owns both answers. Splitting the two would mean asking the
/// pty actor for its state twice and hoping it had not changed in between.
#[derive(Debug)]
pub struct PaneExport {
    /// Everything about the pane that is bytes.
    pub manifest: PaneManifest,
    /// A duplicate of the pty master, the same open file description the
    /// exporter is still holding (unix(7)).
    pub master: OwnedFd,
}

/// A pane could not be frozen.
#[derive(Debug, Error)]
pub enum ExportError {
    /// The pane is not quiesced, so its terminal is still moving.
    ///
    /// The refusal comes from the pty actor rather than from a flag kept
    /// alongside it: one owner of the state, one answer.
    #[error("pane is not quiesced")]
    NotQuiesced,
    /// The pty actor could not answer.
    #[error(transparent)]
    Pty(PtyActorError),
    /// The pane's state could not be turned into a manifest entry.
    #[error(transparent)]
    Manifest(#[from] ManifestError),
    /// The pane is on its way down.
    #[error("pane is stopping")]
    Gone,
}

impl From<PtyActorError> for ExportError {
    fn from(err: PtyActorError) -> Self {
        match err {
            PtyActorError::NotQuiesced => Self::NotQuiesced,
            PtyActorError::Gone | PtyActorError::Released => Self::Gone,
            other => Self::Pty(other),
        }
    }
}
