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

## W08 — Server-side reconnect-resync

**"Opens delta-only" is implemented literally: nothing is sent.** D-M3-7 says a
re-bound grid stream opens with "`Generation`-keyframe-or-nothing", and 04 §4
says a keyframe is sent "on reconnect/resync". They only agree if equal
generations mean *nothing owed*, so that is what landed: a bind whose
generation matches the pane's live one starts with an empty dirty set, adopts
the first publication's shape and publication counter without marking a row,
and sends its first cells only when new damage arrives. (It does send one
cursor message immediately — `Resume` carries no cursor, so the server owes it,
and a cursor message is not a keyframe.)

The alternative considered and rejected was a full-grid *delta* — same bytes as
a keyframe, applied onto the client's grid rather than replacing it — which
would have been correct no matter what the client's cells actually held. It was
rejected because it makes the generation meaningless: every re-bind would repaint
regardless, and the field would be doing nothing. The cost of the literal
reading is a contract the client now carries, stated in `conn/resume.rs`:

> Presenting a generation asserts "I hold a **complete** grid at generation G."
> The server cannot check it — the generation moves on resize and reset only, so
> agreement means the geometry still matches, not that the cells are current.

That contract is cheap to honor across a handoff, because panes are quiesced
*before* the exporter retires its gateway (§3): a client drained at disconnect
time was drained against a grid nothing could still move. A client that was not
drained must omit the generation (opening with `First`, exactly as before) or
ask for a keyframe with `FlowControl::Resync`. **W09 owns that judgement.**

**The hello's `generations` is read too, and both halves of `Resume` are spent
once.** §5 scopes W08 to the bind parameter, but `Resume.generations` is the
other frozen field with no reader and 04 §6 puts the claim in the *hello*
("re-Hellos presenting its last cursor and generations"). Since no stream exists
at hello time — bindings die with their connection — the only thing the hello's
generations can be is a per-pane default for the first re-bind, and that is what
they are. Both `last_seq` and each pane's generation are **taken**, not read:
the claim describes the moment the client reconnected, so it answers exactly one
bare `events.subscribe` and one bare `stream.bind` per pane. A second bare
subscribe means "from here", not "rewind and replay it all again"; a second bare
bind for a pane is a client asking for that grid afresh, which is a request for
a keyframe, not a repeat of a claim it already cashed in.
`a_generation_presented_in_the_hello_is_spent_by_the_first_rebind` pins both
halves, and both halves are load-bearing — removing the fallback turns the
first assertion red, making the lookup non-consuming turns the second one red.

**Four edits outside the §5 scope list.** None is behavior beyond the decision
above; all four are the plumbing without which the decision cannot reach the
code that makes it.

- `conn/streams.rs` — the generation comparison happens here and nowhere else,
  because this is the only place the pane's live generation (from the wiring)
  and the client's claim are both in hand. `ConnStreams::bind` therefore takes
  `BindParams` whole instead of `StreamKind`, and `ConnStreams::new` takes the
  connection's resume block. No wave-3 or wave-4 peer lists this file.
- `conn/resume.rs` (new) — the resume block as connection state, shared by the
  event subscription and the stream bindings. It is a new file rather than
  fields on both, because the "spent once" rule has to be one rule.
