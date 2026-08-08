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

**M4 wave 1 (2026-08-09, `6f4fb5d`).** The milestone
([11-m4-plan.md](../11-m4-plan.md)) took this register as its work list. Wave 1
closes DR-4, DR-5, DR-6, DR-9, DR-13, DR-15 and DR-19; statuses below are
struck by the standing integration owner (X00) from each task's report, one
editor rather than five. Two more rows turned out to have been paid before they
were scheduled — DR-9's R3 clause and DR-19's `agent_verbs` clause — which with
DR-17's `--help` clause (R-M4-6) makes three in twenty-one. Each was
re-verified against the tree and the commit graph before it was struck, and
each is recorded as *found already done* rather than as work the milestone did.

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
Residuals: the stale comment at `amx-client/src/model/grid.rs:33` is corrected
(M4/X03, under DR-15), and it now states the gap it used to hide — the client's
`Attrs` remains a reduced projection, six of the wire's ten attributes.
Widening it is scheduled as M4's X18.

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

**DR-4 — The unowned integration seam, paid for four times.**
`resolved (M4, structurally)`
The fix asked for is in place and running: [11-m4-plan.md](../11-m4-plan.md) §6
gives M4 a standing integration owner from wave 1 (X00) holding five named
cross-crate seams, and the live smoke is a gate at every wave boundary rather
than an exit step — [m4-live-smoke.md](m4-live-smoke.md) §1 is the baseline
recorded before wave 2 opened, §2 the first delta. It found the wave's own
integration break to be one the compiler catches (§2.6) and two seams measured
before either half of them is built. Whether the practice pays is answerable at
M4's exit; that the structure exists is answerable now, which is what this row
asked for.
The wave/file-ownership scheme that makes parallel execution safe leaves
cross-crate joins owned by nobody, by construction. T19, U10, V17 and now
W14 are four retrofits of the same hole; M2's W-1 (hub and gateway "both
correct and never met over a socket") named it the third payment, and M3's
W14 closed eight hand-offs and found three new bugs at the join — the
fourth. Fix structurally in the M4 plan: a standing integration owner per
milestone, and the live end-to-end smoke running from wave 1 — not as an
exit gate.

**DR-5 — Over-planning at test-name granularity.** `resolved (M4 plan)`
11-m4-plan.md carries no acceptance-test names and no agent prompt drafts, and
says so in its own preamble; each §5 entry states scope, dependencies, owned
files and what must be true, and leaves the tests to the implementer. What it
keeps is what this row said to keep — a decision register (§1), a risk table
(§8) and spike-first gating (X01 gates X13).
Pre-drafted acceptance-test names invited satisfying the name over the
intent (T18: "identical grid" asserted against possibly-blank screens), and
pre-specified items were later withdrawn as misreadings (R-M2-2, R-M3-9).
Keep decision registers, risk tables, spike-first gating. Drop pre-written
test names and prompt drafts.

## Contract-before-consumer debt

**DR-6 — ShortNumbers.** `resolved (M4, X05)`
Both bodies implemented as 04 §6 specifies — lowest free number, reused after
release — adopted by `Core`, read by `attach --pane`, and joined to the
UUID-then-label resolution order. Two things make it more than a `todo!()`
paydown. A shipped test was asserting the *stand-in's* behaviour and was
rewritten, and a collision the stand-in could produce during restore was found
and fixed while porting it. And the claim that most needed evidence — that a
snapshot the stand-in wrote still restores correctly — was measurable exactly
once: the wave-1 baseline smoke left such a file behind before X05 merged, and
[m4-live-smoke.md](m4-live-smoke.md) §2.3 restores it, keeps all 25 numbers and
then fills its six holes in lowest-free order. No `todo!()` remains in
`crates/*/src`, and `tests/hygiene/unfinished.rs` now enforces that rather than
stating it.

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

**DR-9 — Owed corrections.** `resolved (M4, X03)`
- `resolved on main`: 04 §6 now reads `history/<pane-uuid>.rows`.
- `resolved before M3 merged`: README milestone label.
- `resolved (X03)`: 04 §2/§10's "broadcast event bus" now names what is built —
  a cursor-over-replay-ring bus with typed gaps and resumable cursors, which is
  better than the promised primitive; the word was what was wrong.
- `resolved (X03)`: both stale SSH passages — 09-m3-plan §7 clause 3 and the
  wave-outcomes paragraph beside it — now record m3-live-smoke §5's second
  machine.
- **Already paid when it was written.** The R3 clause (herdr's bindings *are*
  bindgen-generated; the defect is the missing regeneration check) was corrected
  by `261e33b`, *docs: correct the herdr FFI drift description*, on 2026-08-06 —
  an ancestor of `b727786`, the tree this register says it re-verified against.
  04 §3 and 02's W10 have said so since. Struck as found, not as work X03 did.

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
**M4 is that milestone of field time, and the count is kept**: every live-smoke
run records each `session stop`'s exit status, the presence of a `drain-census`
file and any census log line. Six clean stops through wave 1, over sessions of
2, 3, 25 and 26 panes ([m4-live-smoke.md](m4-live-smoke.md) §1.7, §2.7); nothing
has fired. R-M4-10 is what turns that from an impression into a record.

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

**DR-13 — Remote latency honesty.** `resolved (M4, X03)`
The p99 < 5 ms key→echo budget is local and round-trip by design; over SSH,
typing feel is tmux-class, and local-echo smart clients (Superlogical) will
beat it there. Predictive echo stays a capability-gated extension (04 §4).
The trade is right; 03 should own it explicitly — and now does, in its own
words, beside the budget it qualifies. Nothing about the trade changed; the
doc stopped leaving it unsaid.

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

