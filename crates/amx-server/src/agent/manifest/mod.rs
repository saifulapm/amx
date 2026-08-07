//! Tier 2: the screen manifest engine.
//!
//! 04 §5 keeps herdr's TOML rule engine wholesale — "priority, region, gate
//! trees, `skip_state_update` freezes, bottom-buffer snapshot, `agent explain`
//! debuggability … running continuously as the fusion partner for `edges`
//! agents and the sole source for `identity` agents". amx writes its own
//! engine against that grammar; herdr is Apache-2.0 and amx is an independent
//! implementation, so the mechanism is studied and never the lines.
//!
//! What amx changes, per D-M2-3:
//!
//! - **The scrolled-viewport hazard is gone by construction.** herdr anchors
//!   its detection buffer to the scrollback bottom and regression-tests that a
//!   scrolled viewport never moves it, because its server owns the scrolled
//!   view. amx's published snapshot *is* the live visible grid — scrollback and
//!   scroll position are client-side (04 §3) — so there is no anchor to keep.
//! - **Evaluation is pushed, never polled.** The trigger is the `PaneDamage`
//!   event stream plus per-pane coalescing in `AgentHub`, not herdr's permanent
//!   300–500 ms scan loop (03 §5: push, never poll).
//! - **The region vocabulary starts minimal and whitelisted**: `whole_recent`,
//!   `bottom_lines(N)`, `bottom_non_empty_lines(N)`, `title`. A whitelist so
//!   rules cannot drift into whole-scrollback greps; minimal because herdr's
//!   structural prompt-marker regions should only arrive if a shipped manifest
//!   needs one. Bias state rules to bottom-anchored regions — herdr's changelog
//!   documents `whole_recent` matching stale "esc to interrupt" scrollback as a
//!   real production bug.
//!
//! Every regex compiles **once at load**, stored beside its parsed rule. This
//! runs per damage batch; a pattern recompiled there would be paid for on every
//! frame of a busy pane.
//!
//! # Task ownership
//!
//! **V06** fills this whole subtree — `rule.rs`, `region.rs`, `compile.rs`,
//! `explain.rs` beside this file, the `claude` and `codex` manifests under
//! `assets/manifests/`, and `dispatch/agent.rs`'s `explain` arm as a declared
//! sequential fill (`docs/08-m2-plan.md` §6). Its screen fixtures come from
//! V01's recordings, under the spike's `dumps/`.
//!
//! V02 planted the file so `agent/mod.rs` could be planted whole.
