# Design review — findings register (2026-08-08)

An external full-design review: docs 01–09 read against the tree at
`497ed41`, a code audit of every mechanism 04 promises, and a plan audit of
06–09 against their recorded outcomes. This note is the durable register of
what it found. Each finding carries an id (DR-n) so fixes can reference it;
strike a row's status when it lands.

**M3 re-verification (same day).** Hours after this register was written,
main landed the M3 milestone (100 commits: handoff, SSH bridge, self-update,
worktrees, layout export, reconnect-resync — live smoke passed all seven
exit criteria). Every row was re-verified against the merged tree at
`b727786`; statuses below are post-M3. M3 resolved DR-1, DR-2, DR-3 and
DR-8, hardened DR-11, and partially addressed DR-12 — none of them by
reference to this register, which had not landed yet; the milestone simply
built what the register asked for. New M3-era findings are DR-15…DR-21 at
the end.

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

**DR-1 — The `Cells` wire format is the missing keystone.** `resolved (M3)`
The codec is real: length-prefixed `put_cells` / `read_rows`
(`amx-proto/src/stream/grid.rs:242,379`) over a per-cell layout in
`amx-proto/src/stream/cell.rs`; no `todo!()` remains in amx-proto.
Residuals: `amx-client/src/model/grid.rs:33` still *says* "both bodies are
still `todo!()`" (stale prose → DR-15), and the client's `Attrs` remains a
reduced projection — check styling completeness before calling the
smart-client rendering claim finished.

**DR-2 — History delivery is unwired.** `resolved (M3)`
The full path exists: client requests (`app/binds.rs:151`
`fetch_wanted_history` / `bind_history`), pump in `app/wired.rs:193`,
decode + `cache.commit`/`cache.fill` in `amx-client/src/stream.rs:141-211`;
server chunks ranges in `amx-server/src/history/range.rs:92`. Residual:
`amx-client/tests/scrollback.rs:5` prose still claims no wire path delivers
history (→ DR-15).

**DR-3 — `pane.run` prompt loss.** `resolved (M3, redesigned)`
The claim was re-specified rather than patched: text and submit are two
chunks queued back-to-back under the pane input queue's ordering lock
(`pty/handle.rs:209` `try_write_input_pair`, called from
`pane_host/drive.rs:178`), with the ~3% single-`write()` swallow rate cited
in the spec comment (`drive.rs:58-65`) and tests at `tests/pty.rs:476,550`.
Matches 04 §8's wording as amended.

## Process

**DR-4 — The unowned integration seam, paid for four times.** `open`
The wave/file-ownership scheme that makes parallel execution safe leaves
cross-crate joins owned by nobody, by construction. T19, U10, V17 and now
W14 are four retrofits of the same hole; M2's W-1 (hub and gateway "both
correct and never met over a socket") named it the third payment, and M3's
W14 closed eight hand-offs and found three new bugs at the join — the
fourth. Fix structurally in the M4 plan: a standing integration owner per
milestone, and the live end-to-end smoke running from wave 1 — not as an
exit gate.

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

**DR-8 — Tier-3 probe walk has no caller.** `resolved (M3)`
`identify` (`agent/identity.rs:267`) is now reached in production through
the hub (`actor/agent_hub/mod.rs:366`, `run.rs:163`) with the process tree
injected from `pane_host`; tests in `tests/identity.rs`.

## Doc drift (04 is binding per HACKING.md — it must stay true)

