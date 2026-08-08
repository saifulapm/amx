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

## W10 — Self-update

**The dormant path, named exactly.** `crates/amx/src/cmd/update.rs` carries
`pub const HANDOFF_AFTER_INSTALL: bool = false`, and `hand_over()` beneath it is
the whole of D-M3-8's second half: `session.handoff` with the staged binary,
then a reconnect-poll that treats *same `SessionId`, different server version*
as success and the old version answering again as the abort path having run.
It compiles and typechecks on every build and is not reached. **Turning it on is
flipping that one constant to `true`** — nothing else, no feature, no argument.
A cargo feature was considered and rejected: code excluded from the build is
code that rots between the wave that writes it and the wave that enables it,
and this glue has to survive three waves.

`apply_says_plainly_that_the_running_session_is_not_handed_over_yet` is the
tripwire in W03's style: it asserts the constant is `false` *and* that `apply`
prints the sentence saying panes are still on the old binary. Both halves go red
the day W14 flips it, which is the point — the sentence must be replaced by an
assertion that the successor answered, not merely deleted.

**Three files outside the §5 scope list, each forced by the config field.** The
field itself (`[update] channel`) is in scope; these are the places the tree
does not compile without:

- `amx-core/src/lib.rs` re-exports `UpdateConfig` beside the other section
  types.
- `amx-core/tests/config.rs` builds `Config` with an exhaustive struct literal
  and pins `SECTIONS` as a literal list, so both had to learn the third section.
  Its `diagnostics_name_the_failing_section` text gained a bad `[update]` line
  too, or the section would have reset to its default while the test asserted
  nothing moved.
- `amx-server/tests/config_reload.rs` has one exhaustive `Config` literal;
  one line.

No wave-2 peer owns any of them (W04 is `handoff/{manifest,grid}.rs` and
`pane_host/**`, W05 is `handoff/{fd,protocol}.rs`).

**`check` cannot take a `--channel`, and did not grow one.** W03's clap tree
puts `--channel` on `apply` only. Rather than edit `cli.rs` — the one file the
wave plan exists to keep single-owner — `check` reads the channel from
`config.toml` and the built-in default, which is what the acceptance test
exercises. Asymmetric, and arguably wrong for a read-only verb: **hand-off to
W14 or whoever next owns `cli.rs`** — one `Arg` on the `check` subcommand and
one `args.get_one` at the call site, no other change.

**mise's `$MISE_INSTALLS_DIR` is not read, deliberately.** herdr consults it so a
relocated installs root is still detected. This crate's rule is that process
environment is read once in `run` and threaded as an `amx_core::Env`, and no
`Env` field carries that variable; adding one is amx-core's file, not this
task's. Consequence, stated in `update/pm.rs`'s docs rather than left to be
discovered: a user who has moved mise's installs root out of a directory named
`installs` is classified `Standalone`, and `apply` would replace a file mise
believes it owns. The fix is one `Env` field plus one parameter on
`pm::classify`.

**Version strings are strictly `major.minor.patch`.** A `-rc1` suffix is refused
by name rather than compared as though the suffix were absent. There is no
preview channel to publish a prerelease into (R-M3-4), so this costs nothing
today and cannot silently order `0.2.0-rc1` above `0.2.0` later.

**The default channel was exercised against the real network, once, by hand.**
`https://github.com/saifulapm/amx/releases/latest/download/latest.json` answers
404, and `amx update check` prints `no manifest at … (curl exited 22: curl: (22)
The requested URL returned error: 404)` followed by the no-release-pipeline
sentence, exit 0 — R-M3-4's expected answer, observed rather than assumed. The
suite itself touches no network: every test channel is a `file://` URL.

**curl's exit codes are never translated.** amx prints curl's own stderr line
beside the numeric status instead of mapping codes to English. The mapping would
have been written from memory, and `HACKING.md`'s first rule forbids exactly
that; the flags themselves (`-sSfL --retry --connect-timeout --max-time
--max-filesize`) were read off `curl --help all` on the build machine.

