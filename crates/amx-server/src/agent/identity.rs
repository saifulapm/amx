//! Tier 3: what is actually running in the pane's foreground.
//!
//! 04 §5: "Process-tree foreground job + prompt detection for unknown programs:
//! `busy/quiet`, **never fake `blocked`**." That last clause is already true by
//! typing — this tier's output is an
//! [`Activity`](amx_core::agent::Activity), which has no `blocked` to spell.
//!
//! What this module answers is the identification question: given a pane's
//! foreground process group, which registry stanza (if any) owns it. D-M2-4 and
//! V07 fix the method:
//!
//! - the foreground **group leader** first, then job members scored by
//!   unwrapped evidence;
//! - unwrapping `node`/`bun`/`python`/`sh -c` and path tokens, because an agent
//!   shipped as a Node script has `node` as its `comm`;
//! - **eval-flag arguments never identify**: `python -c "codex"` is not codex.
//!   herdr carries that negative test and V07 re-derives it.
//!
//! Probes are triggered, rate-limited work — a spawn, damage-after-quiet, an
//! `agent start` readiness check — and never a free-running scan loop.
//!
//! Identity carries weight two other tiers do not, and V01 §4 is why: a pane
//! running an idle Codex has **no hook-borne identity at all** until the user
//! types their first prompt, because Codex fires no `SessionStart` before then.
//! For that pane, this module is the only thing that knows what it is looking
//! at — so `agent status` must not report it as unidentified and `agent start
//! codex` readiness cannot wait on a hook.
//!
//! # Task ownership
//!
//! **V07** fills this, together with the `argv`/`exe` extension to
//! `amx-core`'s `ProcessTree` seam and its two platform implementations (Linux
//! `/proc/<pid>/cmdline`, darwin `KERN_PROCARGS2` — which libproc does not
//! wrap, so R-M2-8 budgets real time for its buffer layout).
//!
//! V02 planted the file so `agent/mod.rs` could be planted whole.
