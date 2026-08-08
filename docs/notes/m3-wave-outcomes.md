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
is gone.

---

## W03 — M3 contracts

**W02's hand-off is carried.** The `PaneReport` comment now reads as W02 wrote
it, in `actor/panes.rs` — the file the split moved it to.

**`PaneReport::Title` is still open.** W02 flagged, undecided, that the variant
now folds to nothing and nothing reads it. `pane_host/actor.rs` is wave 2's
file; W04 either drops the send and the variant or keeps it as the mailbox
counterpart of a bus event with a deliberately empty arm. W03 did not decide it.

**Three edits outside the §5 scope list, each forced by a field this task
added.** None is behavior; all three are the pass-throughs without which the
tree does not compile:

- `actor/core/persist.rs` and `actor/core/view.rs` copy the workspace
  `worktree` block into the snapshot and into `session.state`. Without them the
  field exists and is unreachable, and `workspace_worktree_block_round_trips_
  snapshot_and_state` would be asserting about serde rather than about amx.
  W12's own scope (`dispatch/workspace.rs`, `core/workspace.rs`,
  `core/restore.rs`) is untouched.
- `amx-client/src/app/wired.rs` gains `Method::SessionHandoff` in the
  "does not change the layout tree" arm of an exhaustive match, and passes
  `generation: None` on its one `stream.bind`. Both are the minimum the new
  row and the new field cost a file W09 owns in wave 4; the reconnect logic
  that will *send* a generation is entirely W09's.
- `crates/amx/src/cmd/viewport.rs` passes `generation: None` for the same
  reason.

**`tests/hygiene.rs` is edited, which W14's scope anticipates.** The seam guard
was in its resting "no seam exists" form and had to be rewritten with M3's
ledger — one row, `session.handoff`, in `dispatch/session.rs`, owed by W06 —
because a helper and the test that bounds it always move together. W14 empties
the ledger and restores the resting form.

**`amx server --handoff-import` was left out on purpose.** §4 names it as
hidden CLI surface but the W03 scope list does not, and it is a flag on an
existing verb rather than a routing arm. **W07 adds the argument to `cli.rs`'s
`server()` itself**; no wave-3 peer touches that file, so it is safe. Every
other M3 verb — `update`, `work`, `layout`, `apply`, `_bridge`,
`_handoff-caps` — is planted, routed, and refuses by name from the real binary.

**`session.handoff` carries no `Core` command yet, deliberately.** A
`SessionCall::Handoff` variant would force an arm in `actor/core/route.rs`,
which is not W03's file and has no behavior to put in it. W06 adds the mailbox
variant with the orchestrator that answers it; until then the seam replies
without reaching `Core` at all.

**One split more than the two the plan named.** `amx-vt/src/snapshot.rs` was at
494 lines and the generation seed pushed it to 518, so it became
`snapshot/{mod,publish}.rs` — the published value and the parser's end of the
double buffer. The R-M1-3 rule ("no split waits for the hard limit") applied to
a file this task grew rather than one it found over.

**A latent bug the seed would have shipped.** `Snapshots::publish` decided
"this is the first frame, force a full one" by testing `generation == 0`. A
seeded counter makes that false on a fresh buffer, and the first frame after a
handoff would have published blank rows for every undamaged row. The flag is
now `published`, tracked separately; `Snapshots::new` is unaffected either way.

**`HistoryTracker::resume` needed one behavior change, not just a
constructor.** A tracker's proof that history is what it thinks it is comes
from an anchor it placed itself, and a tracker handed an inherited terminal has
none — so its first observation read as a `Clear`, invalidated every row the
manifest had just carried across, and rebaselined the id space above them.
`adopting` makes the first look adopt what it finds. Provably load-bearing:
disabling that one branch turns
`a_tracker_resumed_at_head_floor_commits_the_next_row_id_contiguously` red with
`Invalidated { from_row: 2233, cause: Clear }`. `HistoryTracker::new` takes the
same branch and is unchanged by it — floor and issued are both zero there, so
the rebaseline it replaces moved nothing and announced nothing, and
`a_fresh_tracker_still_starts_at_zero_and_announces_nothing_on_its_first_look`
pins that.