**Budget watch.** `crates/amx/tests/update.rs` lands at 557 lines, the
nineteenth test file over the soft 500 and well under the hard 1000. About 140
of those are a rig that plants a *copy* of the binary under test at an arbitrary
path — which `crates/amx/tests/support/mod.rs` cannot do (it hard-codes
`CARGO_BIN_EXE_amx`) and which W11 and W12 will both want. Lifting it into
`support/` is the split, and it belongs to the wave that next needs it rather
than to this one, since `support/mod.rs` is shared and already at 509. No `src/`
file this task touched is over the soft budget; the largest is
`cmd/update.rs` at 338.

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

---

## W06 — Export path

**The pre-flight runs on the connection task, not on `Core`.** §3 numbers it step
0 and D-M3-6 point 2 says why it exists; where it *runs* was open, and putting it
in `Core`'s arm would have made "refused before any pane is touched" a claim
about statement order inside one function. It runs in `dispatch/session.rs`
instead, and only its verdict travels on: `SessionCall::Handoff` carries
`preflight: Result<Caps, String>`, so a `Core` that refuses a wrong binary has
provably not reached a pane. It also keeps a staged binary that never answers
from stalling every other verb — `Core` serves one mailbox loop, and an exec on
it would hold `session.state` behind a subprocess.

**`Core` freezes in one mailbox turn, and the orchestrator does the talking.**
`handle_handoff_live` quiesces every pane (on the blocking pool), captures each
one through `PaneCommand::ExportHandoff`, builds the manifest, and hands a `Job`
to a task; `Core` is stalled for exactly that turn and the socket answers
throughout. The alternative — the orchestrator asking `Core` for each piece —
would have meant the session frozen across a socket conversation with a peer
that may never answer.

**Three fences, not one, and the second is what R-M3-10 actually needs.** D-M3-6
point 5 disarms "the final Persist push". The final push is one of *two* ways the
exporter's dying view reaches disk: the other is a debounced save, which asks
`Core` for a capture through the ordinary mailbox. So the fence is read in both
places — `push_final_capture` returns early, and `handle_capture` drops the reply
channel, which `Persist` already reads as "`Core` cannot be reached" and turns
into a save that does not happen. The third is the ordering: the fence is armed
*before* `committed` goes on the wire, never after, so there is no window at all.
`no_final_snapshot_is_written_after_commit` pins both halves and fails on each
one independently — with the final push unfenced the exporter writes a
`session.json` that was not there before (`None` → `Some(inode)`), and with the
capture unfenced a post-commit flush succeeds.

**A failed commit unfences.** `HandoffError::ending()` puts `Stage::Committed` on
the abort side — the importer never heard the word, so it strict-aborts and this
server owns the snapshot again — which means the fence has to be reversible.
`Ledger::unfence` is that, and it is the only path back.

**Gateway retirement is a mode, and the client token is replaced rather than
reused.** `GatewayControl::{retire, restore}` drive one actor through two modes;
the `JoinSet`, the accounting, the `StatusView` and the hub handle all survive,
so `GatewayReport::clean()` still means what it meant. The one thing that cannot
survive is the connections' cancellation token: retiring cancels it, and a
cancelled token cancels everything handed it afterwards, so a restored gateway
reusing it would accept and immediately hang up. It takes a fresh child token on
the way out of the retirement.

**The retirement's join is bounded (5 s) and the swap does not wait for it.** The
successor is waiting for the socket path, so a connection slow to notice its
cancellation must not hold the upgrade open. Nothing is abandoned — the task
stays in the same `JoinSet` and is accounted whenever it returns — but
`RetireReport::outstanding` says how many were left, which is W01's near-miss
signal seen early rather than at the drain.

