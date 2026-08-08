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
//! 2. a short number, if the string is digits — the number `session.state`
//!    reports for the pane (04 §6), checked before labels for the same reason
//!    UUIDs are: `2` is what the picker and the status line show, and a pane
//!    labelled `2` must not be able to take that away from the pane that
//!    *is* 2;
//! 3. otherwise a label match, which must be **unique among agent panes**;
//! 4. otherwise an error that *names the candidates*, because "ambiguous
//!    target" without them is a message the user cannot act on.
//!
//! Rules 1 and 2 are questions about the string, not about the tree: a
//! digits-only target is a short number even when no pane holds that number,
//! and answers `no pane is numbered 7` rather than quietly looking for a pane
//! called `7`. A target that resolved differently depending on which panes
//! happened to be open is one nobody can learn.
//!
//! The pane-driving verbs (`pane.send_text` and friends) use the same wire type
//! with the wider rule — any pane, not only agent panes — since a script
//! driving a plain shell has no agent to be unique among. That is [`Scope`],
//! and it is the only difference between the two resolutions: one function, one
//! order, one set of error messages, so the two can never learn to disagree
//! about what a name means.
//!
//! `agent rename` and `agent read` are this module plus an existing row:
//! resolve, then call `pane.rename`/`pane.read`. R-M2-10 flags the reading of
//! 04 §5's verb list in case a distinct agent-rename semantic was meant; it
//! names none.
//!
//! # No I/O, no actors
//!
//! Everything here is a pure function over the pane list `session.state`
//! already carries, which is what lets the suite check the three rules against
//! a handful of literals rather than against a session. The one `Core` round
//! trip the label path costs belongs to the caller (`dispatch/pane.rs`), not to
//! this module.
//!
//! # Task ownership
//!
//! **V13** fills this, with `agent.start` and `agent.prompt`. Its acceptance
//! test `addressing_resolves_uuid_then_unique_label_and_names_ambiguity` is the
//! three rules above, in order.
//!
//! V02 planted the file so `agent/mod.rs` could be planted whole.

use amx_core::{PaneId, ShortNumber};
use amx_proto::control::pane::PaneTarget;
use amx_proto::control::session::PaneState;
use amx_proto::rpc::RpcError;
use thiserror::Error;

/// Which panes a label is allowed to name.
///
/// The two readings of one wire type, and the whole difference between them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Scope {
    /// Any pane in the session — the pane-driving verbs' rule.
    ///
    /// A script driving a plain shell has no agent to be unique among, so
    /// narrowing here would make `pane.run dev` fail on a pane the user named
    /// themselves.
    AnyPane,
    /// Only panes with an identified agent — the agent verbs' rule.
    ///
    /// D-M2-9's "unique among *agent* panes". Narrower on purpose: five
    /// identical Claude panes are orchestratable because five labels name them,
    /// and a shell that happens to share a label must not be one of the five.
    Agent,
}

impl Scope {
    /// Whether `pane` is inside this scope.
    ///
    /// An agent pane is one whose status carries a *kind*: the hub names a pane
    /// only once tier 1, 2 or 3 has identified the program in it, so a shell
    /// that briefly showed as `busy` is not an agent and a pane whose agent has
    /// exited still is — which is what a user addressing it by name means.
    fn admits(self, pane: &PaneState) -> bool {
        match self {
            Self::AnyPane => true,
            Self::Agent => pane
                .agent
                .as_ref()
                .is_some_and(|status| status.kind.is_some()),
        }
    }
}

/// Resolve `target` against the panes `session.state` reports.
///
/// The rules of the module documentation, in order. A UUID is taken at face
/// value in either scope: it is unambiguous, and refusing one for a pane
/// whose agent has not been identified *yet* would make `agent prompt <uuid>`
/// fail for a race rather than for a reason. A short number is not taken at
/// face value in the same way, because unlike a UUID it can be a number no
/// pane holds — it is looked up, and the scope applies to what it finds.
///
/// # Errors
///
/// [`AddressError`] naming the candidates whenever there is more than one, or
/// naming the panes that did match when the scope excluded all of them.
pub fn resolve(
    target: &PaneTarget,
    panes: &[PaneState],
    scope: Scope,
) -> Result<PaneId, AddressError> {
    let name = target.as_str();
    if let Ok(pane) = name.parse::<PaneId>() {
        return Ok(pane);
    }
    if let Some(number) = ShortNumber::parse(name) {
        let Some(numbered) = panes.iter().find(|pane| pane.short == number) else {
            return Err(AddressError::UnknownNumber { number });
        };
        if !scope.admits(numbered) {
            return Err(AddressError::NoAgent {
                name: name.to_owned(),
                panes: vec![numbered.pane],
            });
        }
        return Ok(numbered.pane);
    }
    let named: Vec<&PaneState> = panes
        .iter()
        .filter(|pane| pane.label.as_deref() == Some(name))
        .collect();
    if named.is_empty() {
        return Err(AddressError::Unknown {
            name: name.to_owned(),
        });
    }
    let matched: Vec<PaneId> = named
        .iter()
        .filter(|pane| scope.admits(pane))
        .map(|pane| pane.pane)
        .collect();
    match matched.len() {
        1 => Ok(matched[0]),
        0 => Err(AddressError::NoAgent {
            name: name.to_owned(),
            panes: named.iter().map(|pane| pane.pane).collect(),
        }),
        _ => Err(AddressError::Ambiguous {
            name: name.to_owned(),
            panes: matched,
        }),
    }
}