**The bridge skew row is a tripwire, not a run.** §4's law asks for "a bridge
transport case in `tests/skew.rs`", and `amx _bridge` is a W11 stub, so the
case cannot run the conformance table over it yet. Rather than a skip that
would sit green forever,
`the_bridge_transport_row_is_planted_and_fails_when_the_splice_arrives` asserts
the stub's refusal by name — so **it goes red the day W11 writes the splice**,
and finishing the row (`for &method in Method::ALL` over a `Wire` on the
child's stdio) is the only way past it. W11's own acceptance name for the
finished row is `every_skew_sample_row_answers_over_the_bridge_transport`.

**Budget watch.** `amx-core/src/state/session.rs` is at 499 after
`set_worktree`, and `tests/hygiene.rs` went 501 → 522 (already over before this
task, and the seam ledger is what grew it). No `src/` file is over the soft
budget; the next task to add to either of those two should expect to split
rather than trim.

---

## W05 — SCM_RIGHTS transport

**`handoff/protocol.rs` became `handoff/protocol/`, four files.** The §5 scope
names `handoff/{fd.rs,protocol.rs}`. Written as one file the protocol lands
around 900 lines — the line codec and the token, the stage vocabulary and the
error type, and two state machines — so it split by responsibility on arrival
rather than at the hard limit (R-M1-3): `protocol/{mod,wire,exporter,importer}.rs`,
466/233/264/271 lines. W03's `handoff/mod.rs` is untouched; `pub mod protocol;`
resolves to the directory unchanged, and W04's two files are not in it.

**The one-byte pane index is a 256-pane cliff, not the absence of one.**
D-M3-6 point 3 says per-pane messages have "no session-size cliff", and the
payload it specifies — one byte — addresses 256 entries. herdr's cliff moves
from 64 to 256; it does not go away. `fd::MAX_PANES` names it and both machines
refuse past it (`HandoffError::TooManyPanes`) rather than wrapping into a
silently mispaired descriptor, so the failure is a message and not a pane
handed to the wrong terminal. Widening the payload is one constant and one
`u8`; nothing above the transport reads the index.

**Both machines are blocking, and W06/W07 own the `spawn_blocking`.** Ancillary
data has no async surface in rustix and tokio's `UnixStream` cannot be given a
`recvmsg` without draining its own buffer first, which is exactly the read-ahead
this protocol must not do (below). The transitions are ordinary blocking calls
bounded by `SO_RCVTIMEO`/`SO_SNDTIMEO`; the orchestrator and the importer
assembly are the ones that must not call them on a runtime thread.

**Line reads peek, because a buffered reader would eat descriptors.** A
descriptor rides beside a byte, and anything that consumes that byte with an
ordinary `read(2)` gets the byte while the kernel closes the descriptor —
silently, with no error on either side. `wire::read_line` therefore `MSG_PEEK`s
for the newline and then consumes exactly up to it, never one byte further.
`UnixStream::peek` is still unstable (rust-lang#76923), so it is rustix's `recv`
with `RecvFlags::PEEK`. Any future edit that puts a `BufReader` on this socket
breaks the descriptor stage without failing a line test.

**A failed transition tells the peer before it dies.** Consuming `self` is what
makes an illegal ordering unrepresentable, and it also means a fault leaves no
state machine to abort from afterwards. So the locally-detected faults — out of
order, malformed, over the cap, mispaired index, too many panes, a bad token —
write the `abort` line themselves on the way out. Timeouts and closed sockets do
not: there is nobody to tell.

**Three things W06 and W07 have to supply that the protocol deliberately does
not.** (1) The manifest crosses as an opaque `serde_json::Value`; W04's codec
serialises into it and the importer checks its own read window — this layer
never reads a field. (2) `Importer::validated(panes)` takes the descriptor count
*from the manifest the caller just checked*, and that number is what
`recv_masters` will accept; getting it from anywhere else reopens the pairing
the index byte closes. (3) `Timeouts::socket_free` is §3's 5 s constant and the
protocol never uses it — the probe-loop between `restored` and `ready` is W07's,
and `Importer<Binding>` is the state it belongs in.

**What `Ending` is for.** §3's crash table has exactly two survivor states and
`HandoffError::ending()` computes which: everything before the commit is
`Abort`, and the only stage past it is the advisory ack. W06's abort matrix and
W07's strict abort can both assert against it instead of re-deriving the table.

**One test file over the soft budget.** `crates/amx-server/tests/handoff_protocol.rs`
is 779 lines and joins the sixteen already over. The crash-table test alone is
seven rows, each of which has to script a peer up to its stage before killing
it; splitting it would move the shared `Fake` and rig into `tests/support/`,
which is W14's call if it wants one.

**Killed peers are killed, not dropped.** The table talks about processes dying
— "its fds close with the process (kernel)" — so every crash row moves its
socket into a real `/bin/sh` child's stdin and `SIGKILL`s it. A dropped
`UnixStream` would have proven something weaker and read the same.
