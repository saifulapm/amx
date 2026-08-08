# Design review — findings register (2026-08-08)

An external full-design review: docs 01–09 read against the tree at
`497ed41`, a code audit of every mechanism 04 promises, and a plan audit of
06–09 against their recorded outcomes. This note is the durable register of
what it found. Each finding carries an id (DR-n) so fixes can reference it;
strike a row's status when it lands.

**Overall verdict.** No architectural mistake found. The bets 04 makes are
sound and the measured spikes (scrollback identity, hook coverage, shutdown
wedge) validated the risky ones. The budget holds (zero files over hard
1000; largest source file 505 lines), `todo!()` count is 2, CI is real. The
findings below are execution gaps and process debt, jointly estimated at
**2–3 weeks** of focused work.

Ratings, for the record: herdr ≈ 7.5/10 product, 4.5/10 structure; amx
design docs ≈ 9/10; amx execution to date ≈ 8.5/10. The gap between herdr's
two numbers is amx's reason to exist, and it was earned by verification
(02), not assertion.

---

## Critical path

**DR-1 — The `Cells` wire format is the missing keystone.** `open`
`GridMessage::Cells` encode/decode are `todo!()` and the client's `Attrs` is
an admitted stand-in, while the entire damage pipeline around the payload —
coalescing, keyframes, credit flow, priority writer — is finished and
tested. This violates the M0 rule that wire-visible things exist before
goldens freeze the protocol. Highest priority; it only gets more expensive
as surface accretes around it. Scope: packed cell layout (text incl. mode
2027 grapheme clusters, wide cells, palette + RGB color, attrs), goldens.
~1 focused week.

**DR-2 — History delivery is unwired.** `open`
The client cache computes fetchable gaps; nothing delivers `HistoryChunk`
into it, so "smart client with local scrollback cache" — the headline
differentiator — is scaffolding. Both ends exist and are measured
(3.3–3.8 µs/row served off the parser thread). Does not block on DR-1:
history ships as unstyled rows (R-M1-1). ~2–4 days.

**DR-3 — `pane.run` prompt loss.** `open — verify first`
M2's live outcomes recorded ~3% of prompts lost against real Claude Code,
and the ordering invariant the "queue-order atomic" claim leans on was found
false (two producers on the input queue). 04 §8 sells the verb on exactly
that atomicity. Verify current behavior; if live, fix ordering (single
producer or sequencing), then re-run the live smoke. ~1–3 days.

## Process

**DR-4 — The unowned integration seam, paid for three times.** `open`
The wave/file-ownership scheme that makes parallel execution safe leaves
cross-crate joins owned by nobody, by construction. T19, U10, V17 and W14
are four retrofits of the same hole; M2's W-1 (hub and gateway "both correct
and never met over a socket") names it the third payment. Fix structurally
in the next plan: a standing integration owner per milestone, and the live
end-to-end smoke running from wave 1 — not as an exit gate.

**DR-5 — Over-planning at test-name granularity.** `open`
Pre-drafted acceptance-test names invited satisfying the name over the
intent (T18: "identical grid" asserted against possibly-blank screens), and
pre-specified items were later withdrawn as misreadings (R-M2-2, R-M3-9).
Keep decision registers, risk tables, spike-first gating. Drop pre-written
test names and prompt drafts.

## Contract-before-consumer debt

**DR-6 — ShortNumbers.** `open`
`todo!()` at two sites in `amx-core/src/id.rs`, routed around in
`cmd/attach.rs`, and the snapshot persists a documented stand-in — quietly
shaping the eventual implementation's constraints from disk. Pay down
before more snapshots accumulate. ~1–2 days.

**DR-7 — `GridMessage::Scrolled` is dead wire surface.** `open`
Defined, golden-tested, client-decoded, never emitted (deferred since M1).
Delete it (re-add when a scroll-region optimization wants it) or emit it.
Golden tests protecting unexercised surface is how skew guarantees rot.
~0.5 day.

**DR-8 — Tier-3 probe walk has no caller.** `open`
Shipped in M2 (W-2) and still unconsumed; Codex-by-hand remains
unidentified. Either wire the caller or record the deferral with a revisit
condition in the M4 plan.

## Doc drift (04 is binding per HACKING.md — it must stay true)

**DR-9 — Owed corrections.** `open`
- 04 §2 "broadcast event bus" → the implementation is a bespoke
  cursor-over-replay-ring bus (typed `gap`, resumable cursors — better than
  the promised primitive; the word is what's wrong).
- 04 §6 `history/<uuid>.ansi` → `.rows` (R-M1-1, already flagged).
- The R3 correction from the M0 plan: herdr's bindings *are*
  bindgen-generated; its defect is the missing regeneration check, not
  hand-copied output.
- README milestone label ("this is M0") lags a tree carrying M2/M3 work.
Hours, one PR.

**DR-10 — Three unrelated `Effect` enums; client dirtiness is ad-hoc.**
`open`
`amx-core::effect::Effect`, `agent/fusion`'s `Effect`, and
`amx-vt::callbacks`' `Effect` shadow each other. And `amx-client` never
consumes the structural dirtiness type at all — it uses two plain booleans,
the exact failure mode D2 exists to prevent. Rename the two shadows; either
adopt `Effect` client-side or write the exemption into 04. ~1 day.

## Watch items (instrumented, not schedulable)

**DR-11 — The second shutdown wedge.** `watch`
The seven field corpses were *not* the diagnosed prologue wedge (peerless
sockets refuse those awaits). The open mechanism is narrowed to
`ConnEvents::shutdown` / `ConnStreams::shutdown` and the drain census now
names the holder when it next fires. Owner: whoever takes W08's
neighborhood. Do not let it age silently the way the first wedge did.

**DR-12 — `frame on unbound channel` under flood.** `watch`
Distinct from the fixed client-reader cancellation bug: a stream-lifecycle
race seen 3/30 rounds under adversarial load, in `stream.bind`'s
neighborhood (W08 binds a generation onto that path).

## Accepted trades to state out loud

**DR-13 — Remote latency honesty.** `open (one sentence in 03)`
The p99 < 5 ms key→echo budget is local and round-trip by design; over SSH,
typing feel is tmux-class, and local-echo smart clients (Superlogical) will
beat it there. Predictive echo stays a capability-gated extension (04 §4).
The trade is right; 03 should own it explicitly.

**DR-14 — Fusion does not eliminate screen scraping.** `no action`
Both shipped agents measured `edges`: tier 2 owns every user-initiated exit,
so herdr's manifest-catalog maintenance burden carries over. amx's edge over
herdr is entry-edge latency plus honesty about coverage — not scraping's
removal. Recorded so nobody resells the differentiator as more than it is.

## Resolved by design since review

- **Wheel scroll / mobile** → D14, [10-attention-surfaces.md](../10-attention-surfaces.md).
- **Attention at 25-agent scale** (per-workspace visibility, survey view,
  scoped cycling) → D15, same doc.

## Suggested order

DR-1 → DR-2 → DR-3 → DR-6 → DR-7/DR-9/DR-10 batched → DR-4/DR-5 into the M4
plan → D14/D15 implementation (~2 weeks, [10-attention-surfaces.md](../10-attention-surfaces.md)).