**DR-15 — Stale in-tree prose contradicting shipped code.**
`resolved (M4, X03)`
All four corrected: `amx-client/src/model/grid.rs:33` (which now states the real
gap — four of ten attributes plus two colours — instead of a `todo!()` that no
longer exists), `amx-client/tests/scrollback.rs:5` (which now names the delivery
path file by file), and the two SSH passages under DR-9. X03 went further than
the row asked in one place worth recording: `crates/amx-client/tests/cell_style.rs`
pins the corrected comment to the decode, so the claim cannot go stale silently
again — a staleness guard rather than a regression test, and X18 has to update
both together.

**DR-16 — Reconnect coverage is uneven (W09's untaken hand-offs).** `open`
The attached client rides a server swap, but: `cmd/viewport.rs`
(`attach --pane`) never reconnects; a bridged (SSH) client cannot redial
(needs a respawned ssh child — `remote/`'s business); and
`DriveError::NotAccepting` surfaces as `INVALID_PARAMS`, so D-M3-6's
"caller's retry" is unactionable — a distinct error code would make every
mutating verb retriable across handoff, including `agent.prompt`. The last
one is wire-adjacent: decide before more callers bake in the ambiguity.

**DR-17 — Remote UX edges.** `open — one clause of three was already paid`
~~`amx --help` never mentions `--remote`~~ — it does, and did before this row
was written: `8e508c1` declared the flag documentary in clap with the reason,
and it is an ancestor of `b727786`, the tree this register re-verified against.
Confirmed against the shipped binary at `6f4fb5d`: `amx --help` prints
`--remote <HOST>`. R-M4-6 records this as the first of the three already-paid
rows.

The two clauses that are real are M4's X19. A newline-containing session name
cannot cross to a csh login shell (needs a wire encoding — interface
change, so decide, don't drift). `$MISE_INSTALLS_DIR` is deliberately
unread, so a relocated mise root misclassifies as Standalone (one `Env`
field + one `pm::classify` param when it matters).

**DR-18 — No release channel exists.** `open (R-M3-4 standing)`
The update machinery is proven end to end (sha256, swap, handoff, live
smoke 0.1.0→0.1.1 over a `file://` channel) but the default channel 404s.
Fine until the first external user; the risk stays "a stub read as a
service".

**DR-19 — Recorded flakes, unowned.** `resolved (M4, X04)`
Four owners, three code changes, and two rows that were not what they said.

- `flow_control::two_clients_at_different_speeds` — **fixed**; the paced reader
  was only slow while the round was faster than its nap.
- the `urandom` 8 MiB threshold — **half stale, half fixed**. `f09a87c` had
  already replaced the fixed *window* with a wait and **is** an ancestor of
  `18c9261`, this register's re-verification, so "fails under 8-way load every
  time" was recorded against a tree where it no longer did. The fixed
  *quantity* was real; X04 replaced it with rate-over-observed-time.
- `agent_verbs` — **already paid**. `aba0877`, *wait for the fact, not for the
  call that starts it*, fixed both named sites; it is **not** an ancestor of
  `18c9261` (both branch from `08a4257`), so the register could not have seen
  it. Re-measured at the reproduction that once gave 22/320: nothing left.
- `_hook`'s BrokenPipe self-race — **fixed**, and it did not reproduce in 18
  runs. The mechanism was not in doubt, so the write tolerates `BrokenPipe` and
  a new case forces the race every time.

**Two more, in the same file, that this register never named**:
`no_client_grid_is_corrupted_after_coalescing` (16 failures in 16 runs under the
pinned load) and `server_memory_is_bounded_under_a_stalled_client` — same
mechanism, a wall clock standing in for a rate. Recorded as found: a row struck
only for what it listed would leave those two looking like they had always been
green. Verification is per-suite at the load each was reproduced under
(`taskset`-pinned, 8 copies × 8 threads, 160 runs each): 0 failures.

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

Budget snapshot after M4 wave 1 (`6f4fb5d`): 0 files over hard; the four `src`
files this snapshot named over soft were all split by X02 before the waves
pressed them (`pane_host/parser.rs`, `fusion/tracker.rs`, `pane_host/mod.rs`,
`dispatch/agent.rs`, plus `cli.rs` and `control/agent.rs` ahead of growth).
`todo!()` count in src: **0**, and `tests/hygiene/unfinished.rs` fails if it ever
stops being 0, rather than leaving it to a snapshot in a note.

## Suggested order (post-M3)

DR-15 (hours, do first — stale prose misleads every next task) → DR-6 →
DR-16's error-code decision (wire-adjacent) → DR-7/DR-9/DR-10 batched →
DR-19 flake paydown → DR-4/DR-5 written into the M4 plan → D14/D15
implementation (~2 weeks, [10-attention-surfaces.md](../10-attention-surfaces.md))
→ DR-17/DR-18/DR-20/DR-21 as M4 scope decisions.

**Taken, as of M4 wave 1**: DR-15, DR-6, DR-9, DR-13, DR-19, DR-4 and DR-5 are
struck above. DR-7, DR-10, DR-12 and DR-16 are wave 2 (X08, X09, X06); the D14/D15
implementation is waves 3–5; DR-17/DR-20 are X19; DR-18 and DR-21 are declined
with written revisit conditions (11-m4-plan §4). Progress against this order is
recorded per wave in [m4-wave-outcomes.md](m4-wave-outcomes.md), and each wave's
run of the real binary in [m4-live-smoke.md](m4-live-smoke.md).