**The drain watchdog shortens the census interval to fit inside its bound.**
W01's census is written when a drain overruns `CENSUS_INTERVAL` (5 s); a
post-commit bound of 30 s would produce one, but a *test's* bound of 400 ms would
not, and a watchdog whose diagnosis is unavailable under test is a watchdog
nobody can check. `export::drain::bounded` therefore sets the interval to half
its bound (capped at W01's own), and reads the file back into the error. What it
prints, measured: `the exporter's shutdown drain did not empty within 400ms after
the handoff committed; pid 1098484; waiting 200 ms; 1 task(s) not returned:
stubborn`. Returning drops the `JoinSet`, which aborts whatever had not returned
— correct only after the commit, which is the only place this is used.

**Two edits outside the §5 scope list, both forced.**

- `crates/amx/src/cmd/handoff_caps.rs` — W03 planted the stub with `**W06** owes
  it` written on it and on its clap entry, and the §5 scope list simply does not
  name it. Without a body the pre-flight refuses every real successor, so
  `session.handoff` against the actual binary could never be accepted. It prints
  `Caps` itself (`{"version":"0.1.0","handoff":[1,1],"proto":[1,1]}`), which is
  the same type the orchestrator reads back — there is no second spelling of the
  format to drift.
- `actor/pane_host/mod.rs` gains `PaneHost::pty()`. `quiesce`/`resume` block, so
  the freeze and the rollback belong on the blocking pool, and a `PaneHost`
  cannot go there because `Core` is still serving with it. The pty actor's own
  mailbox can: it is `Clone`, and the orchestrator holds one per frozen pane so
  an abort unfreezes the session without queueing behind `Core`. Four lines in a
  wave-2 file no wave-3 peer touches.

**Three edits in files W03 had already crossed into, and why each is now
different.** `tests/hygiene.rs`'s seam ledger is emptied and the assertion
restored to its resting form — W06 answered M3's one row two waves before the
plan expected it, and the test's own failure message says to do exactly this. Its
agent-event guard also grew one exemption: `Exporter::commit` is §3 step 13 and
shares nothing with `StatusView::commit` but the English word. And
`tests/skew.rs`'s `method_golden_and_skew_arm_cover_session_handoff` asserted the
seam code; it now asserts the behavior — a staged binary that does not exist is a
*reply* (`accepted: false` with the reason), never a failed call. **W11 also owns
`tests/skew.rs`**; the two edits are in different functions.

**`actor/gateway.rs` became `actor/gateway/`, three files.** R-M3-7 predicted it
("`gateway.rs` 383 + retire") and retirement took it to 648. Split by
responsibility on arrival rather than at the hard limit (R-M1-3):
`mod.rs` (347, the actor and its accept loop), `bind.rs` (159, the socket-taking
rules, read twice now — once by `Gateway::bind` and once by a restore), and
`retire.rs` (187, the mode the export path drives). Nothing outside the module
changed: `crate::actor::gateway::…` resolves to the directory unchanged.

**One rig hazard worth writing down: `ETXTBSY` when a test writes a program.**
`fixture_binary` plants a shell script and then execs it, and a pane spawned by a
*concurrent* test can inherit the write descriptor for the few microseconds
between its `fork` and its `exec` — the kernel then refuses to execute a file
somebody holds open for writing. Seen once in a full-workspace run here and once,
the same day, in `crates/amx/tests/update.rs`, which plants a copy of a real
binary the same way. The rig retries the exec past `ETXTBSY`; the update suite
has no such retry yet and is the other half of this note.

**`amx session report` does not *render* the handoff row.** The wire carries it —
`session report --json` against a real server prints
`"handoff": {"outcome": "aborted", "stage": "quiesce", "reason": "the handoff
peer went quiet during the token stage", …}` — and the acceptance tests assert on
the reply. What is missing is the human line: `format_report` in
`crates/amx/src/cmd/session.rs` renders `reply.report` and ignores
`reply.handoff`, so `amx session report` still says "no losses to report" after
an aborted upgrade. That file is not in W06's scope and nothing forced it.
**Hand-off to W14** (or whoever next owns `cmd/session.rs`): one branch above the
restore table, printing outcome, stage, binary and reason.

**Live-smoke of the real binary, since three milestones have been caught by a
green suite.** Against `./target/debug/amx` on 2026-08-08: `_handoff-caps` prints
its window and exits 0; `amx session handoff --binary <that same binary>
--timeout 2s` is *accepted*, freezes the pane, spawns the importer, and — because
`amx server --handoff-import` is W07's and does not exist — times out at the
token stage, aborts, resumes, and the session answers `ping` and `session state`
afterwards; `--binary /bin/true` is refused with "did not print handoff
capabilities"; the server then stops on `SIGTERM` with exit 0. That is the whole
export path over a real socket with a real pty, up to the point where the
successor would answer.
