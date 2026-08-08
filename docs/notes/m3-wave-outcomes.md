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

## W13 — Layout export/apply

**`session.state` does not carry a pane's cwd, so an export cannot.** D-M3-11
says the reply "already carries the workspace list, BSP trees, cwds, labels, and
agent kinds". Four of those five are there; the cwd is not.
`amx_core::state::Pane` holds one and the persist snapshot writes it, but
`control::session::PaneState` has no such field and `core/view.rs` never
projects one — so a client cannot read a cwd at all, and export writes none.

The layout file has a `cwd` key regardless, and apply honours it on every pane:
a file a person writes is the main thing a `cwd` is for, and a format missing
the key would have had to grow one later anyway. What is missing is only the
export half, and it is missing by one additive optional field —
`PaneState::cwd`, under the same R-M1-8 terms as `label` and `agent` beside it,
plus the `core/view.rs` line that fills it and a regenerated `session.state`
golden. **Hand-off:** that is proto and server work, which W13's scope
explicitly excludes ("no server code and no new wire surface"), so it is left
for whoever owns the next `session.state` change — W14, or a follow-up. Until
then `amx layout export` writes shape, labels and agent kinds, and a
round-tripped layout starts its panes wherever the server's default cwd is.

**Three verb-shaped gaps the plan does not mention, and what apply does about
them.** None is a divergence from D-M3-11 — the decision says "replays it
through the public surface", and this is what that surface costs:

- `workspace.create` takes no cwd, so a workspace's *first* pane cannot be
  given one directly. When the file asks, apply splits the root with the cwd it
  wants and closes the root under it — two calls to say what one parameter
  would, and the tree ends identical.
- `agent.start` picks its own slot (it splits the workspace's focus,
  `dispatch/agent.rs`), so an agent leaf cannot be created where the file puts
  it. Apply builds a placeholder there, starts the agent, `pane.swap`s it into
  the placeholder's slot and closes the placeholder — which collapses the
  temporary split and restores the tree exactly.
- `pane.split` always cuts at 0.5, so ratios are reached with `pane.resize`
  immediately after each split, while both children are still leaves and the
  nudge therefore lands on the split just made.

**Ratios are written to three decimals, and that is what makes the round trip
byte-exact.** Apply reaches a ratio by nudging a half-split by
`ratio - 0.5`, which lands within an f32 rounding error of the written value
rather than on it. Three decimals is a tenth of a percent of a workspace —
finer than a cell — and rounding to it makes the second export write the same
characters as the first. The file also omits `ratio` entirely at 0.500, so an
unresized layout says nothing about ratios at all.

**Export reads the layout tree through its serialized form.** `amx_core::Layout`
keeps `Node` private and exposes only `panes()`, `rects()` and `zoomed()` —
none of which recovers the axis and ratio of an internal node. Rather than
widen `amx-core` (not W13's file), `build.rs` deserializes the tree's own wire
shape, which is frozen and golden-pinned as the thing clients mirror (04 §3).
If a later task wants a public visitor on `Layout`, this is the caller that
would use it.

**Three edits outside the §5 scope list.** `crates/amx/src/lib.rs` gains
`pub mod layout;` — a module cannot exist otherwise. `crates/amx/Cargo.toml`
gains `toml` (`parse` + `serde`, both already in the lock) and the `derive`
feature on the `serde` it already had; no new dependency enters the tree, and
`toml`'s `display` feature is deliberately *not* taken, because export renders
its own TOML so that key order, the header comment and the omission of defaults
are the format's design rather than a serializer's. **W10 touches both files
too** (its own `pub mod update;` and its `sha2` line): the two edits are
additive and adjacent, so the wave-2 merge should expect a two-line textual
conflict and nothing semantic.

**A second test file, for the budget.** The five acceptance names plus the two
that pin the nudge table and the root-replacement rule came to 550 lines in one
file. Split by responsibility rather than left over the soft budget (R-M1-3):
`crates/amx/tests/layout.rs` (359) drives the real binary against a real
server, `crates/amx/tests/layout_file.rs` (205) is the pure pair of
translations and needs no session — which is also the only way the agent path
can be tested at all, since a real agent binary does not exist on a runner
(R-M2-8). `agent_kinds_apply_but_session_refs_never_export` lives in the
second file for that reason.

**The round-trip test needs a session with no workspaces**, which is why it
starts servers with `amx server` rather than `amx attach`: an attaching client
seeds a first workspace (`Session::attach`'s `attach: true`), and a layout
applied on top of one would export as itself plus somebody else's shell. Apply
connects with `attach: false` for the same reason.

**Not exported, deliberately, beyond the refs D-M3-11 names.** Focus and zoom
(where a person is looking, not how the session is built), and the `worktree`
membership block W03 added — `amx work` owns that association, and a layout
replayed on another machine would name a checkout that is not there. `command`
has no key either: D-M3-11's list does not include it, and `session.state`
could not export one.