/// Whether `name` may become a new agent pane's label.
///
/// `agent.start`'s gate, and the reason it exists is [`resolve`] above: a name
/// that cannot be resolved back is a pane that cannot be addressed, and 04 §5's
/// whole argument for names is that "five identical Claude panes are only
/// orchestratable if they have names". Three ways a name fails that test:
///
/// - **blank** — nothing to type, and a label that renders as nothing in the
///   picker;
/// - **UUID-shaped** — rule 1 checks UUIDs first, so such a label is shadowed
///   by whatever pane the UUID names (or by nothing at all) and can never win;
/// - **all digits** — rule 2 checks short numbers first, for the same reason
///   and with the same result;
/// - **already a pane's label** — the second one makes both unaddressable,
///   which is exactly the ambiguity error the resolver would then have to
///   report forever.
///
/// The check is against *every* pane, not only agent panes: a shell named `dev`
/// and an agent named `dev` are ambiguous to a human reading the picker even
/// though [`Scope::Agent`] could tell them apart.
///
/// # Errors
///
/// [`AddressError`] saying which of the three it was.
pub fn check_new_name(name: &str, panes: &[PaneState]) -> Result<(), AddressError> {
    if name.trim().is_empty() {
        return Err(AddressError::BlankName);
    }
    if name.parse::<PaneId>().is_ok() {
        return Err(AddressError::UuidName {
            name: name.to_owned(),
        });
    }
    if ShortNumber::parse(name).is_some() {
        return Err(AddressError::NumberName {
            name: name.to_owned(),
        });
    }
    if let Some(taken) = panes
        .iter()
        .find(|pane| pane.label.as_deref() == Some(name))
    {
        return Err(AddressError::NameTaken {
            name: name.to_owned(),
            pane: taken.pane,
        });
    }
    Ok(())
}

/// Why a target named no single pane.
///
/// Every variant that *could* name candidates does: a caller told only
/// "ambiguous target" has to go and query the session themselves to find out
/// what they collided with, and the server already knows.
#[derive(Clone, PartialEq, Eq, Debug, Error)]
pub enum AddressError {
    /// No pane carries that label, and it is not a pane UUID.
    #[error("no pane is named {name:?}")]
    Unknown {
        /// The target, as written.
        name: String,
    },
    /// More than one pane in scope carries that label.
    #[error("{name:?} names {} panes: {}", panes.len(), join(panes))]
    Ambiguous {
        /// The target, as written.
        name: String,
        /// The panes that matched.
        panes: Vec<PaneId>,
    },
    /// The label matched, but no agent is tracked in any pane carrying it.
    #[error(
        "{name:?} names {} panes and no agent is running in {}: {}",
        panes.len(),
        if panes.len() == 1 { "it" } else { "any of them" },
        join(panes)
    )]
    NoAgent {
        /// The target, as written.
        name: String,
        /// The panes that carried the label.
        panes: Vec<PaneId>,
    },
    /// A new agent was asked for with no name.
    #[error("an agent's name is the pane's label, and a label cannot be blank")]
    BlankName,
    /// No pane carries that short number.
    #[error("no pane is numbered {number}")]
    UnknownNumber {
        /// The number, as written.
        number: ShortNumber,
    },
    /// A new agent was asked for with a UUID-shaped name.
    #[error("{name:?} is shaped like a pane UUID, which is checked first and would shadow it")]
    UuidName {
        /// The name, as written.
        name: String,
    },
    /// A new agent was asked for with an all-digits name.
    #[error("{name:?} is a short number, which is checked first and would shadow it")]
    NumberName {
        /// The name, as written.
        name: String,
    },
    /// A new agent was asked for with a name a pane already carries.
    #[error("{name:?} already names pane {pane}; two panes with one name address neither")]
    NameTaken {
        /// The name, as written.
        name: String,
        /// The pane that has it.
        pane: PaneId,
    },
}

impl From<AddressError> for RpcError {
    /// Always `INVALID_PARAMS`: every one of these is a fact about what the
    /// caller asked for, not about whether the session is working.
    fn from(err: AddressError) -> Self {
        Self::new(Self::INVALID_PARAMS, err.to_string())
    }
}

/// Pane ids as one comma-separated list, for an error a human has to act on.
fn join(panes: &[PaneId]) -> String {
    panes
        .iter()
        .map(PaneId::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}
