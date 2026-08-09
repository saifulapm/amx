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

**M4 waves 3 and 4 (2026-08-09, `b698c51`).** D15's three surfaces and D14's two
landed: `agent.list`, the status-line breakdown, the narrow projection, the mouse
path, scoped `next-attention`, the agents view, the peek and `amx agents`. They
carry no register row of their own — the D14/D15 implementation never did — so
what changed below is DR-1's residual (still X18's), DR-11's count, DR-19's
addendum, and the budget snapshot. Both waves were smoked in one run against the
real binary ([m4-live-smoke.md](m4-live-smoke.md) §4, §5), which found five
things nothing in this register predicted; they are listed at the end.

**M4's exit (2026-08-09, `ad4b44b`).** All twenty implementation tasks landed and
[§7](../11-m4-plan.md#7-the-m4-exit) was run item by item
([m4-live-smoke.md](m4-live-smoke.md) §6). **The milestone's work is done and its
exit criteria are not met**: the CI half holds except a queue-head age that stops
advancing on a quiet screen, the agents view's jump, and one frozen field with no
reader; the by-hand half's item 1 fails on its substance — a real Claude Code
blocked on a real permission dialog is reported `idle` 35.4 s later with the
dialog still on its screen — and its items 2 and 3 are unrun and recorded as
unrun. **None of what fails is a row this register named**, which is the case for
having kept both instruments: the review found execution gaps and process debt,
the smoke found behaviour. The rows the exit moved are DR-1's residual and DR-17
(struck), DR-11's count, DR-19's fifth flake, DR-21's measurement, and the budget
snapshot.

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

**DR-1 — The `Cells` wire format is the missing keystone.**
`resolved (M3); its residual resolved (M4, X18)`
The codec is real: length-prefixed `put_cells` / `read_rows`
(`amx-proto/src/stream/grid.rs:242,379`) over a per-cell layout in
`amx-proto/src/stream/cell.rs`; no `todo!()` remains in amx-proto.
Residuals, both closed in M4: the stale comment at
`amx-client/src/model/grid.rs:33` was corrected by X03 under DR-15, and the gap
it then stated — the client's `Attrs` carrying six of the wire's ten
attributes — is gone. **X18 widened it to all ten**, with the underline *style*
carried as the wire's own enum and emitted in the sub-parameter form
(`4:2`…`4:5`) the vendored parser reads, and the underline colour as `58;2;r;g;b`.
Two reductions remain and neither is an attribute: a palette-indexed underline
colour never reaches the wire (an `amx-vt` limitation, recorded there), and a
cell's text is one `char`, so a multi-codepoint grapheme keeps its first scalar.
Both are named in `Attrs`' own doc rather than left for a reader to find. Watched
end to end at the exit: the widest SGR run a stand-in can paint arrived through a
**peek** of a pane in another workspace, underline and both 24-bit colours intact
([m4-live-smoke.md](m4-live-smoke.md) §6.4).

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
before either half of them is built.

**Whether the practice pays was left to M4's exit, and the exit answers yes —
though not where this row expected.** The merge gate caught four integration
breaks across the milestone, three of them in wave 3 alone and every one between
tasks whose *files never overlapped*, which is precisely the hole this row names;
one of the three (`pane_interior`, defined twice) was hiding a real defect rather
than merely duplicating code. But the merges were the cheaper half. **The live
smoke found more than the merges did** — ten findings with no register row, five
of them behavioural defects in shipped surfaces, and the largest of them
(a blocked agent reported idle after thirty seconds) is invisible to every suite
in the tree and was found because the smoke ran the binary and then *waited*.
The structural fix this row asked for is worth keeping; the part of it that
earned its cost is the standing smoke, not the standing merge.
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

**DR-7 — `GridMessage::Scrolled` is dead wire surface.** `resolved (M4, X08)`
Decided by deletion: the variant, its tag, the codec arms, the golden and the
client's decode arm are all gone, and **tag 2 is retired rather than
renumbered** — an older build is entitled to go on reading 3 as the cursor — with
`decode` refusing it like any other unknown tag and two tests pinning that. The
reason emission lost is structural rather than budgetary and is worth keeping in
the register: a client binds a grid stream only for the panes of its *focused*
workspace, so a commit notice on that stream never reaches a pane in any other
one, and under D14's narrow projection the bound set shrinks to a single pane. A
fact the scrollback cache depends on cannot ride the narrowest channel in the
protocol when `history.committed` already carries it on the widest, with the
bus's ordering, typed `gap` and resumable cursor. What tag 2 alone carried — the
per-row hash — has no client-side reader to compare it against, because a served
row is decoded into a `String` and its packing dropped; that is the revisit
condition, written where the variant used to be. Golden `stream/grid_scrolled.json`
is gone, so the harm this row actually named is paid.

**Owed, and named here because no task owns the file.** 04 §3's fourth
scrollback-identity bullet (`docs/04-architecture.md:112-114`) still says rows
scrolling out "are announced on the pane's delta stream (id + content hash…)".
After X08 that sentence is false, and stale prose in the binding architecture doc
is DR-15's disease at its worst. X03 held 04 in wave 1 and has landed; no wave-3,
-4 or -5 task lists it. The correction is one bullet: the announcement is
`history.committed` on the event bus, with the per-row hash recorded as what the
delta-stream form would have added and the condition under which it returns. The
same paragraph's "content push for panes the client is actively scrolled back in"
is unbuilt and unrelated. Carried as an owed correction rather than taken by the
integration owner, which is a doc-truth task's job and not an integration seam.

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
`resolved (M4, X06 + X09)`
Both halves, split by file ownership as D-M4-9 planned. The shadows are gone:
`agent/fusion`'s went with X06 — the tracker now returns *directives* rather
than effects — and `amx-vt::callbacks`' is `TerminalEvent`, which is what it
always was (X09). `amx-client`'s two booleans are one folded
`amx_core::Effect`, adopted **before** the four surfaces that would each have
set one, which is the whole of why it was scheduled in wave 2 rather than after.
The live evidence that it was the right type is in
[m4-live-smoke.md](m4-live-smoke.md) §3.8, from the opposite direction: the one
client path that does *not* fold its stream frames into `Effect` —
`amx attach --pane`, which binds through the bare `Session::call` — drops the
pane's first keyframe under load and shows a blank terminal, while the full
client, which folds through `App::call`, does not. Same bug class, same
milestone, one path converted and one not.

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
file and any census log line. **Seventeen clean stops across the milestone**, all
exit 0, over sessions of 2, 3, 4, 24, 25 and 26 panes, beside the handoffs whose
successors took the session in every case
([m4-live-smoke.md](m4-live-smoke.md) §1.7, §2.7, §3.9, §4.9, §6.8); no
`drain-census` file and no census line in any server's stderr. Stops of servers a
driver did not spawn are not counted, since their exit status cannot be read.
R-M4-10 is what turns that from an impression into a record, and **the milestone
of field time this row asked for has now passed clean**. The watch's own
condition is met; whether to close DR-11 or keep it through M5 is a decision this
register leaves to the next plan, since the field mechanism was never caught in
the act and a watch that costs nothing is cheap to keep.

**DR-12 — `frame on unbound channel` under flood.** `resolved (M4, X08)`
Decided, and not as a choice between refusal and silence: the two layers answer
differently because only one of them can tell the cases apart. The reader
refuses a channel this connection *never* bound —
`NetError::UnboundChannel` (`amx-client/src/net/read.rs:147-148`), deliberately
not a transport error, so no redial swallows it — which keeps the check that
caught a desynchronised peer (notes/frame-read-cancellation.md). By the time a
header reaches `stream::apply` the reader has already vouched that the
connection bound the channel, so what is left is a route this client let go of,
and dropping that frame is the only answer that does not kill a live session
over a frame that was correct when it was written. Both routes to the drop —
`Bindings::forget_pane` and an inbound frame on a raw channel — are latent
today, and the module header says so rather than claiming a live case. The named
test the row asked for is `crates/amx-client/tests/unbound_channel.rs`, whose
flood half reproduces the actual shape of the 3-in-30 failures — the client's own
read cancelled mid-frame and resuming out of step, so a payload byte was read as
a channel — at 60 frames with every read cancelled at least once, rather than at
whatever load happened to be running. Watched from outside too: a client
attached through an 8 MiB flood stayed alive and kept painting
([m4-live-smoke.md](m4-live-smoke.md) §3.6).

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

**One new instance, from wave 2, recorded under DR-7.** X08's deletion of the
scroll notice made 04 §3's fourth scrollback-identity bullet false. The row is
struck for what X03 fixed, not for the class, and the class keeps producing:
this is the second milestone in which shipped code outran a sentence in the
binding architecture doc.

**DR-16 — Reconnect coverage is uneven (W09's untaken hand-offs).**
`resolved (M4, X09) — two of three; the third declined with a condition`
- `resolved (X09)`: **`attach --pane` reconnects.** It shares the full client's
  redial rather than growing a second one (`reconnect::dial_until`, so both
  agree how long a swap may take), rebinds both streams on the successor and
  re-declares the viewport when it holds size authority. Watched live in
  [m4-live-smoke.md](m4-live-smoke.md) §3.7: the single-pane client came back on
  the successor with the session id unchanged, a sentinel painted *after* the
  swap reached its screen, and `prefix+q` detached it with exit 0.
- `resolved (X09)`: **`NotAccepting` has a code of its own.**
  `RpcError::RETRIABLE = -32001`, the second occupant of amx's own
  −32000..=−32099 range, and the reader is `cmd::call::one_shot` — the redial
  loop every generated and hand-written verb already goes through, which is why
  X09 took `cmd/call.rs` outside its listed scope and said so. The smoke did
  **not** catch the refusal by hand: it needs a call to reach a quiesced server
  rather than a dead socket, and that window is the same milliseconds-wide one
  M3's mid-restore kill could not aim at either. CI owns it; §3.7 records the
  miss rather than rounding it up.
- **Declined, with a revisit condition** (11-m4-plan §4): the bridged (SSH)
  client's redial needs a respawned ssh child, which is `remote/`'s mechanism
  and not this row's. Revisit when a remote attach is a daily path rather than a
  smoke step.

One thing the row did not name and the smoke found: the non-`--takeover`
`attach --pane` can start on a blank screen under load, because it binds through
the bare `Session::call` and the pane's first keyframe is discarded while the
second bind's reply is outstanding. Pre-existing — identical at `6f4fb5d` and
`335cc27` — and unowned. [m4-live-smoke.md](m4-live-smoke.md) §3.8 has the
measurement and the one-line shape of the fix.

**DR-17 — Remote UX edges.** `resolved (M4, X19) — one of three was already paid`
All three clauses answered. The newline one was **decided rather than encoded**:
`SessionName::new` refuses every ASCII control character, with the reasoning
written at both ends of the path, because anything encoded here must be decoded
by a far side of unknown version — and an older one would serve a *different*
session under a name nobody asked for, a silent wrong answer where a refusal is
a sentence a user can act on. That is a stated interface change: a name holding a
control character was legal yesterday and is refused today, in `--session`,
`$AMX_SESSION` and the serde path alike. `$MISE_INSTALLS_DIR` is now read (one
`Env` field, one `pm::classify` parameter, two call sites), so a relocated mise
root classifies as mise and `amx update apply` redirects to `mise upgrade amx`
instead of writing over a managed install — asserted against the real binary from
such a tree, with the variable unset installing as the control. The `--help`
clause needed nothing and is recorded as already paid (R-M4-6).

The row as first written, for the record:
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

**A fifth, found at the wave-3/4 boundary and not in the same file.**
`events_json_resumes_from_its_cursor_or_reports_the_gap`
(`crates/amx/tests/wait_retry.rs`) failed once under parallel-agent load with a
hole at seq 24 and no gap marker. **The defect was the assertion**: half one
asserted contiguity over `seq` alone, and a `gap` delivery carries `from`/`to`
and no `seq` — so the answer the test's own name promises ("*or reports the
gap*") was invisible both to the assertion and to the message it printed. The gap
is legal and, under load, likely: the successor's bus continues the sequence
space with an **empty replay ring** (`Bus::new_at`), so an event published after
the manifest was captured is inside its head and behind its ring. Reproduced
deliberately by pausing a relay across a real handoff (one gap, naming exactly
the missing range), fixed by folding every delivery into the range it accounts
for and requiring those to tile, verified to bite against a mutated relay, and
verified green at X04's load (40 runs, 8 copies × 8 threads, one core).
[m4-wave-outcomes.md](m4-wave-outcomes.md), "the fifth flake".

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

**DR-21 — Resume optimization, recorded not built.**
`open (optional) — now with the measurement §7 asked for`
R-M3-14's sound route: a reconnecting client drains event replay before
binding grids, so unchanged grids skip their keyframes without trusting
generations. Client-side only, no wire change. Worth taking when reconnect
traffic matters (many panes × frequent swaps), not before.
**Measured at M4's exit** ([m4-live-smoke.md](m4-live-smoke.md) §6.8), and the
number argues for leaving it alone: with 25 agents and a peek open the bound set
is **six streams** — the five panes the client draws plus the peek, because D14's
projection binds the drawn set and not the session — so a resume costs six
keyframes and about 6900 cells, not twenty-five panes' worth. The wire figure the
row really wants needs a counter in `damage/keyframe.rs`, which is one line and
is named here so the next person does not have to rediscover that the product
exposes none. What the same measurement *did* find is a bigger number in the
other direction: the agents view repaints every cell four times a second
(`absorb(Effect::Full)`), 82 KB/s at 160×44 against nothing at all with the board
closed — reconnect traffic is not where this client's bytes are going.

Budget snapshot post-M3: 28 files over soft (24 tests), 0 over hard;
largest src `pane_host/parser.rs` 532, then `remote/ssh.rs` 516,
`fusion/tracker.rs` 511, `pane_host/mod.rs` 501 — all with recorded
reasons. `todo!()` count in src: 2, both DR-6.

Budget snapshot after M4 wave 1 (`6f4fb5d`): 0 files over hard; the four `src`
files this snapshot named over soft were all split by X02 before the waves
pressed them (`pane_host/parser.rs`, `fusion/tracker.rs`, `pane_host/mod.rs`,
`dispatch/agent.rs`, plus `cli.rs` and `control/agent.rs` ahead of growth). Two
`src` files went the other way in the same wave and this entry did not say so
when it was written: `actor/core/restore.rs` 499 → 536, which X05 grew, and
`remote/ssh.rs`, still 516 and X19's in wave 5.
`todo!()` count in src: **0**, and `tests/hygiene/unfinished.rs` fails if it ever
stops being 0, rather than leaving it to a snapshot in a note.

Budget snapshot at M4's exit (`ad4b44b`): **35 files over soft, 0 over hard**.
The three `src` files over are `actor/core/restore.rs` 536 (wave 1),
`crates/amx/src/agents/watch.rs` 530 (X16) and `crates/amx/src/remote/ssh.rs`
**559**, which X19 grew from the 516 it had carried since M3 — the only `src`
file wave 5 pushed further over, and the one R-M4-5 named as certain to grow.
Nothing crossed the hard budget in the whole milestone, and the two files that
came closest were split by the tasks that wrote them rather than by a later
cleanup.

Budget snapshot after M4 waves 3 and 4 (`b698c51`): **34 files over soft, 0 over
hard**. Three `src` files are over: `actor/core/restore.rs` 536 and
`remote/ssh.rs` 516, both carried from wave 1, and `crates/amx/src/agents/watch.rs`
530, which X16 landed there. `amx-client/src/app/mod.rs` — the file wave 2's
snapshot named as the next one the R-M1-3 rule would want split, at 498 with four
surfaces still to land in that directory — came out at **470**: X14 moved the
resize debounce into `app/resize.rs` rather than growing it, and
`crates/amx-client/tests/modules.rs` is what caught the overrun. Two suites were
split for the *hard* budget as they landed (`agents.rs` at 1102 lines became
three files), which is the rule biting in the direction it was written for.

Budget snapshot after M4 wave 2 (`335cc27`): 30 files over soft, **0 over hard**.
The two `src` files over are the two wave 1 left there — `actor/core/restore.rs`
536 and `remote/ssh.rs` 516 — both unchanged by wave 2, and every file X02 split
is still under. `amx-client/src/app/mod.rs` is at 498 with four wave-3
and wave-4 surfaces still to land in that directory, which is the next file the
R-M1-3 rule will want split and the reason to say so here rather than at 1000.

## Suggested order (post-M3)

DR-15 (hours, do first — stale prose misleads every next task) → DR-6 →
DR-16's error-code decision (wire-adjacent) → DR-7/DR-9/DR-10 batched →
DR-19 flake paydown → DR-4/DR-5 written into the M4 plan → D14/D15
implementation (~2 weeks, [10-attention-surfaces.md](../10-attention-surfaces.md))
→ DR-17/DR-18/DR-20/DR-21 as M4 scope decisions.

**Taken, as of M4's exit**: DR-1's residual, DR-4, DR-5, DR-6, DR-7, DR-9,
DR-10, DR-12, DR-13, DR-15, DR-16, DR-17 and DR-19 are struck above — **the whole
of the order**, including the D14/D15 implementation it ends with. What is left
of this register is **one open row** (DR-20, whose two clauses need a second
machine and have written procedures), one declined-with-a-condition (DR-18), one
`open (optional)` with its measurement taken (DR-21), one `watch` whose condition
is now met (DR-11, seventeen clean stops) and one `no action` (DR-14). Twenty-one
rows in, four out, and three of the twenty-one turned out to have been paid
before they were scheduled — which is R-M4-6's whole argument, re-earned three
times.

**The register is not the exit.** M4's §7 criteria are *not* met
([m4-live-smoke.md](m4-live-smoke.md) §6.9), and none of what fails is a row this
register named: the review found execution gaps and process debt, and the smoke
found behaviour. Both instruments were needed and they found different things,
which is the case for keeping both. Progress is recorded per wave in
[m4-wave-outcomes.md](m4-wave-outcomes.md), and each wave's run of the real
binary in [m4-live-smoke.md](m4-live-smoke.md).

**Ten findings this register has no row for**, none a regression, none
blocking M4, every one outside every remaining task's file scope. From the
wave-2 boundary: a session can be handed over only once (the importer assembles
no export path), and the non-`--takeover` `amx attach --pane` can start blank
under load. From the wave-3/4 boundary:

- **a blocked agent goes idle 30 s after its last paint** and leaves the
  attention queue, while its dialog is on screen and the manifest rule still
  matches it — the largest of the five, because D15's whole subject is who is
  waiting and the phone case is the one with nobody attached to keep repainting
  (m4-live-smoke §4.8);
- **the agents view's `Enter` lands on the workspace's remembered pane**, not on
  the selected agent (§5.5);
- **the status line's queue-head age advances only when something else
  repaints** (§4.4);
- **`prefix+d` does not reach a client with the agents view open** (§5.5);
- **the board's filter survives closing and reopening it** (§5.3).

And three more from the exit itself:

- **the shipped `claude.toml` sees only one phrasing of a permission dialog** —
  `contains = ["do you want to proceed?"]`, which a Write/Edit dialog does not
  say, so for that whole class tier 2 has no opinion and `agent explain` answers
  `matched: null` with a dialog plainly on screen (m4-live-smoke §6.8). It is
  what makes the first finding above unrecoverable rather than merely wrong, and
  it is DR-14's "manifest-catalog maintenance burden carries over" with a date
  on it;
- **the agents view repaints every cell four times a second** —
  `apply_agent_list` ends in `absorb(Effect::Full)`; 82 KB/s at 160×44 and
  9 KB/s at the phone width D14 exists for, against *nothing at all* with the
  board closed (§6.8);
- **`pane.run` did not submit to a real Claude Code composer**, 3 of 3, and a
  following `pane.send-keys enter` did (§6.8). DR-3 records the mechanism and
  cites ~3% for the swallow it redesigned around; this is the same shape at a
  rate nobody has measured against a real agent since.

All ten are written up in [m4-wave-outcomes.md](m4-wave-outcomes.md) under the
boundary that found them, with mechanisms and citations. They want plan decisions
rather than rows invented by the integration owner — and the first two want one
**before M4's exit criteria are read as met**, because they are why by-hand item
1 fails and why every one of §7's items 1–5 has to be measured inside thirty
seconds of a paint to hold at all.
