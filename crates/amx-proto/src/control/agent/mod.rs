//! `agent.*` payloads.
//!
//! Six rows now: `agent.report` (the hook path), `agent.start`, `agent.prompt`,
//! `agent.explain`, `agent.next` — five of `docs/08-m2-plan.md` §4 — and M4's
//! `agent.list`, the one data source all three of D15's attention surfaces read
//! (`docs/10-attention-surfaces.md`). `agent rename` and `agent read` are
//! deliberately *not* here — D-M2-9 delivers them by resolving the agent's
//! target and calling the existing `pane.rename` / `pane.read` rows, because the
//! agent's name **is** the pane's label. The CLI surface matches 04 §5's verb
//! list; the method table stays one row per behavior (R-M2-10 flags the reading
//! in case 04 meant a distinct semantic — it names none).
//!
//! # Why this is a directory
//!
//! One file of 463 lines, at the soft budget with M4's payloads still to land
//! (`docs/11-m4-plan.md` R-M4-5). Split by what a reader comes here for rather
//! than by size: [`hook`] is the emitter's contract, which changes when an
//! agent's hook system does; [`verbs`] is the four things a person asks an
//! agent to do; [`list`] is the read-only projection three surfaces render. The
//! move was mechanical — not a line of the three moved types changed — and every
//! name is re-exported below, so `amx_proto::control::agent::HookEvent` is the
//! path it always was.
//!
//! # Task ownership
//!
//! V02 froze the M2 shapes. **V09** fills `agent.report`, **V13** `agent.start`
//! and `agent.prompt`, **V06** `agent.explain`, **V08** `agent.next`. X02 froze
//! `agent.list` and `NextParams.workspace`; **X10** answers the row from `Core`,
//! **X14** and **X16** render it, **X17** reads the scope.

pub mod hook;
pub mod list;
pub mod verbs;

pub use hook::{HookEvent, ReportParams, ReportReply, ReportScope};
pub use list::{AgentEntry, ListParams, ListReply};
pub use verbs::{
    ExplainParams, ExplainReply, NextParams, NextReply, PromptParams, PromptReply, PromptWait,
    Readiness, RuleVerdict, StartParams, StartReply,
};
