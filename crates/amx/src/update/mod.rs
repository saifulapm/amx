//! Self-update: the channel manifest, the fetch, the checksum, the swap.
//!
//! D-M3-8's four mechanisms, one module each. They are herdr's mechanisms —
//! read from its `update.rs` and reimplemented, never copied — because each one
//! answers a question amx has too:
//!
//! - [`manifest`] — what a channel publishes: a version, release notes, and one
//!   asset per platform carrying a URL and a sha256.
//! - [`fetch`] — how bytes arrive: a curl subprocess with argv as data, so no
//!   HTTP stack, no TLS stack and no certificate store enter the dependency
//!   tree for a command most users run twice a year.
//! - [`verify`] — the sha256 the manifest named, checked before anything is
//!   installed, over the `sha2` crate (R-M3-8, the milestone's one new
//!   dependency).
//! - [`install`] — stage in the state directory, then one `rename(2)` over the
//!   running executable. Rename is legal on a running binary on unix: `ETXTBSY`
//!   guards writes into the image, not the replacement of the directory entry
//!   that names it.
//! - [`pm`] — and the case where amx must do none of that: a binary Homebrew,
//!   mise or Nix installed is that manager's file, so amx names the manager's
//!   own upgrade command and writes nothing.
//!
//! # There is no channel to point at yet
//!
//! This module ships the format and the machinery. It does not ship a release
//! pipeline, prebuilt binaries, or a preview channel — none of the three
//! exists, and R-M3-4 is explicit that treating this as a hosted service would
//! be reading a stub as a product. [`manifest::DEFAULT_CHANNEL`] names where the
//! manifest *will* live; until a release puts one there, `amx update check`
//! says so in a plain sentence and exits successfully, because "nothing is
//! published" is an answer and not a failure.
//!
//! # The dormant half
//!
//! D-M3-8 ends with a handoff: `apply` asks the running session for
//! `session.handoff` and then reconnect-polls for the successor. The server
//! side of that call is W06's and has not landed — `session.handoff` answers
//! with a seam error today — so the glue is written, compiled and typechecked
//! but not reached: [`crate::cmd::update::HANDOFF_AFTER_INSTALL`] is `false`,
//! and flipping that one constant to `true` is the whole of turning it on
//! (W14). `apply` says which of the two it did, every time, rather than leaving
//! the user to infer whether their session was swapped.

pub mod fetch;
pub mod install;
pub mod manifest;
pub mod pm;
pub mod verify;