- `tests/support/mod.rs` — `Server::start_with_replay` (the gap case needs a
  bus with the *real* `DEFAULT_REPLAY_CAPACITY`, not the harness's 64),
  `Client::{hello_resuming, hello_as_attach_resuming, send_request, next_frame,
  next_frame_within}` and `try_read_frame`. All additive; nothing existing
  changed shape. `next_frame_within` is the only way to assert a *silence*, and
  "no keyframe" is a claim about what the server did not send.
- `tests/agent_verbs/harness.rs` — one `ConnResume::none()` argument, the
  in-process rig's truth (no hello reached it).

**The keyframe *reason* is not observable on the wire, and is not made so.**
`GridMessage::Reset` has no reason field; adding one would be a wire change for
a fact only the server's own logs and tests want.
`a_bind_with_a_stale_generation_opens_with_keyframe_reason_generation` therefore
proves the keyframe over the socket and the reason through the public damage
API (`GridStreamConfig::resuming` → `GridStream::owed_keyframe`), against the
same live generation the socket case was judged against.

**The goldens did not move.** No file under `tests/goldens/` and nothing in
`amx-proto` was touched: `Resume` and the additive `generation` were already
frozen (R-M3-12 in full — this is the reader it was frozen for), so the resync
needed no wire at all.
`a_verb_connection_without_resume_behaves_exactly_as_before` is the behavioral
half of the same claim, and it is the one case in the suite that stays **green**
when the change is neutered.

**One intermittent failure observed, in `flow_control`, not attributable here.**
`two_clients_at_different_speeds_each_stay_consistent` failed once in a full
`cargo test --workspace` run with both streams reporting identical stats
(`absorbed: 60, coalesced: 1, keyframes: 1, deltas: 1`) — 58 of 60 publications
carried no damage at all, i.e. the pane's `cat` never echoed the bursts. That is
upstream of `GridStream` entirely, and `GridStreamConfig::new` still produces
byte-identical state for that suite (`live: FIRST`, `resume: None` ⇒ the same
`First` keyframe and the same `resumed: false`). Not reproduced in 10 repetitions
under 8-way CPU load, nor in two subsequent full workspace runs. Recorded rather
than explained.

**For W09 — the client contract, exactly.** What to send, when, and what to be
ready for:

1. **On transport EOF**, reconnect with backoff and re-`Hello` with
   `resume: Some(Resume { last_seq, generations })`, where `last_seq` is the
   highest bus sequence the client has *consumed* (from `Welcome.seq`, then
   every `Delivery::Event`'s `seq`, and `to` from every `Delivery::Gap`), and
   `generations` lists only panes whose grid the client believes **complete** —
   omit a pane whose stream was paused, stalled, or mid-delta at the
   disconnect.
2. **Branch on `Welcome.session`** before using any of it. A different
   `SessionId` means a different server: drop the caches and treat it as a
   fresh attach (`resume: None` would have been the honest hello, so re-Hello
   or simply re-bind with no generation — a stale generation from another
   server's pane id space cannot collide, but presenting one is a lie).
3. **`events.subscribe` with `after_seq: None`** on a resumed connection and
   the server opens at the hello's `last_seq`. Passing an explicit `after_seq`
   overrides it. Either way, be ready for `Delivery::Gap{from, to}` as the
   **first** delivery: that is a resume past the replay ring
   (`DEFAULT_REPLAY_CAPACITY`, 1024) and it means re-query `session.state` and
   resume from its capture seq — never a silent skip, and never an error.
   The fallback is spent; a second bare subscribe on that connection opens at
   the head.
4. **`stream.bind`** for a pane in the hello's `generations` may omit
   `generation` and gets the hello's value for its first bind; every later bind
   of that pane must pass `generation` explicitly or it opens with a `First`
   keyframe. Passing it on the call always wins.
5. **What arrives on a delta-only open**: a `Cursor` message, then ordinary
   `Delta`s. No `Reset`. A `Reset` after presenting a matching generation means
   the pane resized between the bind and the first frame, which is the
   `Generation` keyframe arriving late and correct.
6. **If the client cannot vouch for its cells**, `FlowControl::Resync` on the
   bound stream is the ask, and it is cheaper than a wrong screen.

**Budget.** `conn/events.rs` 302 → 315, `conn/streams.rs` 234 → 252,
`damage/stream.rs` 380 → 447, `conn/mod.rs` 209 → 227 — every one under the soft
budget, so the anticipated `conn/events.rs` split did not happen and should not
be forced. `tests/resync.rs` did pass 500 and *was* split: the frame-level
helpers moved to `tests/resync/harness.rs` (the `flow_control` `#[path]`
convention), leaving 358 and 218.

---

## W11 — SSH remote

**W03's tripwire is discharged, not deleted.**
`the_bridge_transport_row_is_planted_and_fails_when_the_splice_arrives` went red
the moment `_bridge` stopped refusing, and
`every_skew_sample_row_answers_over_the_bridge_transport` replaced it in the
same file: `for &method in Method::ALL` over the same `sample_params` table,
against a `Wire` on the child's stdio. It is **current-vs-current**, the M0
harness's honest label inherited whole — only protocol version 1 exists, so what
it proves is that the bridge negotiates and answers, not that it has been tested
across versions. A second version lands as a second entry in `ROWS` and nothing
else changes.

**W10's rig is lifted, and `support/mod.rs` is split four ways.**
`crates/amx/tests/support/mod.rs` went 509 → 44: it is now the module list and
the re-exports, over `env.rs` (roots, running, waiting), `tty.rs` (the
pseudoterminal and a child on it), `procs.rs` (the process table), and `rig.rs`
— W10's ~140 lines, verbatim in behavior, for a binary at an **arbitrary path**.
`update.rs` went 557 → 426 and keeps only the manifest fixture, as a
`Manifests` trait on the lifted `Rig`. **W12 can use it**: `Rig::plant` puts a
copy of the binary under test anywhere under the rig's root, `Rig::command`
hands back a `Command` still open for extra variables, and `Rig::run_on_tty`
runs it on a real terminal. `Env::spawn_on_tty` now takes `&mut Command` for
that same reason; the only caller signature that changed.

**`--remote` is not in the clap tree, and that is a hand-off.** It selects
*which machine parses the rest of the command line*, so it has to be taken off
`argv` before a parse happens — `remote::split` does that in `main.rs`, and
`cli.rs` stays the single-owner file the wave plan needs it to be. The cost is
real and unpaid: **`amx --help` does not mention `--remote`.** One `Arg` with
`.global(true)` on the root command would document it, and because `main` strips
the flag before clap ever sees it the argument would be purely documentary.
Hand-off to W14 or whoever next owns `cli.rs`, exactly as W10 handed over
`update check --channel`.

**`--remote` attaches; it does not carry verbs.** `amx --remote host session
list` refuses by name rather than running on the local machine. A remote
one-shot would mean re-implementing `cmd::call::one_shot` against a stream
instead of a socket path — `call.rs` is W09's file this wave, and the
duplication would be a second copy of the connect-negotiate-call preamble. Same
for `attach --pane`, whose chrome-free client also reaches its session by path.
Both refuse with the reason; neither is hard to add later.

**Seeding does not retry the attach.** After a successful install it prints the
path and says to run the command again, where herdr re-probes and continues. The
retry is a few lines, but the second attempt has to distinguish "installed and
now works" from "installed and still cannot exec", and the honest message for
the second case is the same message as the first. Recorded as a deliberate
simplification rather than an oversight.

**Three things about ssh and sshd were measured on the build machine, not
assumed.** All three shape code that would otherwise have been written from
memory:

- ssh(1) joins the command arguments with spaces into **one string for the
  remote login shell**, so quoting is amx's job. A `SessionName` is validated as
  a path component and may hold spaces, quotes and `$`, so every argument amx
  sends is single-quoted (`remote::ssh::sq`, with the `'\''` construction that
  works in every `sh`).
- **ssh ignores `$HOME` when it looks for `~/.ssh/config`** — it reads the
  passwd entry's home. So `tests/remote_ssh.rs` cannot hand ssh a port and a key
  by redirecting `HOME`; it puts a two-line `ssh` on `PATH` that adds
  `-F <config>` and `exec`s the real binary with amx's argv untouched. The
  connection, the absent pty, the login shell and the framed bytes are all real.
- **sshd takes the first value it obtains for a keyword**, so five `SetEnv`
  directives set one variable and silently drop four. The first version of that
  test config set `PATH` and lost all three XDG roots, and the "remote" session
  ran in the developer's own runtime directory. One `SetEnv` line, all variables
  on it. `HOME` and `SHELL` cannot be set that way at all — sshd assigns both
  from the passwd entry afterwards — which is why the pane's shell is pinned
  through `[terminal] shell` in the config root sshd *does* carry across.

**The remote command looks in two places, not one.** `PATH`, and then
`~/.local/bin/amx`. The second is not redundancy: it is the directory seeding
installs into, and a non-interactive ssh `PATH` usually does not contain it — so
without the fallback a seeded host would still fail on the next attach. herdr
hits the same wall and warns about it; amx looks there instead. Anything else
prints amx's own marker (`remote::ssh::NO_REMOTE_AMX`) and exits 127, so
"the far side has no amx" is detected by a string this crate chose rather than
by pattern-matching whatever the login shell says about a missing command.

**The loopback-sshd job ran here, not only in CI.** `AMX_TEST_SSHD=1 cargo test
-p amx-rig --test remote_ssh` is green on this machine: a real sshd on
127.0.0.1, a real ssh connection, a real remote server, a pane that renders its
prompt, a typed line whose output comes back, and a detach that leaves the
remote session running. `scripts/ci.sh` sets the variable on Linux when
`/usr/sbin/sshd` exists and prints the skip reason otherwise; the workflow
installs `openssh-server` on the Linux runner only (R-M3-6).

**Two files outside the §5 scope list, both forced.** `crates/amx/src/lib.rs`
gains `pub mod remote;` — **W12 will need the same one line for `git.rs`**, so
that is one trivial conflict declared in advance. `tests/support/wire.rs` gains
`Wire::over(UnixStream)`, without which the skew row has no way to speak the
protocol over a stream that was never connected to a path; `tests/support/env.rs`
gains `Env::home()` for the same kind of reason. No wave-4 peer owns any of them.

**Budget watch.** `tests/skew.rs` went 444 → 516 and is over the soft budget:
the bridge row and its socketpair helper are what grew it, and it was **not**
split, because splitting would separate the conformance table from the rows that
run it and the plan names this file as the row's home. `tests/support/env.rs`
(the rig's) is 561, over before this task and one method more now. No `src/`
file this task touched is over: the largest is `remote/ssh.rs` at 321.

---

## W07 — Import path

**The five-actor order moves in one place, and §3 is what moves it.** `serve`
binds the session socket *first* — losing that race must cost an error, not a
set of actors to tear down — and builds `AgentHub` before the restore so no
`PaneStarted` is missed. An importer cannot bind first: the exporter keeps its
listener through `restored` (D-M3-6 point 4). So the bind lands in the middle,
and everything hanging off the gateway moves with it: `StatusView` is the
gateway's to create, so the hub is assembled *after* the bind. Nothing is lost,
because the reason `serve` builds the hub early is that its restore **spawns**
panes, and this path spawns none — `Core::announce_inherited` hands the hub
every inherited pane explicitly, `await`ed rather than `try_send`, once the hub
is listening and before `Core` is running.

**`Core` runs before `ready`, not after the commit.** §3 step 12 promises that
new connections see "frozen grids and full state", and a `Core` that had not
started yet would leave a client's first call parked in a mailbox until the
commit — including its `Welcome`, which is built from `Core`'s state. Running it
early costs nothing that matters, because nothing it can be asked to do moves a
pane: every one is quiesced and every terminal is behind a closed gate. It does
mean a client that connects in the `ready`→`committed` window can *mutate* the
session (a split, a close) a moment before ownership formally transfers; the
window is one round trip and the alternative was worse.

**The terminal is gated, not just quiesced, and the gate is load-bearing.**
`InheritedPtySession` (`platform/pty.rs`) starts behind a closed `PtyGate`:
`read` and `write` answer `WouldBlock` until the commit opens it. The quiesce
covers the pane from a moment after `PaneHost::spawn`; the gate covers *that
moment*, and it is not academic — with the gate forced open,
`panes_stay_quiescent_until_committed_and_resume_after` reads `"go\nRESUMED"`
off the successor's screen before the commit. What that costs in the abort case
is the whole point: output the successor consumed pre-commit is output the
exporter can never show again, so a handoff that then aborted would have
destroyed exactly what it was carrying across. A gated `write` answers
`WouldBlock` rather than failing, so the pty actor keeps the chunk queued and
writes it after the commit — the same promise a quiesce makes about queued
input. **`resize` is deliberately not gated**: refusing one would lose it (the
actor logs and moves on) and leave the successor's grid and the child's winsize
disagreeing, which is worse than a `SIGWINCH` arriving a beat early.

**Resuming a pane is not `Core`'s to do, so `PaneHost` grew one accessor.** The
commit lands long after `Core` owns the hosts, and a resume must bypass the
mailbox the way a quiesce does. `PaneHost::resumer()` answers a `PaneResume`
(in `pane_host/export.rs`, beside the rest of the handoff vocabulary) which can
resume that pane and do nothing else — not release the terminal, not duplicate
it. `Adopted::take_over` opens every gate and then resumes every actor, in that
order; a resumed actor whose gate was still closed would poll a terminal it is
still refused. It is spelled `take_over` rather than `commit` because
`tests/hygiene.rs` counts `.commit(` call sites in the server, and one of them
is the agent-status ordering rule.

**Hub seeding is W07's work and the §5 scope list does not name its files.** §3
step 9 says "hub seeded" and R-M3-13 says why, but the plan's file list stops at
`session/import.rs`, `core/import.rs`, `platform/pty.rs`, `cmd/server.rs` and
the tests. Seeding needs three more, none of them touched by a wave-3 or wave-4
peer (W06: `handoff/export.rs`, `gateway.rs`, `core/handoff.rs`,
`dispatch/session.rs`, `serve.rs`; W08: `conn/**`, `dispatch/stream.rs`,
`damage/stream.rs`; W11: `cmd/bridge.rs`, `remote.rs`, `main.rs`):

- `actor/agent_hub/inherit.rs` (new) — `InheritedPane`, `AgentHub::inherit`
  (statuses into the view before the socket is bound) and `AgentHub::adopt`
  (the tracker, when the pane's frames arrive), plus the one-line branch in
  `track` that chooses between adoption and the ordinary identification path.
- `agent/fusion/tracker.rs` — `Tracker::adopt`, which continues a status a
  predecessor established: already-reported (so the next transition says what it
  moved *from*), no identity grace (the agent booted on somebody else's clock),
  and the staleness deadline re-armed for a held state.
- `actor/agent_hub/commit.rs` — `AgentHub::write`, one line wrapping
  `StatusView::commit`, because `agent_events_have_exactly_one_publisher`
  requires exactly one `.commit(` call site in the server and the seeding is a
  second writer. Both writers are still the hub's and still go through one
  function, which is what the rule is actually about.

**Provably load-bearing.** With the adoption branch disabled,
`restored_agents_report_their_manifest_status_without_a_flap` fails with both
agents reading `idle` — the hub re-identifies from the carried argv, publishes
`agent_identified` plus a probe transition per pane, and its mirror overwrites
the one `Core` was seeded with. That is the flap R-M3-13 describes, reproduced.

**The hook token does not cross the manifest, and post-handoff hook reports are
dropped.** `AMX_HOOK_TOKEN` is minted per spawn and lives in `SessionState`;
`PaneSnapshot` does not carry it (a cold restore respawns the child and mints a
fresh one, so it never needed to) and neither does `PaneManifest`. A handoff
keeps the *same* children, whose environment still carries the exporter's token,
so the successor mints one they do not know and D-M2-4's misattribution guard
drops every hook report from an inherited agent until it restarts. Agent status
degrades to tier 2 alone — screen detection still works, `wait --until blocked`
still returns, and the carried status is right until the screen moves. **The fix
is one additive field**: `PaneSnapshot.token` (or a `token` on `PaneManifest`),
filled by `capture_cheap`, read where `core/import.rs` currently calls
`mint_token()`. It belongs to whoever owns the persist schema next — W06 or W14
— and it is the largest honest gap in this path.

**§3 step 6's "session dir identity" cannot be checked, and is not.** The
manifest carries a `SessionId` and a persist `Snapshot`; it carries no session
*name*, socket path or state directory, so the importer has nothing to compare
its own `Ctx` against. What it does check is the manifest's read window
(D-M3-6's *window*, not equality) and that the descriptor count matches the
entry count — the pairing `Importer::validated(panes)` then enforces. The
exporter spawns the importer with the session already selected, so the check
would only catch a bug in W06; adding it is one additive manifest field and one
comparison.

**The attention queue is reconstructed from `AgentSnapshot.attention`.**
`Manifest.agents` is a list, and the only place block order survives in it is
the rank the hub fills in on a status before mirroring it. `core/import.rs`
sorts by that rank; a pane that wants attention and carries no rank is queued
behind everything that does, because leaving a blocked agent off the queue would
lose it from `agent.next`. **For W06:** capture the hub statuses from `Core`'s
mirror (where `AgentCall::Status` has already filled the position in), or the
queue crosses in whatever order the entries happen to be listed.

**§3 step 14 says the successor publishes nothing; it publishes one
`pane_damage` per pane.** The seeded parser publishes its first frame, the pane
actor announces it, and that is one sequence number per inherited pane before
any client can have connected. It is harmless — at-least-once damage is the
contract, and the resync W08/W09 build treats it as an ordinary delta — but it
is not literally "nothing", so the flap test asserts the sharper thing: every
delivery replayed from the exporter's last sequence is examined, and **none** of
them may be an `agent_status`, `agent_identified`, `attention_enqueued` or
`attention_dequeued`. `bus_continues_from_the_inherited_seq_and_welcome_reports_it`
pins the continuity itself: the successor's first event is `inherited + 1`,
gapless and never a restart at zero.

**Two helpers are spelled twice, deliberately.** `session/import.rs` carries its
own copy of `serve.rs`'s config watcher and signal watch. `serve.rs` is W06's
file this wave, and a live upgrade that silently stopped reloading `config.toml`
or stopped answering `SIGTERM` would be a capability lost to a merge conflict.
Folding the two assemblies' shared scaffolding into one is a **W14 seam**, named
here so it is a decision rather than an oversight.

**`Core`'s history window is seeded from the manifest, approximately.**
`session.state` answers the window synchronously from a map `Core` folds from
pane reports, and an import has none to fold. It is seeded with the carried
`head` and the *first carried row* as the floor; the parser derives the
authoritative floor from what actually landed in the successor's scrollback
(W04), which is never older. A client asking for a row the replay could not fit
gets the ordinary refusal rather than silence.

**One line outside the scope list in `crates/amx/`.** `cmd/mod.rs`'s server arm
reads its sub-matches (`Some(("server", sub))`) instead of discarding them,
because `--handoff-import` is the one flag that changes which assembly the
process runs. `cli.rs` gained exactly the argument W03 said it would, hidden.

**Budgets.** Three splits, all on arrival rather than at the hard limit
(R-M1-3): `agent_hub/inherit.rs` out of `agent_hub/mod.rs` (573 → 484),
`PaneResume` into `pane_host/export.rs` (mod.rs 520 → 488), and
`tests/handoff_import/harness.rs` out of the suite (1080 → 435 + 685, the suite
having gone over the *hard* limit). `agent/fusion/tracker.rs` is at 511 and was
**not** split: it is one state machine and `adopt` is one of its transitions, so
the only seam available would cut the transition function in half. It is the
first `src/` file over the soft budget on this path; the next task to add to it
should expect to move `Armed`/`Pending` out rather than trim.

**What W14 needs to know for the real upgrade under load.** (1) The importer is
`amx server --handoff-import <socket>` with the token on stdin, and it exits
non-zero having served nothing on any pre-commit failure — smoked against the
real binary: `the handoff socket failed during the token stage … ending=Abort`,
exit 1. (2) There is no full-binary upgrade smoke yet and there cannot be until
W06 exists, because the exporter is what spawns the importer; everything below
that line is exercised in-process over real sockets, real descriptors and real
ptys. (3) The successor blocks briefly per pane, twice — once quiescing at
adoption, once resuming at the commit — so a 200-pane session pays two mailbox
round trips per pane, on the blocking pool rather than on a runtime thread.
(4) The probe loop between `restored` and `ready` costs one `Gateway::bind`
attempt per 10 ms up to `Timeouts::socket_free`; each attempt's connect probe is
fast against a real exporter (it answers) and slow (1 s) against anything that
holds the socket without answering. (5)
`strict_abort_on_missing_commit_serves_nothing` covers the wedged-exporter row
by *holding* the socket open without committing, which is §3's split-brain case
rather than a peer that merely died.

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

---

## W12 — Worktree flow

**The plan's W12 scope was missing the field the pass-through passes through.**
§5 gives W12 `dispatch/workspace.rs` for "worktree block pass-through" and D-M3-10
says the block is what `done` and restore read — but §4's additive-field list
names only `session.state` and the persist snapshot, and W03 accordingly planted
the field on state, on the snapshot and on `SessionState::set_worktree`, with no
wire path that could ever *set* it. Two files outside the §5 scope are what closed
that, both concurrent-safe (wave 4's other task, W11, owns `cmd/bridge.rs`,
`remote.rs`, `main.rs`, `tests/skew.rs` and CI):

- `amx-proto/src/control/workspace.rs` — `worktree: Option<Worktree>` on
  `CreateParams`, additive and optional under R-M1-8. One field for two facts,
  because the server needs both at the same instant: the membership to remember,
  and the directory the new workspace's root pane opens in. A workspace on a
  worktree whose shell started in the server's own directory would be a
  membership in name only.
- `amx-core/src/config/mod.rs` — a fourth section, `[work] dir`, exactly as W10
  added `[update] channel` for the same reason (its §5 scope named "config
  channel-URL field"; W12's forgot the equivalent). `SECTIONS` is asserted
  verbatim by `amx-core/tests/config.rs`, so that suite moved with it.

`dispatch/workspace.rs` then needed **no arm at all** — every function there
forwards its parameters whole — which is the better outcome: the membership rides
the create it belongs to, arrives on the same serialized mailbox as the workspace
it describes, and cannot be recorded against a create that failed. What the file
gained is the paragraph saying so.

Three one-line sequential fills in files earlier waves own, each declared here
rather than discovered: `cmd/apply.rs` and two server test suites gained
`worktree: None` on their `CreateParams` literals (W13's layout deliberately
carries no worktree, so `None` is the semantic and not a placeholder), and
`amx-proto/tests/goldens.rs`'s `method_workspace_create` gained the same —
**the golden's bytes are unchanged**, which is itself the additivity assertion.
The field's own two directions are pinned in `additive.rs` beside W03's, as
`workspace_create_with_a_worktree_reads_at_v1_and_without_it_still_parses`.

**`workspace.create`'s `focus` field has no reader in the server, and `amx work`
found out the hard way.** Nothing between the dispatch arm and `Core` looks at
`CreateParams::focus`; `finish_workspace_create` never switches. A green suite
did not notice — the acceptance tests read `session.state` rather than asking
which workspace is focused — and the live smoke did, at the first `amx work done`
with no branch, which resolves the focused workspace and found none. `amx work`
now sends `focus: false` and calls `workspace.switch`, the verb that actually
moves focus, with the comment saying why rather than two mechanisms for one
effect. **Hand-off:** either give the field a reader in
`actor/core/workspace.rs` or delete it from the row. It is not W12's to decide,
because honoring it would change what every existing `focus: true` caller does
(`tests/config_reload.rs` is one) and D-M3-10 says nothing about it. Fourth time
a green suite has hidden a non-working path from a live run.

**`Repo::discover` reads `git worktree list --porcelain`, not
`rev-parse --show-toplevel`.** Verified on git 2.55.0: the porcelain list names
the **main** worktree first, from anywhere inside the repository, including from
inside a linked worktree. `--show-toplevel` would answer with the linked tree —
and since the default template is built on `{repo_parent}`, `amx work` run from a
tree it had just made would stack the next one *inside* it, which is the ordinary
way somebody would use the verb. Pinned by
`work_from_inside_a_worktree_places_the_next_one_beside_the_repository`, a fifth
test beyond the four §5 names.

**The destructive path got a fourth lock the plan did not ask for.** D-M3-10 says
"pinned under the derived path, never a user-supplied one"; that is lock 1
(recompute the template, compare, refuse with both paths named). Locks 2 and 3
came out of writing the failure down: the repository's own `worktree list` has to
agree the path is one of its worktrees, and the path may not be the repository's
main working tree. `git worktree remove` refuses that last one too, but its
sentence is "the main worktree cannot be removed" where amx's is "that is the
repository's main working tree", and the difference matters when the reason a
user is reading it is that a template went wrong. Lock 4 is the dirty refusal,
asked by amx before the workspace dies rather than left to git after — the
ordering is the point, since a `done` that took the panes and then declined the
tree would have destroyed the half nobody can rebuild.

**A failed `workspace.create` rolls the tree back; a failed `agent.start` does
not.** Asymmetric on purpose. A checkout seconds old with no workspace ever
opened on it holds nothing to lose, so leaving it would be litter the caller did
not ask for. By the time an agent fails, the caller has the workspace and the tree
they asked for, and taking both away over a third thing would be amx deciding
their work was worthless — V13's "the pane is left running for inspection", one
level up.

**`cmd/work.rs` split on arrival at 527 lines**, per R-M1-3's no-split-waits rule:
`crates/amx/src/work.rs` (131) holds the template question — `DEFAULT_DIR`,
`template`, `derive_path` — and `cmd/work.rs` (423) is the verb, the same
`cmd/update.rs` + `src/update/` shape W10 used. The split is load-bearing rather
than cosmetic: `derive_path` is the pin on the destructive path *because* it is a
pure function of three strings, and a home with nothing else in scope is what
keeps it one. `actor/core/restore.rs` crossed the soft budget at 505 with the new
handler and is back at 499 on a right-sized doc comment; the split it will want
eventually is `replay_bytes`, which is about the sidecar format and not about
restore's policy.

**Live-smoke of the real binary**, against `./target/debug/amx` on 2026-08-08 in a
throwaway repository, because the green suite already hid the focus gap once:
`amx work fix-thing` printed its two lines and `session state` carried the block
with all three fields; `git worktree list` showed the tree; an untracked file made
`work done` refuse and leave both halves standing; `work done --force` collapsed
all three and left the branch; `amx work -- --force` was refused before any argv
("a branch name cannot begin with '-'") and `amx work 'bad..name'` by git's own
grammar; then `work gone`, `session stop`, `rm -rf` the tree, restart — and
`amx session report` printed `degraded workspace gone … the git worktree for
branch gone is gone; kept as a plain workspace` beside the pane's own degradation,
with the workspace back under its own id and no worktree block. Also smoked: no
session running, no arguments, outside a repository, twice for one branch, and
both no-branch `done` refusals.

---

## W09 — Client reconnect

**Both halves of the resume claim are sent, and either alone would do.** W08's
contract puts the generations in the hello (spent by the first bare re-bind) and
also lets `stream.bind` name one explicitly, where "passing it on the call always
wins". The client does both, and the redundancy was measured rather than
assumed: neutering the bind parameter leaves
`a_dropped_client_reattaches_with_resume_and_repaints_only_stale_panes` **green**
(the hello answers the first bare bind), and neutering `App::vouched` leaves the
keyframe half of it green too (the bind parameter answers that one). Only
removing both turns it red. Kept anyway, and here is the case that needs the
explicit one: the hello's block is spent *once per pane per connection*, so the
second `bind_visible` for a pane on the same connection — which is what happens
after `Bindings::forget_pane` and a pane re-entering the visible set — would open
with a `First` keyframe if the call said nothing. Recorded because a reviewer
looking for the single load-bearing line will not find one.

**What the client is willing to swear to, and the one thing that revokes it.**
`Resume.generations` asserts a *complete* grid, so `PaneGrid` gained the flag
that makes the assertion checkable at all: `complete` is false for the blank grid
the model mints as somewhere for a delta to land, and true only once a keyframe
has filled it. The second half is `Session::torn` — the read state the
cancel-safe reader already kept, asked a new question. A connection that died
part way through a payload names the channel it died on, and the pane bound to
that channel is dropped from the claim; a connection that died part way through a
*header* names nothing, so the whole claim is dropped. That is the literal
reading of "omit a pane whose stream was paused, stalled, or mid-delta at the
disconnect", and it costs one keyframe in the case where it fires.

**`agent.prompt --wait` is *not* re-issued once its request has been written, and
the task list asked for it.** The divergence is deliberate: `agent.prompt`'s
first act is `pane.run`, which types into a pane, and a connection that died
after the request went out cannot say whether it typed. Re-issuing would be a
second prompt into a live agent's conversation — a worse outcome than the error.
So `cmd/call.rs` splits the failure in two. A failure *before* the request was
written (connect, `Hello`) is retried for every method, because a verb connection
mutates nothing by connecting and that is the ordinary shape of a verb typed
while a swap is in flight; a failure *after* it was written is retried only for
the reads and the state predicates (`reissuable`, an exhaustive match so a new
table row cannot inherit an answer nobody chose). `wait`, `pane.wait_output` and
`agent.prompt`'s connect half are covered; `agent.prompt`'s wait half, once
submitted, is not.

**Hand-off, and the thing that would close it:** D-M3-6 says input arriving
during the frozen window "fails fast at the handle (`NotAccepting`) and is the
caller's retry". It cannot be, today — `dispatch/pane.rs` maps `DriveError::
NotAccepting` onto `RpcError::INVALID_PARAMS`, which is indistinguishable from a
caller naming a pane that does not exist, and retrying on `INVALID_PARAMS` would
retry genuine mistakes. A distinct code (or an error-data tag) meaning "the
session is mid-swap and nothing happened" would make every mutating verb safely
retriable, including `agent.prompt`. Server-side and out of W09's scope.

**The redial window is the caller's own patience, not a constant.** A call
carrying `timeout_ms` gets exactly that long in total, measured from the first
attempt, and the re-issued call's `timeout_ms` is *reduced* by what has already
been spent — so `amx wait --timeout 2s` is two seconds whether it took one
connection or four, and `the_retry_gives_up_at_the_deadline_with_an_honest_error`
runs in about two. Everything without a timeout of its own gets ten seconds,
which is §3's five-second socket-free probe loop with room. This shape was chosen
because the alternatives all needed surface W09 does not own: a `--reconnect`
flag means editing `cli.rs` (W03's), a config field means editing `amx-core`'s
config (nobody's, this milestone), and an environment variable means reading the
environment outside `Env`, which T01's contract forbids.

**`amx events --json` had to branch on `Welcome.session`, and that was not in the
brief.** Found while testing the redial: a *cold restart* begins the sequence
space again at zero, so a relay resuming from its old cursor (say 24) would sit
silent until the new session reached 25 and then continue — silently skipping
everything the new session published first. Which is precisely the failure the
bus design exists to refuse. The relay therefore compares the `SessionId` it last
spoke to, and on a change says so on stderr and resubscribes from the head. A
*handoff* keeps both the id and the sequence, which is what makes resuming across
one exact, and is what
`events_json_resumes_from_its_cursor_or_reports_the_gap` drives — over a real
`amx session handoff`, asserting the printed sequences are contiguous across the
swap.

**A bridged client cannot redial, and does not pretend to.** `App::assemble` is
what `crates/amx/src/remote` builds a client from, and its connection is one end
of a socketpair whose far end is an `ssh` child; redialling that means respawning
ssh, which is the bridge's business. So the reconnect is keyed on an `Origin`
that only `App::attach` records, and a client without one keeps M2's behavior
exactly — a transport failure ends the loop. `a_bridged_client_has_nowhere_to_
redial_and_says_so` pins it so nobody later turns it into a busy loop against a
path that was never a path. **Hand-off to W14:** an ssh session that drops today
still ends the client; making the bridge redial is a `remote/` change, not a
client one.

**`crates/amx/src/cmd/viewport.rs` does not reconnect.** The `amx attach --pane`
client builds its own loop over a `Session` rather than running `App`, so none of
this reaches it. It is nobody's file in wave 4 and the change is not mechanical
(it has no model to resync). **Hand-off to W14**, or to whoever next owns that
file.

**W11's `amx --remote host <verb>` refusal is not made natural by any of this.**
The refusal stands because a remote one-shot needs `cmd::call::one_shot` to work
against a stream instead of a socket path. What changed is the shape of that
function: it is now a loop over `attempt(&ctx, wire, &params)`, and `attempt` is
the only thing that touches `ctx.socket`. A transport that hands back a
negotiated `Session` instead would plug in there — but a bridged one-shot has no
path to redial (see above), so it would want the loop bounded at one attempt, and
that is a decision for whoever does it.

**Files edited outside §5's list, all inside §6's "W09: `amx-client/src/**`,
`cmd/{attach,call}.rs`, client tests".** `crates/amx/src/cmd/events.rs` — the
`events` half of "reconnect-and-reissue for … `events`" lives there and §5's list
named only `attach.rs`/`call.rs`; no wave-4 peer lists it.
`amx-client/src/model/{grid,mod}.rs` and `src/stream.rs` — the completeness flag,
the keyframe counter and the channel→pane lookup the claim is built from.
`amx-client/tests/support/mod.rs` — `Server::control` (W06's `GatewayControl`,
which is how a test takes a client's connection away without taking the panes
with it) and `Server::usurp` (a second `Core` on the same path, the only way to
produce a *different* `SessionId`). `crates/amx/src/cmd/attach.rs` is in the
scope list and was **not** touched: `App::attach` records where to redial, and
`App::run` does the redialling, so the caller needed no edit.

**Budget: four splits, one of them not the one R-M3-7 predicted.**
`app/wired.rs` was 487 and R-M3-7 named `app/reconnect.rs` in advance; that split
happened and was not enough, because the loop also had to grow a fallible round
and a recovery arm. So the stream surface came out too — `app/binds.rs`, the
grid/raw/history binds and the viewport declaration — leaving `wired.rs` 418,
`reconnect.rs` 356, `binds.rs` 188. `net.rs` went 481 → 529 on the `Torn` and
`is_transport` additions and became `net/{mod,read}.rs` (353 + 206); the frame
reader was already a section of its own in that file's header, so the seam was
drawn rather than invented. `crates/amx/tests/wait_retry.rs` passed 500 and was
split on the `#[path]` convention (285 + `wait_retry/harness.rs` 293) — that
suite is four process-level tests over three long-lived children each, and the
scaffolding was most of its lines.

**An observed flake, in `amx-server`, not attributable here.**
`crates/amx-server/tests/agent_verbs.rs` failed twice in about a dozen full runs
(`agent_start_spawns_from_the_registry_labels_the_pane_and_returns_ready` once,
`agent_prompt_submits_via_run_and_wait_blocked_returns_on_the_next_block` once),
both inside the in-process dispatch harness. W09 changed no line of `amx-server`
and `amx-server` does not depend on `amx-client`, so the binary that failed was
compiled from unchanged sources. Recorded rather than explained.

**Live smoke of the real binary, on 2026-08-08.** A real `amx attach` on a real
40×120 pty, against a real server: `echo SMOKE-BEFORE` painted on its screen,
then `amx session handoff --binary <the same build>` — accepted, exporter logs
"the session has been handed over" — then `echo SMOKE-AFTER`. The client was
never restarted and never touched. Its final frame holds **both** sentinels and
2082 non-blank characters, which is the T19 lesson taken literally: a screen that
demonstrably held content, compared against itself across a swap.

---

## W14 — Integration

**Three bugs the milestone would have shipped, all of them found by leaving the
in-process world.** Each is written up where it was fixed; what they have in
common is worth stating once. Every one was green in `crates/*/tests/` and
broken over a socket, and none of them needed an unusual input — a named
session, a signal at startup, a client reconnecting after a pane painted. M2's
V17 lesson was that a suite one process away catches what a suite inside the
process cannot; M3's is that the *second* process has to be a different one.

1. **An upgrade of a named session bound the wrong socket.** `amx --session live
   server` takes its session from the flag, and the flag is not in its
   environment; the exporter spawned the successor with neither, so it derived
   `default`. Observed against the real binary: `live`'s socket unlinked,
   `default`'s bound, the panes held by a server nobody could reach by name, and
   `amx --session live ping` answering "not running". Fixed at the spawn
   (`--session` on the successor's argv) and refused on the importer's side,
   which is §3 step 6's session-dir identity — the check W07 judged "would only
   catch a bug in W06". It did.
2. **A `SIGTERM` in the assembly window killed the server outright.** W01 saw it
   once and could not place it. Two orderings were wrong, and both had to move:
   tokio registers a signal handler when `signal()` is *called*, not when the
   task that calls it is first polled, and neither assembly awaits between the
   spawn and the end of the assembly; and the install ran after `Gateway::bind`,
   which is the statement that makes the process findable. The regression test
   seeds sixty panes and signals the instant the socket path appears — red on
   every run without either half of the fix.
3. **A reconnected client could stay wrong forever.** §4 below.

**The eight hand-offs, and what became of each.**

| # | Hand-off | Disposition |
|---|---|---|
| 1 | the hook token does not cross (W07) | **fixed** — an additive `token` on `PaneManifest` |
| 2 | `workspace.create`'s `focus` has no reader (W12) | **fixed** — the field gets the reader, not the deletion |
| 3 | `session report` does not render the handoff row (W06) | **fixed** — one branch above the restore table |
| 4 | no `PaneState::cwd`, so `layout export` writes none (W13) | **fixed** — additive field, golden regenerated, export reads it |
| 5 | `update check` cannot take `--channel` (W10) | **fixed** — one `Arg` on both verbs |
| 6 | `amx --help` does not mention `--remote` (W11) | **fixed** — a documentary global argument |
| 7 | budgets (W04, W11, W01) | **fixed for the two named, judged for the rest** |
| 8 | §3 step 6's session-dir identity (W07) | **fixed, and it caught a live bug** |

Each in turn, where the disposition needed an argument.

**1 — the hook token rides the manifest, not the snapshot.** W07 offered
`PaneSnapshot.token` or a `token` on `PaneManifest`; it is the manifest. The
snapshot is a disk format and a cold restore respawns the child with a freshly
minted token, so a persisted one would be a dead secret in `session.json`
forever. The manifest crosses a 0600 socket to a process that needs it and is
never written anywhere. `an_inherited_agents_hook_reports_are_still_attributed_to_it`
sends a report carrying the exporter's token over the successor's socket and
asserts it is accepted, and a stranger's is still refused — the guard is a guard
again rather than a hole.

**2 — `focus` gets a reader.** The alternative was deleting it from the row, and
that is the wrong way round: the field is documented, on the wire and
golden-frozen, and every caller that sends `true` means it. Honouring it changes
what those callers do, which is what W12 could not decide alone — and every one
of them is a harness or a verb that creates a workspace in order to work in it.
`amx work` now says it once instead of following the create with a
`workspace.switch`, and the create publishes the same `FocusChanged` a switch
would. `a_create_asking_for_focus_gets_it_and_one_that_does_not_leaves_it_alone`
pins both directions, because the second is the one that matters to a tool
making a workspace in the background.

**4 — `PaneState::cwd`, and what it cost the layout round trip.** The additive
field landed as W13 specified. One consequence it did not predict: a layout file
now carries a `cwd` for every pane, because the server records one for every
pane, so `apply_builds_the_bsp_by_splits_in_deterministic_order` — which
compares an exported file against a hand-written fixture — compares them without
the `cwd` lines and says why. The round trip itself is unaffected: both sides
carry the same directories.

**7 — the budgets, one by one.** `handoff/grid.rs` (601) and
`handoff/manifest.rs` (592) are split, and W04 was right that the cost is now
one line: `grid/{mod,replay,emit}.rs` and `manifest/{mod,history,modes,base64}.rs`
resolve through the same `pub mod` declarations W03 planted, exactly as W05's
`protocol/` did. Three more files were split because *this* task grew them past
the line: `tests/support/env.rs` (which W01 raised and named `ServerChild` as
the seam for — that is the seam it was), `tests/handoff_exit.rs` into its own
harness, and `handoff/manifest/mod.rs` a second time when the identity check
pushed it back over.

**`tests/skew.rs` is not split, and W11's argument holds.** The file is the
conformance table and the rows that run it; splitting it would put the table one
file away from its only readers, and the plan names this file as the bridge
row's home. 526 lines, under the hard limit, and the reason it grew is a
transport that belongs there.

Left over the soft budget and *not* split, each with the reason: `pane_host/parser.rs`
(532, W04 grew it with the adoption and it is one loop),
`agent/fusion/tracker.rs` (511, W07's reasoning stands — the only seam cuts a
transition function in half), `pane_host/mod.rs` (501, one line over).
Nineteen-odd test files remain over the soft budget and under the hard one; the
rule the waves have followed is that a test file splits when the wave that grows
it grows it, and that is what happened here.

---

### The seam ledger, and the one exemption that was narrowed

W06 emptied `tests/hygiene.rs`'s ledger two waves early and restored the resting
form; that is still true and nothing re-opened it. What W06 also added was an
exemption on the agent-event publisher guard, so that `Exporter::commit` — §3
step 13, ownership of a session transferring — is not read as a second
`StatusView::commit`. It is still needed: `handoff/export/mod.rs` has exactly
that call. It was wider than the fact it covers, though — "any file under
`handoff/`" — and the import half of that module *does* seed agent statuses,
which is precisely the second publisher the guard exists to catch. The exemption
is now the receiver (`exporter.commit(`) rather than the directory.

---

### The joins, and the two assemblies that were spelled twice

W07 named the fold of `serve.rs`'s and `import.rs`'s shared scaffolding as a W14
seam, and it was worth doing rather than leaving: the two copies of the signal
watch, the config watcher and `home_dir` were byte-identical, so the next change
to either would have been a change to one. They are `session/scaffold.rs` now.

The fold is what made bug 2 above legible. Reading one copy, the ordering
question — *when* is the handler actually installed — is a detail of a
forty-line function; reading a file whose whole subject is "the scaffolding both
assemblies need", it is the only question there is.

---

### 4 — the resync bug, and a retraction

The exit suite's first honest run failed at the step §7 spells out: "the client
reconnected by itself and `pane.read` plus the client's own screen show every
sentinel". `pane.read` showed the sentinel; the client showed the pane's
*pre-swap* screen and never updated it again.

W08 opened a re-bound grid stream **delta-only** when the generation the client
presented matched the pane's, adopting the first frame it saw and marking
nothing. The premise is that a matching generation means the client's cells are
current, and `conn/resume.rs`'s own documentation says why it does not: the
generation moves on resize and reset only, so what agrees is the *geometry*.
W08's note argues the gap is closed across a handoff because panes are quiesced
before the exporter retires its gateway — true, and beside the point. The
successor **resumes** every pane at the commit, and a reconnecting client lands
after whatever they painted.

The failure mode is the worst kind: no error, no second repaint, a screen that
is wrong until something else forces a keyframe. It is not handoff-specific
either — an ordinary reconnect after a network blip has the same hole.

**What landed:** a resumed bind repaints, which is what 04 §6 asks for in as
many words ("keyframes for stale grids") — without evidence to the contrary
every grid is stale. What the generation still buys is the keyframe's *reason*
(`KeyframeReason::Resumed` beside `First` and `Generation`), and R-M3-12's
payoff narrows to `Resume.last_seq`, which drives the event replay and is the
larger half of D-M3-7.

**What would make the optimization sound**, written down rather than lost: a
resuming client is already replayed the events it missed, and D-M3-2 made
`pane_damage` exactly one event per transition — so a client that drained its
event replay *before* binding knows exactly which panes moved while it was away
and could vouch for the rest. That is a client-side sequencing change and it
needs no wire at all. The alternative, carrying the publication counter in the
claim, is a change to the grid stream's payload and is not worth it.

Three of W08's tests and one of W09's assert the retracted behaviour and were
rewritten to the corrected one, each with the reasoning at the assertion. Two of
them are *sharper* now than they were: they assert which reason a keyframe
carries, where before they asserted that none was sent.

---

### The exit suite: what it proves and what it stands in for

`tests/handoff_exit.rs`, three tests, all over the real binary.

- **§7 steps 1–4** in one test: five scripted agents across three workspaces, a
  styled sentinel (bold, underlined, 24-bit foreground *and* background) painted
  in every pane, a real client at 200×50 on a real pty, a standing `amx wait` and
  an `amx events --json` relay from other connections, then
  `session.handoff --binary <this build>`. Asserted in §7's order: the same five
  child pids by identity, the successor's `Welcome` with the same `SessionId` and
  a larger seq, the client's own cells for the sentinel compared **cell for cell
  and pen for pen** either side of the swap, the standing wait returning
  satisfied on a block that happens afterwards, the relay's sequences contiguous
  or gap-marked, the exporter's exit, and the pane's row-id window unmoved.
- **§7 step 5**: the abort injections a *staged binary* can produce — a successor
  that is not an amx (refused at the pre-flight, before a pane is touched) and
  one that answers the capability probe and never authenticates (aborted after
  the freeze). After each: the same server, the same children, every pane still
  answering input, and `session report` naming the outcome, the stage and the
  reason.
- **§7 step 6**: the bridge as a child against a session with five agents in it.

**Three things it stands in for, said plainly.** The agents are the M2 scripted
stand-ins (R-M2-8, unchanged). The successor is a build of the same tree, which
is the M0 skew harness's honest label — the *version* change is the live smoke's
and only the live smoke's. And the stages between `manifest` and `ready` are not
reachable from a binary at all: they need a peer that speaks §3 and then stops,
which is W05's and W06's in-process crash matrix.

**Two measurements the plan asked for.** R-M3-11 wanted the swap's wall clock:
**24 ms frozen for six panes** on this machine, measured from the exporter's own
log during the live smoke. And the T19 lesson is taken literally — every screen
comparison is paired with a count of non-blank characters, because two blank
screens compare equal and prove nothing.

**Serialized, deliberately.** The three tests run one at a time behind a mutex.
Each is a server, five pty children, a real client and a process swap; three at
once measures the machine, and it did — a sixty-second client-repaint timeout
when the suite ran beside a compile. A deadline that has to absorb two other
copies of the same test is a deadline that no longer means anything.

---

### The rig race W06 named, closed

`crates/amx/tests/support/rig.rs` now retries an exec past `ETXTBSY`, as W06's
export rig does, and the comment names the race so nobody removes it as
defensive noise: `cargo test` runs a suite on threads of one process, a spawn is
a fork and an exec, and between those two the child holds every descriptor the
process had open — including a write descriptor another thread is planting a
binary through. A long-lived server child holds it for its whole life. Copying
rather than hard-linking the plant does not close it: the race is about who
holds a descriptor, not about which inode.

The retry went into `support/env.rs`'s spawn helpers rather than only the rig's,
because `spawn_on_tty` is the third exec path and shares the same hazard.

---

### Turning W10's dormant half on

`HANDOFF_AFTER_INSTALL` is `true`, which is what W10 said would be the whole of
it, and it was. One change to the code behind it: the completion test is a
**pid**, not a version. A successor is a different process serving the same
`SessionId`; a version bump is the ordinary case and not a guaranteed one, and
the M0 skew harness and the exit suite both hand a session to a build of the
same tree — a version comparison would have sat out the ninety-second deadline
while a perfectly good successor served underneath it.

W10's tripwire is discharged rather than deleted: the sentence it asserted is
replaced by an assertion that the successor answered. One of W10's other tests
changed meaning as a result and says so — `apply` against a payload that is not
an amx now installs the binary, is refused the handoff by the pre-flight, and
exits non-zero with the reason on stderr. That is the right shape: the install
succeeded and says so on stdout, and something the user asked for did not
happen.

---

### What the live smoke found that CI could not

[m3-live-smoke.md](m3-live-smoke.md) is the record. Three things belong here.

**The first N→N+1 upgrade amx has done.** 0.1.0 → 0.1.1, a `--version`-bumped
build of the same tree, published through a `file://` channel manifest and
installed by `amx update apply` over the running binary, under five real Claude
Code sessions. Same five child pids, every conversation answering its
remembered word afterwards, exporter exit 0. Nothing in CI can do this, because
CI has one version.

**"No visible screen content lost" is two claims, not one.** A pane whose
program does not repaint comes back byte-identical — head, floor and every row.
A pane running Claude Code comes back with the agent's *own* UI redrawn, because
Claude Code redraws on a size announcement and the successor's commit resizes
every pane it takes over; measured as the same content shifted one row, with a
box border that had scrolled off the exporter's grid back on screen. Nothing is
missing. But the honest statement of the criterion has to name which half is
amx's.

**"Kill the importer mid-restore" is not a thing a human can do.** The window is
24 ms wide. A kill at 50 ms, 300 ms, 600 ms and 1200 ms after the importer
starts lands *after* the commit every time — which is not an interrupted upgrade
at all, it is killing the server that now owns the session. The by-hand half of
that step is the pre-commit abort, verified with three real Claude Code sessions
still answering afterwards; the rest is CI's, and the plan should say so.

**One step is not verified: an SSH attach from a genuinely different machine.**
There is one machine here. The loopback tier passed; it is not a substitute, and
the roadmap's "attach to the home machine from a laptop over SSH" is unproven
end to end. It is the one thing on this milestone that needs a person.

---

### Left open

- **The sound version of the resume optimization** (§4), which is a client-side
  sequencing change and needs no wire.
- **W09's three hand-offs, none of them taken.** `cmd/viewport.rs` (`amx attach
  --pane`) still does not reconnect — it builds its own loop over a `Session`
  rather than running `App`, and it has no model to resync, so the change is not
  mechanical. A bridged client still cannot redial, because redialling one means
  respawning `ssh`, which is `remote/`'s business rather than the client's. And
  `DriveError::NotAccepting` still maps onto `INVALID_PARAMS`, so D-M3-6's
  "input arriving during the frozen window is the caller's retry" is not
  something a caller can act on — a distinct code would make every mutating verb
  safely retriable, including `agent.prompt`.
- **`Env::pids_with_arg` and the exit suite's process assertions are Linux-only
  in practice.** `crate::platform::pids_with_arg` has a darwin implementation and
  the suite does not skip; if it turns out not to answer on a macOS runner, the
  assertion to keep is the identity one and the thing to change is the reader.
- **The second wedge path W01 narrowed and did not find** is untouched here, and
  the census still points at it.

---

## Remote login shells

**The bug the second machine found, on the day there finally was one.**
`ssh host <command>` hands the command to the **remote user's login shell**, and
W11 sent that shell a POSIX `sh` script — `if … then … elif … else … fi`, with
`$HOME` in it. Against a Fedora Asahi Remix aarch64 host whose login shell is
`/usr/bin/fish`, with amx installed and answering `amx 0.1.0` at
`~/.local/bin/amx`:

```
$ amx --remote saiful@<host>
saiful@<host> has no amx on PATH or in ~/.local/bin.
  fish: Missing end to balance this if statement
  if command -v amx >/dev/null 2>&1; then
  ^^
```

fish parses a script whole before running any of it, so the syntax error meant
**nothing ran at all** — and the exit status a fish gives that is 127, which is
exactly the status amx reads as "the far side has no amx". So `--remote` told a
user a confident, wrong thing about their own machine and then offered to
install a binary that was already there. Every host whose login shell is not
POSIX was unreachable: fish, csh, tcsh.

**Why no suite caught it.** `tests/remote_ssh.rs`'s loopback sshd runs as the
*same* user on the *same* machine, whose login shell is POSIX, and the
bridge-as-child tier never involves a login shell at all. The transport was
real; the shell reading the command never was. That is the shape of the M3 exit
criterion CI cannot cover, and it took a second machine to see it.

**The fix, and it is one function.** `remote::ssh::via_sh` wraps every script as
`/bin/sh -c '<script>'`. The login shell then sees three words — no keyword, no
operator, no expansion — which is a simple command in every shell there is, and
`/bin/sh` gets the script it was written for. The layering is the part that is
easy to get backwards: the outer string is the login shell's to split into
words, the inner one is `sh`'s to parse, and `$HOME`, `$$` and every keyword
belong to the inner one.

**All three commands amx sends go through it**, because auditing them found the
same exposure in two more places. `bridge_script` is the reported one.
`seed::INSTALL_SCRIPT` is worse than the reported one: under fish it dies on
`dir="$HOME/.local/bin"` after the stream has already started, so a seeding
would fail mid-install. The `uname` probe was the only one that happened to work
bare, and it is wrapped anyway — a probe that survives by accident is not a
probe that survives.

**The wrapping is not the whole fix, and the other three constraints were found
by measurement rather than reasoning.** Each one is a shell disagreeing about
what a single-quoted word means, and each was caught by running the exact string
amx emits through a real binary:

- **csh and tcsh reject a newline *inside* single quotes** — `Unmatched '''.`,
  tcsh 6.24. So wrapping a multi-line script in quotes fixes fish and breaks csh
  in the same commit. Every script here is now one line, `;`-separated; `sh`
  reads `;` and a newline identically, so the whole cost is the semicolons.
- **csh expands history inside single quotes and in `-c` scripts alike**, so
  `sq` escapes `!` the way it escapes `'`. A session name holding one arrived as
  `mine!: Event not found.` — and only when a word follows the bang, which is
  why the fixture is `!mine!` and not `mine!`.
- **fish is the one shell that gives `\` a meaning inside single quotes**: `\'`
  and `\\` are escapes there where every POSIX shell reads a backslash between
  quotes as itself. This is the one that bit the fix rather than the bug. The
  command nests two levels of quoting — the name into the script, the script
  into the login shell's word — so a name's own `'\''` becomes a backslash
  *inside* the outer quotes, and fish read it as an escape and handed `/bin/sh`
  a word with unbalanced quotes. `sq` escapes the backslash too, which makes the
  nesting mean the same thing in all six shells.

`a_session_name_crosses_every_login_shell_intact` in `crates/amx/tests/bridge.rs`
is that measurement made permanent: it builds the two nested levels the real
command has and runs them through every one of sh, bash, zsh, fish, csh and tcsh
the machine has, asserting `sh` receives the name byte for byte. It needs no
sshd, so it runs everywhere — and it is the test that went red for fish on the
backslash and for csh on the bang.

The one character with no answer is a newline *in a session name*, which is
legal (a name is validated as a path component) and which csh has no spelling
for inside a word. It reaches sh, bash, zsh and fish and not csh; `sq`'s docs
say so rather than leaving it to be discovered.

**csh and tcsh cannot reproduce the original bug, which is why the new test
measures rather than assumes.** They read a script line by line: handed the old
bridge script they misparse the `if` line, complain — and then run the `exec` on
the *next* line anyway. The wrong thing happens and the right thing happens too,
so a case staged on tcsh alone would have attached happily with the bug in
place. `refuses_posix` in `tests/remote_ssh.rs` sorts the two by running a POSIX
probe and checking whether the marker appears, and the case skips with that
reason when no installed shell refuses one. It then runs the attach through
*every* installed non-POSIX shell, since csh and tcsh are exactly what catches a
fix that trades one broken shell for another.

**How the far side's login shell is staged.** sshd assigns `SHELL` from the
passwd entry after `SetEnv` is applied, so it cannot be set by a variable and
the developer's own is whatever it is. `ForceCommand exec <shell> -c
"$SSH_ORIGINAL_COMMAND"` is the seam that is left, and it is character for
character what sshd would have run had that shell been in the passwd entry. One
line, and the only `ForceCommand` in the file — W11's rule that sshd takes the
first value it obtains for a keyword applies here too.

**Verified.** `AMX_TEST_SSHD=1 cargo test -p amx-rig --test remote_ssh` green
with fish 4.8.1, csh and tcsh 6.24.16 all present, so the attach ran three times
through three login shells. Every test was watched to fail without the change it
protects: reverting `via_sh` to the identity puts the reported fish message back
on the screen and takes the two unit assertions with it, putting the newlines
back in `bridge_script` turns the csh and tcsh legs into `Unmatched '''.`,
dropping the backslash case from `sq` fails the round-trip on fish, and dropping
the bang case fails it on csh. The install script was exercised by hand under
fish both ways — bare it exits 127 with nothing installed, wrapped it lands a
755 file with the streamed bytes intact.

**CI installs fish on the Linux runner** beside openssh-server. Without a shell
that refuses POSIX the case skips, and a skip here is the coverage silently
going away — the workflow comment says which shell and why csh would not do.

**Budget watch.** `tests/remote_ssh.rs` went 319 → 525 and is over the soft
budget. Not split: the second case shares the whole `Sshd` harness with the
first, and the plan names this file as the ssh tier's home. The next task to add
a third case here should expect to lift `Sshd` out rather than trim prose.

**Left open.** `--remote` still cannot carry a session name containing a newline
to a csh host, per above; fixing it properly means encoding the name across the
wire rather than through the shell, which is an interface change and not this
one. And the machine-to-machine smoke that found this is recorded separately in
`m3-live-smoke.md`.

## Settling the load-sensitive reads

Four flakes landed in one day, all with the same shape: the test performs an
action, then reads a state row or a screen region **once**, before the actor
responsible has finished filling it in. One of the four turned out to be hiding
a product bug behind a green suite, which is the reason this shape gets its own
section rather than four separate fixes.

The two owned here are `explain_names_the_matching_rule_and_reports_every_other_one`
(`tests/agents/explain.rs`) and `agent_start_spawns_from_the_registry_labels_the_pane_and_returns_ready`
(`crates/amx-server/tests/agent_verbs.rs`). Neither is reproducible by
re-running: both pass in isolation and under an unloaded full run.

### Two read models, and the one nobody waits on

`agent.start` answers `ready` off `AgentHub`'s `StatusView` — the fast read
model, the one a wait predicate holds directly. `session.state`'s per-pane
`agent` block is a **different** model: `Core`'s mirror of that view, posted by
`agent_hub/commit.rs`'s `mirror` with an un-awaited, droppable `try_send`, and
folded in `core/report.rs`. `docs/08-m2-plan.md` §3 calls the mirror the slower
path "whose mailbox lag is harmless because nothing awaits on it".

Something does await on it. A state read taken straight after a `ready` reply
comes back carrying the pane's label and **no `agent` field at all** — `Null`
where a kind was expected. Reproduced 22 times in 320 constrained runs; the
failure is exactly

```
assertion `left == right` failed:
  {"cols":80,…,"label":"dev","pane":"077c40b0-…","rows":24,"short":2}
  left: Null
 right: "fake"
```

Note what is *not* in that entry.

### The finding: `ready` does not mean addressable

Two of the three sites this closed in `agent_verbs.rs` were not reading state
for its own sake — they were addressing a pane **by name**, which is D-M2-9's
whole argument for names. `address::resolve` in `Scope::Agent` admits a pane
only if its *mirrored* status carries a kind, and it resolves against
`session.state`. So:

```
agent.start  name=dev  ->  readiness: ready, pane: <uuid>
agent.prompt target=dev ->  "dev" names 1 panes and no agent is running in it
```

That is a product looseness, not a test bug. `agent.start` returns evidence
drawn from one read model about a pane that the *other* read model — the one
every subsequent verb addresses through — does not know about yet, and the
reply carries nothing a caller could wait on. The `pane` field is the escape
hatch (a UUID resolves in either scope, deliberately), but a script that uses
the name it just chose is racing. Polling `session.state` is the only
observation available, which is what the suites now do; closing it properly
means either mirroring synchronously before the start reply is written, or
giving the reply a sequence a caller can wait for. Neither is this change.

The same ordering bites one tier further out. `AgentHub::absorb` publishes the
status and attention *events on the bus* and only then calls `mirror`, so
`crates/amx/tests/skill.rs`'s reference notifier — which reads the bus through
`amx events --json` — learns of an enqueue strictly before `session.state` can
report it. That test read the queue once, on the strength of the notification.

### A frame is not a paint

The second mechanism is the pane's own publication. `parser.rs`'s `publish`
fills the snapshot slot **and then** tracks history, and it runs once per parsed
chunk — so a block of lines can reach a reader a few lines at a time, and a
reader that stopped at the first line is holding a screen that is still being
written.

`Rig::block` waited for `"Do you want to proceed?"` and called the dialog up.
The shipped `permission_dialog` rule needs the question **and** one of the
option lines under it, and `prompt_box_idle` has the question in its `not`
clause — so on a half-painted dialog *no rule matches at all* and `agent
explain` answers `matched: null` about a pane that is, by every other measure,
blocked. The waiter was waiting for the wrong fact.

This one does not reproduce on a box whose `/bin/sh` is bash: the paint arrives
as a single 220-byte write and a single read, measured on the master side of a
real pty. It was reproduced instead by splitting `paint_blocked` at exactly the
line boundary the CI failure shows — 20 failures in 20 runs, with a
`region_preview` that ends at the question and a `permission_dialog` verdict of
"none of 3 alternatives hold", byte for byte the reported failure. With the
wait naming both anchors: 0 in 20, and 0 in 12 full-suite runs still split.

### What each site now waits on

| Site | Waits on |
|---|---|
| `Rig::block` (`tests/agents/fixtures.rs`) | the region holding **both** anchors `permission_dialog` needs, not the first line of the dialog |
| `Rig::start_agent` (same) | `session.state` naming the pane's agent, not the `ready` reply |
| `resume.rs`, `agents.rs` `session_ref` reads | the conversation reaching the state tree — a later commit on the same mirror than the kind is |
| `wait_pane_agent` (`agent_verbs/harness.rs`) | the same, and it answers with the entry so a caller has one read to reason about |
| `start_timeout_reports_failure…` | the child's marker on the screen, not the dispatcher's own expired deadline |
| `skill.rs`'s attention read | the state tree carrying the queue the bus already published |

Every one of them fails with what it last saw. That is not decoration: a pane
whose `agent` block has not been mirrored yet and a pane that never had one are
the same `Null` to an assertion, and only the entry tells them apart.

### Rates, on two cores or one

Reproduction is by cpuset, not by repetition — `taskset` plus N copies of the
whole test binary at once, which is the shape `cargo test --workspace` has on a
small runner.

| Suite | Constraint | Before | After |
|---|---|---|---|
| `agent_verbs` | `-c 0`, 8 copies × 8 threads, 320 runs | 22/320 | 0/320 |
| `agents::explain` | `-c 0`, 8 copies × 8 threads, 160 runs | 0/160 | 0/160 |
| `agents::explain`, dialog split at the line boundary | `-c 0-3`, 4 copies, 20 runs | 20/20 | 0/20 |
| `agents` (whole suite), dialog split | `-c 0-3`, 3 copies, 12 runs | — | 0/12 |
| `skill` | `-c 0`, 6 copies × 8 threads, 60 runs | 0/60 | 0/60 |

The `agents` binary at `-c 0` with 6 or 8 copies also fails `resume::*`, but on
`PATIENCE` expiry in `wait_snapshot_holds` — the persist debounce genuinely not
landing inside sixty seconds under 8× oversubscription. That is the constraint
being unreasonable, not a wait reading too early, and it is not touched here.

### Examined and left alone

The three test trees hold 71 reads of a state tree, a screen or a status that
are *not* already inside a wait helper. Every one of those that sits
immediately after a mutating call was traced to the code that fills the field.
**Nine were changed**, across six files — two of them harness-level, so they
settle every call site rather than one. The ones deliberately left:

- **Reads of a synchronous reply.** `label`, `cwd` on split, `worktree`,
  `restore` — `Core` mutates its own state and then answers, so one read is
  right. `check_new_name` is in this group, which is why `agent.start`'s
  duplicate-name refusal is *not* racy even though `Scope::Agent` addressing is.
- **Reads whose precondition an earlier wait strictly implies.**
  `edges.rs`'s interrupted-screen assertion: the status can only have reached
  idle because `prompt_box_idle` matched, and that rule reads lines *below* the
  text being asserted. Likewise `drive.rs` waiting on the last line of a block.
- **`agent_hub.rs`'s spy reads.** `Rig::settle` is 32 `yield_now`s on a
  current-thread runtime with the spy task on it, which is deterministic rather
  than hopeful.

### Left open

Two sites are racy and have no barrier to wait on, so they are recorded rather
than papered over:

- **`handoff_exit.rs`'s `rows_before`.** `history_head`/`history_floor` are
  `Core`'s fold of `HostEvent::Committed`, and `parser.rs`'s `publish` sends
  those events *after* filling the snapshot slot — so `pane.read` showing the
  sentinel does not mean the commits are in `Core`'s mailbox, and they arrive on
  a different queue from the test's own `session.state` call. The assertion is
  `rows_after.0 == rows_before.0`, so a stale-low `rows_before` fails it. There
  is no observation that orders a pane's history commits against a state read;
  the honest fixes are a barrier in the product or a claim that does not compare
  two point reads. Not attempted here.
- **The late-`SubagentStop` negatives** (`edges.rs`, `agents.rs`). Both comment
  that the following call "cannot be answered before the report was handled,
  since both cross the same connection to the same actors". They do not: the
  hook rides a separately spawned `amx _hook` on its own connection. These only
  ever fail *falsely green* — a lagging report cannot make a negative go red —
  so they are not in this flake class, but the stated reasoning is wrong and the
  coverage is weaker than it reads.

**Budget watch.** The two harnesses that grew a wait went 445 → 518
(`crates/amx-server/tests/agent_verbs/harness.rs`) and 443 → 514
(`tests/agents/fixtures.rs`), both just over the soft budget. Not split: what
was added is three wait helpers each sitting beside the reads they guard, and
lifting them out would put a suite's waits in one file and the suite's model of
the session in another. The next task to add a helper to either should expect
to split the rig from the fake-agent plumbing rather than trim prose.

---

---

## CI: the waits that did not keep waiting

Two failures on `main` after M3 landed, one per platform. They look alike from
the outside — a `wait` that exits 1 where it should have answered — and they
have nothing else in common: one is the milestone's headline sentence being
false, the other is a test that was measuring the wrong thing on a platform
where `/bin` is not the `/bin` it was written against.

### The standing wait that died at the swap (ubuntu-latest)

**The mechanism.** A handoff ends every standing wait twice over: the exporter
cancels the connection, which makes `conn/events.rs`'s long polls answer with
`cancelled()`, and then the socket closes. Both reach the client, and which
one *arrives first* is a race decided by how loaded the machine is. W09's
redial keyed on the second — a transport failure — and read the first as a
refusal: a well-formed JSON-RPC error, delivered before the close, which the
CLI reported as `call wait: call failed: the session is shutting down; the wait
was abandoned` and exited 1. On a twelve-core box the wait usually resolves or
the socket usually wins, which is why
`five_agents_ride_a_live_upgrade_and_nothing_notices` was green here and red on
a four-core runner. Reproduced locally by giving the test the runner's shape —
`taskset -c 0-3 handoff_exit --test-threads=3` — at **2 failures in 10 runs**.

**What changed.** The cancellation got an identity a machine can check:
`RpcError::WAIT_ABANDONED` (−32000, inside JSON-RPC 2.0's implementation-defined
server-error range), which `cancelled()` now carries and which
`NetError::is_abandoned_wait` recognises. `cmd/call.rs` treats it as exactly
what it is — the session going away with the question unanswered — so it takes
the same path a dropped connection does: redial, and re-issue if re-issuing asks
the same question. Nothing matches on the message; the string is documentation,
the code is the contract. This is the shape W09 named and left open above ("a
distinct code … meaning 'the session is mid-swap and nothing happened'"), taken
for the wait half only: the mutating verbs still refuse to be re-issued once
their request is written, because `WAIT_ABANDONED` says a *wait* was dropped and
says nothing about a `pane.run` that may already have typed.

Additive under R-M1-8: a client that has never heard of −32000 reads an ordinary
error and behaves exactly as it did before, and a client that knows it talking to
an older server still sees −32603 and behaves exactly as it did before. The code
is frozen in `tests/goldens/proto/response_wait_abandoned.json` — separately from
the generic error envelope, because this is the one error a client *branches* on,
and a silent change to the number would put every standing wait back where it
started.

**One correction fell out of writing the test.** The re-issued call's
`timeout_ms` was computed at the top of the redial loop, before the backoff
sleep, so a verb that lost four connections could outstay its own `--timeout` by
the sum of the backoffs. It is now measured after the sleep: time spent waiting
to redial is the caller's time too.

**The regression test is not a handoff.** Driving this through a real swap would
be staging the race again and hoping it goes the other way, which is how it
stayed hidden. `a_wait_the_session_abandons_is_re_issued_rather_than_reported`
puts a session socket in front of the real binary that always abandons the first
`wait` — with the server's own `cancelled()`, so the fixture cannot drift — and
always answers the second, on a connection that stays whole throughout. It
asserts the verb exits 0 and satisfied, that the wait was issued exactly twice
rather than redialled in a loop, and that the second call's timeout is strictly
less than the first's. Without the client change it fails on every run with the
CI symptom verbatim; with it, and with the constrained repro, `handoff_exit` is
**10 green runs out of 10**.

### The re-issued wait that found no pane (macos-14)

**Not the same bug, and not a product bug.** `a_transition_that_fires_during_
the_gap_still_returns_the_wait` fires its transition by pointing `[terminal]
shell` at a program that exits the instant it starts, so the restored pane's
process is over before the successor accepts anybody. It named that program
`/bin/true`, which is a Linux path: macOS ships `true` as `/usr/bin/true` and
has nothing at `/bin/true`, and that single fact is the one link in this chain
this machine cannot check for itself. Given it, the restore could not spawn the
restored pane's shell on that runner at all, and D-M1-9's
table says what happens next: the pane is pruned, its workspace loses its only
pane and is pruned too, and the session comes back empty. The re-issued wait then
answered `no pane <uuid> in this session` (−32602), which is the *correct* answer
to the question it was asked. The suspected culprit — the debounced save not
reaching disk before the `SIGKILL` — is already answered by the harness:
`Session1::new` waits for the snapshot to name the pane before any test touches
the server.

**Verified on Linux, not on macOS.** Pointing the same test's shell at a path
that does not exist reproduces the macOS failure here exactly, error code and
message included, which pins the mechanism to "the restore could not spawn it"
rather than to anything about darwin's timing. What remains for CI to confirm is
the premise: that `/bin/true` is the path macOS does not have.

**The fix is in the test.** `exits_immediately` writes a two-line `/bin/sh`
script into the environment and points the config at that, so the suite asks the
platform for nothing it does not already depend on — every pane in this file runs
`/bin/sh`. The test now also *states* its precondition: it asserts the restored
session still holds the pane before it reads the wait's answer, so the next time
something prunes it the failure names the prune instead of looking like a client
that lost track of its own question.