**DR-9 — Owed corrections.** `partially resolved`
- `resolved on main`: 04 §6 now reads `history/<pane-uuid>.rows`.
- `resolved on this branch`: README milestone label updated (was "this is
  M0" after M3 landed).
- Still open: 04 §2/§10 "broadcast event bus" (lines 48, 420) → the
  implementation is a bespoke cursor-over-replay-ring bus (typed `gap`,
  resumable cursors — better than the promised primitive; the word is
  what's wrong); and the R3 correction from the M0 plan (herdr's bindings
  *are* bindgen-generated; its defect is the missing regeneration check).
- New, same family: two stale SSH passages — 09-m3-plan §7 clause 3 and the
  wave-outcomes "one unverified exit clause" paragraph both predate
  m3-live-smoke §5, which records the SSH criterion verified on a second
  machine (aarch64). See also DR-15.
Hours, one PR.

**DR-10 — Three unrelated `Effect` enums; client dirtiness is ad-hoc.**
`open`
`amx-core::effect::Effect`, `agent/fusion`'s `Effect`, and
`amx-vt::callbacks`' `Effect` shadow each other. And `amx-client` never
consumes the structural dirtiness type at all — it uses two plain booleans
(`app/mod.rs:131,139`), the exact failure mode D2 exists to prevent. The
server side consumes the core type properly (post-M3: `pane_host/actor.rs`,
`core/pane.rs`, `core/report.rs`), so the gap is client-only. Rename the
two shadows; either adopt `Effect` client-side or write the exemption into
04. ~1 day.

## Watch items (instrumented, not schedulable)

**DR-11 — The second shutdown wedge.** `watch — hardened in M3`
The seven field corpses were *not* the diagnosed prologue wedge (peerless
sockets refuse those awaits). M3 hardened the suspect epilogue: both
`ConnEvents::shutdown` (`conn/events.rs:184`) and `ConnStreams::shutdown`
(`conn/streams.rs:217`) now cancel first and take their joins out of the
lock before awaiting (no guard held across an await), and the drain census
names the holder when a drain overruns (`tests/runtime.rs`). The field
mechanism was still never caught in the act — keep the watch until a census
either fires or a milestone of field time passes clean.

**DR-12 — `frame on unbound channel` under flood.** `watch — partial`
W08 landed generation-carrying binds (`conn/streams.rs:122-145`,
`conn/resume.rs`) and R-M3-14's retraction made resumed binds repaint
(`59f6ed8`) — that closes the stale-grid-on-rebind hole. The unbound-channel
race itself (3/30 rounds under flood) has no named test, and the client's
current handling of an unknown channel appears to be a silent
`Applied::Nothing` (`amx-client/src/stream.rs`) — decide deliberately
whether refusal or silence is intended: the old refusal was the check that
caught a desynchronised peer (see notes/frame-read-cancellation.md).

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

## M3 addendum — new findings (re-verification at `b727786`)

**DR-15 — Stale in-tree prose contradicting shipped code.** `open`
Comments asserting things M3 made false: `amx-client/src/model/grid.rs:33`
("both bodies are still `todo!()`" — the codec exists),
`amx-client/tests/scrollback.rs:5` ("no wire path delivers" — it does), and
the two stale SSH passages named under DR-9. Cheap and worth doing
promptly: in a repo whose discipline is "never guess, verify against
source", stale prose is actively poisonous. Hours.

**DR-16 — Reconnect coverage is uneven (W09's untaken hand-offs).** `open`
The attached client rides a server swap, but: `cmd/viewport.rs`
(`attach --pane`) never reconnects; a bridged (SSH) client cannot redial
(needs a respawned ssh child — `remote/`'s business); and
`DriveError::NotAccepting` surfaces as `INVALID_PARAMS`, so D-M3-6's
"caller's retry" is unactionable — a distinct error code would make every
mutating verb retriable across handoff, including `agent.prompt`. The last
one is wire-adjacent: decide before more callers bake in the ambiguity.

**DR-17 — Remote UX edges.** `open`
`amx --help` never mentions `--remote` (kept out of clap by design; the
help text still owes the flag a line). A newline-containing session name
cannot cross to a csh login shell (needs a wire encoding — interface
change, so decide, don't drift). `$MISE_INSTALLS_DIR` is deliberately
unread, so a relocated mise root misclassifies as Standalone (one `Env`
field + one `pm::classify` param when it matters).

**DR-18 — No release channel exists.** `open (R-M3-4 standing)`
The update machinery is proven end to end (sha256, swap, handoff, live
smoke 0.1.0→0.1.1 over a `file://` channel) but the default channel 404s.
Fine until the first external user; the risk stays "a stub read as a
service".

**DR-19 — Recorded flakes, unowned.** `open`
`flow_control::two_clients_at_different_speeds` (1×), `agent_verbs` (2 in
~12 runs), the `urandom` 8 MiB machine-speed threshold (fails under 8-way
load every time — turn it into rate-over-observed-time), and `_hook`'s
BrokenPipe self-race (tolerate BrokenPipe on the payload write). Each has
its mechanism written down; none has an owner.

**DR-20 — SSH exit clause residuals.** `open`
m3-live-smoke §5 verified the criterion on a second machine, but: the
cross-arch skew table ran current-vs-current only (far side cross-built
from the same tree, not independently versioned), and no handoff or
`update apply` has run over the remote link. Name these in the M4 plan
rather than letting "SSH works" round up.

**DR-21 — Resume optimization, recorded not built.** `open (optional)`
R-M3-14's sound route: a reconnecting client drains event replay before
binding grids, so unchanged grids skip their keyframes without trusting
generations. Client-side only, no wire change. Worth taking when reconnect
traffic matters (many panes × frequent swaps), not before.

Budget snapshot post-M3: 28 files over soft (24 tests), 0 over hard;
largest src `pane_host/parser.rs` 532, then `remote/ssh.rs` 516,
`fusion/tracker.rs` 511, `pane_host/mod.rs` 501 — all with recorded
reasons. `todo!()` count in src: 2, both DR-6.

## Suggested order (post-M3)

DR-15 (hours, do first — stale prose misleads every next task) → DR-6 →
DR-16's error-code decision (wire-adjacent) → DR-7/DR-9/DR-10 batched →
DR-19 flake paydown → DR-4/DR-5 written into the M4 plan → D14/D15
implementation (~2 weeks, [10-attention-surfaces.md](../10-attention-surfaces.md))
→ DR-17/DR-18/DR-20/DR-21 as M4 scope decisions.
