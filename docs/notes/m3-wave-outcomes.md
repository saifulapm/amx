# M3 wave outcomes

Written by each wave task as it lands, so W14 folds facts into
[09-m3-plan.md](../09-m3-plan.md) §6 rather than reconstructing them. Only
divergences and hand-offs belong here; a task that landed exactly as its §5
entry describes writes nothing.

---

## W02 — One publisher per event kind

**R-M3-9 is withdrawn: 04 §2 needs no doc PR.** The plan records the fix as
contradicting 04 §2's "one publisher" wording and asks for a one-line
architecture change. 04 §2 has no such sentence. What it says about the bus is
that "every state transition … is one typed event with a monotonic sequence
number" — a claim about the *event*, not about who constructs it, and one that
the per-kind rule satisfies exactly (it is the duplicate publishing that broke
it). The absolutist reading existed only in code comments, which is where W02
fixed it: `actor/core/report.rs`, `amx-core/src/event/mod.rs`,
`actor/agent_hub/mod.rs`, `tests/hygiene.rs`, `amx-server/tests/agent_hub.rs`.
Nothing in `docs/04-architecture.md` was touched, and nothing needs to be.

**One declared hand-off to W03.** The wave-1 file-ownership resolution gives
W03 `actor/mod.rs` whole, so W02 did not edit it. The stale comment is on
`PaneReport` (now `actor/mod.rs:115-119`) and reads:

> Reports are facts about what already happened; the `Core` turns them into
> bus events and effects. The pane actor never publishes to the bus itself —
> one publisher per transition is what keeps sequence numbers meaningful.

The last sentence has been false since the pane actor started publishing, and
is now the opposite of the rule. It should become:

> Reports are facts about what already happened, and the pane actor has
> already published every one of them: what `Core` takes from a report is the
> fold beside it, never a second announcement (`docs/09-m3-plan.md` D-M3-2).

W03 carries it in the split commit.

**`PaneReport::Title` now folds to nothing and nothing reads it.** `Core`
answers no question about a pane title — no state row carries one — so with
the republish gone the arm is empty. The report is still *sent*, from
`pane_host/actor.rs`, which is wave 2's file and not W02's to edit. Either
W04 drops the send and the variant, or the variant stays as the mailbox
counterpart of a bus event and the arm stays empty on purpose. Flagged, not
decided.

**What the duplicate was worth, measured.** Against the pre-W02 tree,
`a_pane_transition_publishes_exactly_one_event` sees two `pane.title`
announcements for one title change, and `a_pane_report_folds_without_publishing`
sees six events published by six folded reports. Both are one after the change.
The replay ring (`DEFAULT_REPLAY_CAPACITY`, 1024) therefore covers twice the
history it did for the resync W08/W09 build on.

**A budget warning W02 added rather than removed.**
`crates/amx-server/tests/core_panes.rs` went 326 → 629 lines, which is over
the soft 500 and under the hard 1000. The two tests sit there because the
duplicate only existed where a real `Core` met a real pane, and that harness —
a `Core` on the real actor loop with a mailbox the test sizes itself — is
inline in that file and load-bearing for its three deadlock tests. Splitting
it means lifting the harness into `tests/support/`, which is a refactor of
timing-sensitive tests W02 has no reason to risk. It is now the fifteenth test
file over the soft budget; if the wave that next touches it wants the split,
`support/core_rig.rs` is the shape.

**For W01 and W06, from reading rather than from measuring.** The obvious
Core↔PaneHost shutdown deadlock — a pane actor parked in
`Actor::report(…).await` on a `Core` mailbox nobody drains again, unable to
reach the `cancel` arm of its own `select!` — is already closed:
`Core::run` drops the mailbox receiver *before* `join_panes()`, so a
mid-report send fails rather than waits (`actor/core/mod.rs:380-385`, with the
comment saying exactly that). Whatever the drain wedge is, it is not that.
W02 changes nothing on that path: reports still flow, only the second publish
is gone. W01 confirmed it from the other side.

**W01 closed one drain mechanism and left one open — read its note, not its
commit titles.** [m3-shutdown-wedge.md](m3-shutdown-wedge.md) §4 and §8 are the
authority, and the short version is that "wedge fixed" would be the wrong
summary. What is *closed* is a connection whose handshake watched no
cancellation token: real, reproducible on demand, fixed. What is *open* is the
mechanism that actually parked the wedged servers found on the machine — their
stuck connection has no peer at all (`ss -x` peer inode 0, and a peerless unix
socket was measured to return EOF on read and EPIPE on write), so the handshake
cannot be where they sat. The remaining suspect is the connection epilogue,
`ConnEvents::shutdown` and `ConnStreams::shutdown`, both in W08's
neighbourhood; every await in both selects on the connection's token, and
neither 800 kill-then-stop rounds nor ~18,000 storm cycles produced an
occurrence. Genuinely open, not merely unexamined. **W06 therefore keeps its
post-commit drain watchdog** — §2's outcome (b)/(c) branch is the one M3 is
on, not (a). `scripts/spike/wedge.py --suites field` is the loop if anyone
wants to keep one running.

**W02 and W01 merge clean and green, checked rather than assumed.** The two
branches share `tests/hygiene.rs` — W01 pins its CLOEXEC fix there, W02 the
pane-publisher rule — and the additions do not touch each other. Merging
`origin/worktree-w01-wedge` (at `ee677f0`) into `worktree-w02-publisher` needs
no conflict resolution, and `scripts/ci.sh` on the merged tree exits 0, 648
tests, three runs. W01's `Runtime::spawn` rename reaches no file W02 edits.
W01's tip has since moved to `ffc3abb`; the delta is its own note, its spike
script, and seven lines of comment in `runtime.rs` — checked, not taken on
trust — so the merge stays conflict-free and the three green runs remain the
evidence for it.

**A pre-existing test race the merge check surfaced, owned by neither branch.**
`hook_exits_zero_and_fast_with_no_socket_no_env_or_dead_server`
(`crates/amx/tests/hook.rs:476`) failed once during a full parallel CI run with
`BrokenPipe`: the test writes its payload to the stdin of an `amx _hook` whose
whole contract is to exit immediately, so a child that wins the race is the
test's success condition and its `write_all` failure at the same time. Neither
branch touches `_hook`, its code path, or the part of
`crates/amx/tests/support/` it uses. Measured under 16–20 spinning hogs: 1
failure in 115 runs on the merged tree, 0 in 50 on the untouched base — the
difference is noise, not attribution, and the mechanism is visible in the test
source. Recorded so the next person who sees it red does not go looking in the
publisher change or the wedge fix. The repair, when someone owns that file, is
to tolerate `BrokenPipe` on the payload write rather than to slow the emitter
down — slowing it would fix the test by breaking the thing it measures, and the
race gets *worse* as the emitter gets faster, which is the direction that code
is meant to move. Confirmed by reading from the W01 side and carried in
[m3-shutdown-wedge.md](m3-shutdown-wedge.md) §8 as well; that entry and this
one are the same finding.

**Sequential fills inherited from W01, repeated here so the ledger carries
them.** `conn/mod.rs` is W08's file and W01 has landed fifteen lines on it (the
handshake under cancellation); `session/serve.rs` is W06's and now carries the
task names. Both are declared in W01's note — whoever picks up W08 or W06 is
filling beside a change, not discovering one.
