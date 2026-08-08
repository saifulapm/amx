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

## W04 — Handoff manifest

**`PaneReport::Title` is dropped: the variant, the send, and the arm.** W02
flagged it and W03 left it open; the answer is the one D-M3-2 already implies.
A report exists to give `Core` a *fold*, and there is none — no state row
carries a pane title, `Event::PaneTitle` already tells every client, and the
pane actor is the publisher of it. What the message actually cost was a `String`
clone and a **blocking** `send().await` into a bounded `Core` mailbox on every
OSC 2, which is a shell prompt away from being per-command. Damage avoids that
hazard with `try_send` deliberately (D-M3-2); the title had no reason to take it
at all. If a title ever lands in `session.state`, the report comes back with the
fold that needs it, which is a smaller change than the one being avoided.

**The export answers with the descriptor, not just the entry.**
`PaneCommand::ExportHandoff` replies `PaneExport { manifest, master }`, and the
parser thread asks the pty actor for the duplicate **first**. That is what makes
"quiesced only" structural rather than advisory: the pty actor hands a
descriptor out in no other state, so a running pane is refused before a single
row is read, and there is exactly one owner of the answer. It also spends one
round trip instead of two on a state that must not move in between. **For W06:**
this is where §3 step 8's fd comes from — no other path to a pane's master
exists — and `PaneHost::quiesce()`/`resume()` (new, bypassing the mailbox like
`kill`, blocking for the drain) are the pair that gets a pane into and out of
the state that allows it.

**The cursor is applied after the modes, not with the grid.** Read literally,
D-M3-4's "rows → grid → modes" loses the cursor: replaying DEC mode 6 homes it,
so a position written with the paint is thrown away by the first origin-mode
sequence that follows. `PaneManifest` therefore carries `cursor` as a field of
its own and `PaneSeed.modes` ends with it. Caught by
`modes_survive_the_round_trip` going red, not by reading the spec.

**The alternate-screen switch belongs to the paint, not to the modes.** The
published snapshot is the *active* screen's, so the screen has to be selected
before the grid is painted onto it; `?1049h` rides a small paint prologue
together with the four modes a faithful paint needs held (wraparound on, origin
off, insert off, synchronised output off) and grapheme clustering set to what
the pane actually had. The rest of the modes follow the paint and are what put
the forced ones back.

**The successor's eviction floor is derived, not carried.** What a resumed
tracker must get exactly right is the *head* — that is where the next committed
row id comes from — and the head is the floor plus whatever actually landed in
this terminal's scrollback. So the parser replays the carried rows, feeds line
feeds until the scrollback has swallowed them, and only then builds
`HistoryTracker::resume(head, head − landed)`. Rows the replay could not fit sit
below that floor: evicted in the ordinary sense, never renumbered. The feed also
produces the **blank screen** the paint needs, which is why nothing clears one —
`Terminal.eraseDisplay` turns `ED 2` into a *scroll* when the last non-empty row
looks like a prompt, which would push rows into the scrollback and move every id
the manifest had just carried across.

**R-M3-2: two cell classes cannot round-trip through the C API.** Both were
found by the property test rather than reasoned about in advance, and both are
in `handoff::grid`'s module docs and in the acceptance test's comparison, named
and narrow, rather than hidden behind a looser assertion:

- **A blank column carrying SGR attributes** crosses as a *space* carrying the
  same attributes. `Screen.blankCell` returns `style.bgCell()`, so an erase can
  colour a column and can never underline one, and no other sequence writes a
  column without writing a character. The space renders identically and loses
  the never-written bit; dropping the attributes instead would lose a visible
  underline. The state is reachable only by mutating a grid after the fact.
- **A spacer head is re-derived, not carried.** Its paint is set by the print
  that wraps past it and by nothing else, so a head whose paint was left over
  from an earlier print comes back with the wrapping character's paint, and a
  head whose wide character has since been erased comes back as a blank column.
  Every sequence that could touch the cell afterwards clears it outright —
  `Screen.splitCellBoundary` erases a spacer head whenever the wide character
  below it is disturbed. The wrap flag itself always survives, and that is what
  the test refuses to tolerate a difference in.

**A live bug a naive SGR emitter would have shipped.** `Parser.zig:203` sets
`MAX_PARAMS = 24` and drops a CSI **outright** once the sequence fills it — not
the overflow, the whole sequence. A cell wearing every attribute plus three
direct colours needs 26 parameters, so it would have replayed with *no* styling
at all. Pens are emitted as however many sequences it takes (16 parameters
each); SGR accumulates, so only the first carries the reset. Found by the
property test at ~1900 cases in; the acceptance run does 256 and a soak of
150 000 is green.

**Other bounds, stated in the module docs rather than discovered later.** A pane
on the alternate screen crosses with a **blank primary** — only the active grid
is published and the C API offers no way to read the other one, though its
scrollback still crosses. A **scrolling region (DECSTBM) does not cross**: the
terminal exposes no accessor for it. A **palette-indexed underline colour** was
already flattened to "none" by `amx-vt` before this module sees it.

**The packed rows ride base64.** The manifest is one JSON line and the M1 row
packing is binary; base64 is forty hand-rolled lines against a serde byte array
that would have cost ~4× on the wire, and no dependency either way. Everything
else in the entry is text, including the synthesized grid — every byte of it is
an ASCII control sequence or a cluster the library already gave us as UTF-8. The
256 KiB budget is measured on the **packed** bytes, before that encoding.

**Edits outside the §5 scope list, each forced by the deliverable.**
`actor/panes.rs` (the `ExportHandoff` command and the removed `Title` report —
W03's split moved `PaneCommand` there after §5 was written), `actor/core/report.rs`
(the arm the removed variant left behind), `actor/mod.rs` (re-exporting the two
new types), `pane_host/mailbox.rs` (the `ParserCommand::Export` variant — the
parser's vocabulary lives beside it, not in `parser.rs`), and
`tests/{mailbox,core_panes}.rs`, which both listed the removed variant. No
wave-2 peer touches any of them.

**Budget.** `handoff/grid.rs` is 601 and `handoff/manifest.rs` 592 — over the
soft budget, under the hard one, and **not split**: the scope forbids editing
`handoff/mod.rs` beyond the two module declarations W03 planted, and a third
file needs a third declaration. If a later wave wants them split, that one line
is the whole cost. `pane_host/mod.rs` *was* split — `export.rs`, the handoff
vocabulary, beside `mailbox.rs`'s parser vocabulary — because that file is W04's
to declare in; it went 518 → 474. `pane_host/parser.rs` is at 532, and the
adoption is what grew it.

**For W07.** `PaneHostConfig::seed` is the whole import surface: give it
`PaneManifest::seed()` and a pty session over the received descriptor, and the
pane comes back with its screen, its scrollback, its modes, and all three
counters continued. The session-level `Manifest` is defined with D-M3-5's
inventory (`session`, `seq`, the persist `Snapshot`, `panes` in transfer order,
`agents` as `AgentSnapshot`s) and `Manifest::check_version` implements D-M3-6's
*window* check; W06 fills the session-level fields, W07 reads them.
