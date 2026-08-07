//! Resolving a [`PaneTarget`](amx_proto::control::pane::PaneTarget) to a pane.
//!
//! 04 §5 addresses agents "by user-assigned name or pane UUID", and D-M2-9
//! decides which namespace that name lives in: **the pane's label**. amx
//! already has a persisted, renameable, user-visible per-pane name, and
//! inventing a second would mean a second rename verb, a second persistence
//! field, and a picker showing two names for one pane.
//!
//! So `agent start dev` sets the pane's label to `dev`, and every agent verb
//! resolves its target the same way:
//!
//! 1. a UUID, if the string parses as one — checked *first*, so a pane
//!    mischievously labelled with another pane's UUID cannot shadow it;
//! 2. otherwise a label match, which must be **unique among agent panes**;
//! 3. otherwise an error that *names the candidates*, because "ambiguous
//!    target" without them is a message the user cannot act on.
//!
//! The pane-driving verbs (`pane.send_text` and friends) use the same wire type
//! with the wider rule — any pane, not only agent panes — since a script
//! driving a plain shell has no agent to be unique among.
//!
//! `agent rename` and `agent read` are this module plus an existing row:
//! resolve, then call `pane.rename`/`pane.read`. R-M2-10 flags the reading of
//! 04 §5's verb list in case a distinct agent-rename semantic was meant; it
//! names none.
//!
//! # Task ownership
//!
//! **V13** fills this, with `agent.start` and `agent.prompt`. Its acceptance
//! test `addressing_resolves_uuid_then_unique_label_and_names_ambiguity` is the
//! three rules above, in order.
//!
//! V02 planted the file so `agent/mod.rs` could be planted whole.
