# The M4 live smoke

[11-m4-plan.md](../11-m4-plan.md) §6 puts the live smoke at the *start* of the
milestone rather than at its exit, because four milestones in a row have found
that a green suite hides a non-working feature (DR-4, and M2's, W06's, W12's and
M3's smokes are the four). This file is that record: one baseline taken before
wave 2 opens, then a dated delta at every later wave boundary, in one file for
the milestone — which is what makes "when did this stop working" answerable.

Every section is a run of the **real `amx` binary over its real socket, from
outside the process**. Nothing below is asserted from a unit test.

---

## 1. The wave-1 baseline — 2026-08-09

**Subject.** amx at `0721ac2` (branch `worktree-x00-integration`, debug build)
on Arch Linux 7.1.5, x86_64. The agents are the spike-anchored stand-ins of
`tests/support/agent.rs`, whose script this run lifts verbatim out of that file
so the smoke and the `agents` suite drive the same fake (R-M2-8's standing
constraint: real agent binaries do not exist on runners, and this machine is
running the milestone rather than a person's day).

Wave 1 is still in flight — X02, X03, X04 and X05 are running in their own
worktrees — and this baseline is deliberately taken **against `main` as it
stands**, before any of them merges. That is what makes it a baseline: item 5
in particular measures short numbers on the **stand-in** mapping
(`crates/amx-server/src/actor/core/mod.rs:95-109`), because X05's claim is about
snapshots the stand-in wrote, and after X05 lands there is no way to write one
again.

**Isolation**, the shape m2-live-smoke §10 established and m3-live-smoke reused:
scratch `XDG_RUNTIME_DIR`, `XDG_STATE_HOME`, `XDG_CONFIG_HOME` and `HOME` under
one temp root on a **disk-backed** filesystem (not `/tmp`, which is tmpfs here
and hides persistence races); the session is named `live`; every `AMX_*`,
`CLAUDE*` and `XDG_*` variable is stripped from the driver's own environment
before the server's is built, so nothing of this machine's leaks in.

**Method.** Every fact below arrives through amx's own surface — `amx server`,
`workspace.create`, `workspace.rename`, `agent.start`, `pane.run`, `pane.read`,
`pane.close`, `session.state`, `session.report`, `agent.next`,
`amx events --json`, `amx session stop` — over the real socket. Where a person
would be looking at a terminal, a real `amx attach` runs on a **real
pseudoterminal the driver allocates itself** and its bytes are read back and
reconstructed into a screen. This machine is headless with a locked graphical
session, so item 2's two terminals are two ptys, not two emulator windows; that
is recorded as what it is, and it is the right instrument anyway — what is under
test is the client's own projection, not any emulator's.

### 1.1 Verdict

| # | §6's item | Result |
|---|---|---|
| 1 | 25 agents across 5 workspaces, several blocked | **holds** — 25 started ready in 0.5 s, 5 blocked across 4 workspaces, queue in block order |
| 2 | a real attach at 200×50 and a second at 45×20 | **holds, and the D-M4-7 letterbox is observed** (§1.3) — measured, not argued |
| 3 | `session.state` and `agent.next`, timed | **holds** — 8 ms and 6 ms at 25 agents; the hand-assembled D15 table costs 161 ms (§1.4) |
| 4 | `amx events --json` throughout, gapless-or-gap-marked | **holds** — 1244 deliveries, 0 gaps, 0 non-NDJSON lines on stdout, and the one sequence discontinuity is a cold restart the relay announces (§1.5) |
| 5 | short numbers across a restart | **holds** — every number unchanged across a restart, on a snapshot the stand-in wrote (§1.6) |
| 6 | the old server's exit status and drain census on `session stop` | **holds** — two stops, both exit 0, no census file, no census log line (§1.7) |

Three things worth a later task's attention came out of it; they are §1.8, and
they are in [m4-wave-outcomes.md](m4-wave-outcomes.md) with the tasks that own
them.

### 1.2 Twenty-five agents

Five workspaces (`api`, `web`, `infra`, `docs`, `exp`), five agents each. Each
workspace's own root pane is a plain shell and is closed once its agents are in,
so the session is exactly the 25 panes D15's scenario describes.

```
  api-1: pane d67c046b short 2 ready
  api-2: pane e01de09a short 3 ready
  …
  exp-5: pane …        short 30 ready
25 agents started in 0.5s
all 25 panes painted an idle prompt box
```

`ready` is `agent.start`'s own answer; the run does not trust it on its own — it
waits for every one of the 25 to carry an `agent` block in `session.state` and
to have actually painted `? for shortcuts`, for the reason
`tests/agents/fixtures.rs:126-143` records at length.

Five are then blocked with the scripted `ask`, each waited to three separate
facts (the status, both anchors of the dialog, and the queue), so block order is
established rather than hoped for:

```
attention queue, head first:
  api-1      blocked
  web-2      blocked
  infra-3    blocked
  infra-4    blocked
  exp-1      blocked
per workspace: api 5 agents ⚑1  web 5 agents ⚑1  infra 5 agents ⚑2  docs 5 agents ⚑0  exp 5 agents ⚑1
```

That distribution is deliberate: `docs` has no blocked agent, which is the
workspace X11's status line has to render **without** a count, and `infra` has
two, which is the one whose count must not be a global count in disguise.

### 1.3 Two ptys, and the letterbox D-M4-7 predicted

A real `amx attach` on a 200×50 pty, then a second on a 45×20 pty. Pane sizes
are read back from `session.state` and polled until two consecutive reads agree,
so the resize debounce is not raced.

**At 200×50**, the focused workspace's five panes:

```
  api-1      47x98
  api-2      47x48
  api-3      47x23
  api-4      47x11
  api-5      47x10
```

and the client draws them, the blocked pane's dialog legible in the first box:

```
│● Bash(echo spike-permission-probe)                        …│…
│  Do you want to proceed?                                  …│…
│❯ 1. Yes                                                   …│…
│  2. No                                                    …│…
│  (esc to cancel)                                          …│…
└──────────────────────────────────────────────────────────…┘…
 api · api-1 · blocked ⚑5
```

**At 45×20**, the second client declares its viewport last and so takes size
authority (04 §3 — the pane grid follows the most-recently-active client), and
the same five panes become:

```
  api-1      17x21
  api-2      17x9
  api-3      17x4
  api-4      17x1
  api-5      47x10
```

Its whole screen, all twenty rows, is this:

```
┌─────────────────────┐┌─────────┐┌────┐┌─┐┌┐
│                     ││for 3s   ││────││ │││
│                     ││─────────││────││?│││
│                     ││─────────││────││ │││
│                     ││─────────││────││f│││
│                     ││─────────││────││o│││
│                     ││──       ││────││r│││
│                     ││❯        ││──  ││ │││
│                     ││─────────││  ⏸ ││s│││
│● Bash(echo spike-per││─────────││manu││h│││
│mission-probe)       ││─────────││al m││o│││
│─────────────────────││─────────││ode ││r│││
│─────────────────    ││──       ││on ·││t│││
│  Do you want to proc││  ⏸ manua││ ? f││c│││
│eed?                 ││l mode on││or s││u│││
│❯ 1. Yes             ││ · ? for ││hort││t│││
│  2. No              ││shortcuts││cuts││s│││
│  (esc to cancel)    ││         ││    ││ │││
└─────────────────────┘└─────────┘└────┘└─┘└┘
 api · api-1 · blocked ⚑5
```

That is the phone case as it is today: five pane boxes in 45 columns, the widest
of them 21 cells, one of them one cell wide, one drawing the word `shortcuts`
down a vertical column a letter at a time, and the permission dialog the user is
supposed to answer word-wrapped across four rows. It is the surface D14 exists
to replace, and it is now on the record rather than argued from.

**The letterbox itself is on the wide client, and it is exact.** With the narrow
client holding size authority, the 200×50 client keeps drawing its own
projection — a 98×47 pane box — and paints the 21×17 grid the server is now
publishing inside it, centred:

```
row 24│                                      ● Bash(echo spike-per      │
row 25│                                      mission-probe)             │
row 28│                                        Do you want to proc      │
row 29│                                      eed?                       │
row 30│                                      ❯ 1. Yes                   │
row 32│                                        (esc to cancel)          │
```

Twenty-one columns of content starting at column 38 of a 98-column box, and
seventeen rows inside forty-seven. Correct, and useless. D-M4-7 predicts this
shape for the *phone* under a client-side-only narrow policy; what the baseline
shows is that the tree already produces it today, in the other direction, for
any two clients of different sizes. Both halves of the seam are confirmed
against source:

- the client declares all five panes of the focused workspace along with its
  size — `report_viewport` builds `Viewport { rows, cols, panes }` from
  `ws.layout.panes()` (`crates/amx-client/src/app/binds.rs:131-147`);
- the server reads only two of the three — `handle_viewport` takes `params.rows`
  and `params.cols` and never touches `params.panes`
  (`crates/amx-server/src/actor/core/view.rs:144-157`).

That is X00's seam 4, measured before either half of it is built.

**One behaviour worth naming before X12 meets it.** `api-5` stayed at `47x10`
under the 45-column client — the size the *departed* wide client gave it. That
is deliberate and documented: a pane whose slot insets to zero in either
dimension is skipped, "a pane squeezed out of visible space keeps its last size:
a 0x0 PTY starves the process for nothing"
(`crates/amx-server/src/actor/core/view.rs:192-198`). At 45 columns the fifth
slot is two cells wide and insets to zero, so the rule fires. It is not a
defect; it is a case the narrow projection has to have an answer for, because
under D-M4-7 a single-pane viewport makes the other four panes' sizes a question
nobody has asked yet.

Both clients detached with `ctrl+a d` and exited **0**.

### 1.4 What the surfaces cost today

The number wave 3 is to be compared against, at 25 agents in one session:

```
session.state x7: 7 8 8 8 8 9 10 ms      (reply 12787 bytes, 5 workspaces, 25 panes)
agent.next  x3:   5 6 6 ms
```

and `agent.next` answers with the queue depth the status line already renders:

```
{"pane": "d67c046b-…", "seq": 686, "waiting": 5, "workspace": "4659e806-…"}
```

**And the number R-M4-7 is really about.** D15's table needs a last line per
agent, and the only way to assemble it today is one `pane.read` per pane on top
of one `session.state`:

```
session.state + 25 pane.read (D15's table, assembled by hand): 161 ms
  api-1    last non-empty row: '  (esc to cancel)'
  web-2    last non-empty row: '  (esc to cancel)'
  docs-1   last non-empty row: '  ⏸ manual mode on · ? for shortcuts'
```

161 ms for one frame of the table, twenty times the cost of the state read that
carries everything else. A surface refreshing that way at 4 Hz would spend
two-thirds of a core doing it, which is exactly the shape D-M4-2 refuses and
R-M4-7 warns about. `agent.list` has a number to beat, and the three last lines
above are the literal strings X10's `last_line` must reproduce for those panes.

### 1.5 The event stream

`amx events --json` ran from before the first workspace existed to after the
last `session stop`, with **stdout and stderr kept apart** — which matters,
because the command's contract puts deliveries on stdout and everything else on
stderr (`crates/amx/src/cmd/events.rs:161-165`), and a driver that merged them
would report a defect that is its own.

```
stdout carried 1244 lines, 0 of them not NDJSON, 0 of them gap markers
2 strictly contiguous run(s):
  seq 11..1093 (1083 deliveries)
  seq 122..282 (161 deliveries)
```

and on stderr:

```
subscribed at seq 10
the session restarted; its sequence numbers begin again
subscribed at seq 121
```

So: no consumer fell behind the replay ring at 25 agents under this load, no
delivery was lost, and the one discontinuity in the sequence space is the cold
restart of §1.6 — which the relay noticed by `Welcome.session` and reported
rather than resuming across (`crates/amx/src/cmd/events.rs:136-147`). The
subscriber **survived the restart**: it redialled, resubscribed and kept
printing, which is the contract X16's `--watch` is supposed to package rather
than reinvent.

The kinds delivered, for the wave that adds fields to two of them:

```
agent_identified 51   agent_status 67   attention_enqueued 5
client_attached 335   client_detached 345   focus_changed 6
history_committed 103   history_evicted 47   history_invalidated 47
layout_changed 6   pane_created 26   pane_damage 116   pane_exited 6
pane_renamed 26   pane_resized 48   workspace_created 5   workspace_renamed 5
```

Five `attention_enqueued` and no `attention_dequeued`: the five blocked agents
were never answered, which is the state the session was left in. X06's identity
block lands on those two event kinds, and this is what they carry today.

### 1.6 Short numbers, on the stand-in, across a restart

The point of taking this before X05 merges. Numbers as issued:

```
workspaces: 1=api, 2=web, 3=infra, 4=docs, 5=exp
panes: 2=api-1 3=api-2 4=api-3 5=api-4 6=api-5 8=web-1 … 30=exp-5
```

The gaps in the pane sequence (7, 13, 19, 25) are the five workspace root panes
that were closed; 1 is the session's first pane. Then one pane closed and one
started, to see what the stand-in does with a released number:

```
closed api-4 (short 5), started api-6 -> pane short 31
```

**The released number is not reused** — the stand-in is monotonic and says so
(`crates/amx-server/src/actor/core/mod.rs:95-109`), and 04 §6's mapping is
lowest-free-number. That is DR-6 in one line, measured. X05's acceptance is that
a released number *is* reused by the next assignment; this is the "before".

The snapshot on disk carries the same numbers the stand-in issued:

```
[(2, 'api-1'), (3, 'api-2'), (4, 'api-3'), (6, 'api-5'), (8, 'web-1'), (9, 'web-2')]
```

which is the durable half of D-M4-12's worry — every day the milestone runs adds
snapshots shaped by a mapping nobody implemented. This is one of them, kept.

Then `amx session stop`, a fresh `amx server` on the same state roots, and:

```
workspaces after: 1=api, 2=web, 3=infra, 4=docs, 5=exp
panes after:      2=api-1 3=api-2 4=api-3 6=api-5 8=web-1 … 30=exp-5 31=api-6
workspace shorts unchanged: True
pane shorts unchanged: True
session report after the restart: no losses to report
```

Every number survived the restart unchanged, including `31` — the one issued
after a close. That is the property X05 must still hold *after* it swaps the
stand-in for `ShortNumbers`, on a snapshot the stand-in wrote, and the file this
run left behind is such a snapshot.

### 1.7 DR-11's watch: two clean stops

The milestone of field time R-M4-10 asks for, first entry:

| Stop | Exit | `drain-census` file | Census log line |
|---|---|---|---|
| the pre-restart `session stop` | **0** | absent | none |
| the final `session stop` | **0** | absent | none |

Each server's whole log for its lifetime is one line:

```
INFO amx_server::session::scaffold: SIGTERM: shutting the session down
```

No `the shutdown drain has not emptied` warning, and no `drain-census` left in
the runtime directory — the two places `Runtime::census` writes
(`crates/amx-server/src/runtime.rs:205-231`). Two sessions, of six and of 25
panes, stopped clean.

### 1.8 What this baseline hands the later waves

Recorded in [m4-wave-outcomes.md](m4-wave-outcomes.md) against the tasks that
own them; here so the numbers sit beside the run that produced them.

1. **X10/X14/X16 have a cost to beat: 161 ms.** One `session.state` plus 25
   `pane.read`s is the only way to build D15's table today, against 8 ms for the
   state read alone. R-M4-7's "cheap per call, quadratic per pane" is not a
   worry about the future; it is the present.
2. **X12 inherits a pane the projection skips.** A slot that insets to zero
   leaves its pane at whatever size the last client that could see it gave it
   (`crates/amx-server/src/actor/core/view.rs:192-198`). At 45 columns and five
   panes that is one pane in five. The single-pane viewport rule D-M4-7 adds has
   to say what happens to the other four.
3. **X16 should decide what a *restart* looks like on stdout.** A cold restart
   resets the sequence space; the relay handles it correctly and says so — on
   **stderr** (`crates/amx/src/cmd/events.rs:144-147`). A consumer reading only
   stdout sees seq 1093 followed by seq 122 with no marker between them. That is
   not a `gap` and the doc comment is right that it is not one, but `--watch` is
   a consumer that reads its own stream, and this is the case it will meet.

Nothing here contradicts a D1–D15 decision, and nothing here was fixed by this
task: X00 owns the seams and the record, and each of the three has a task that
owns the file.

---

## 2. The wave-1 delta — 2026-08-09

**Subject.** amx at `6f4fb5d` — wave 1 merged: X02's contracts, X03's doc
corrections, X04's flake paydown and X05's `ShortNumbers`, plus the one
integration fix the merge itself needed (§2.6). Same machine, same isolation,
same driver: §1's six items are re-run *unchanged*, so a number that moved is a
number the wave moved, and the wave's own new surface is smoked beside them.

`cargo test --workspace` on this tree: **122 suites, 808 passed, 0 failed**;
`cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` clean.

### 2.1 Verdict

| # | What | Result |
|---|---|---|
| 1–6 | §1's baseline, re-run whole | **holds** — every item, with one number changed on purpose (§2.2) |
| 7 | DR-6: a released short number is reused | **holds** — the close→create that answered `31` in §1.6 answers `1` (§2.2) |
| 8 | short numbers restored from a **stand-in-written** snapshot | **holds** — X05's hardest acceptance clause, run against the actual file §1 left behind (§2.3) |
| 9 | X02's new wire surface answers by name from the real binary | **holds** — `agent.list` refuses with the seam code and names its owner (§2.4) |
| 10 | `attach --pane <short>` end to end | **holds** — X05 landed it uncovered past the parse; it renders and detaches (§2.5) |
| 11 | DR-11's watch | **holds** — four more clean stops, no census (§2.7) |

### 2.2 What the baseline's own numbers did

Re-run identically. Everything held: 25 agents ready in 0.5 s, the same
five-blocked distribution across four workspaces, both ptys, the same letterbox,
the event stream gapless, both servers exit 0. Two numbers moved, and one of them
is the wave:

```
                          §1 baseline (0721ac2)     §2 wave 1 (6f4fb5d)
session.state x7          7 8 8 8 8 9 10 ms         7 8 9 11 12 13 14 ms
agent.next x3             5 6 6 ms                  10 8 10 ms
state + 25 pane.read      161 ms                    214 ms
close api-4, start api-6  short 31                  short 1
```

The timings moved with the machine, not with the tree — this run shared the box
with four wave-2 worktrees compiling — and the milestone's own load-sensitivity
lesson applies to its smoke as much as to its suites. The figure that matters is
unchanged in shape: **the hand-assembled D15 table still costs an order of
magnitude more than the state read that carries everything else** (214 ms against
9), and `agent.list` still has that to beat.

**The last row is DR-6, visible from outside.** In §1.6 the stand-in answered
`31` — a monotonic counter's next number. It now answers `1`, the lowest number
free at that moment (the session's original first pane, closed long before). The
whole numbering follows:

```
§1  panes: 2=api-1 3=api-2 4=api-3 5=api-4 6=api-5 8=web-1 … 30=exp-5   (holes at 7,13,19,25)
§2  panes: 1=api-6 2=api-1 3=api-2 4=api-3 6=api-5 7=web-1 … 26=exp-5   (one hole, at 5)
```

Each workspace's root shell is closed once its agents are in. Under the stand-in
its number was gone forever; under `ShortNumbers` the next workspace's first
agent takes it, so twenty-five agents occupy 1–26 instead of 2–30. Across the
restart every number came back unchanged, `session report` says `no losses to
report`, and the run's own `pane shorts unchanged: True` is now over the real
mapping.

### 2.3 The stand-in's snapshot, restored by the real mapping

X05's acceptance asks for numbers that survive "a session whose snapshot was
written by the stand-in". §1 left exactly one such file behind — 25 panes, shorts
`2,3,4,6,8,…,31`, a high-water mark of 31 with six holes below it — and it was
kept aside before this run overwrote its scratch tree. Restored under `6f4fb5d`:

```
it holds 25 panes, shorts [2, 3, 4, 6, 8, 9, 10, 11, 12, 14, 15, 16, 17, 18,
                           20, 21, 22, 23, 24, 26, 27, 28, 29, 30, 31]
the free numbers below its high-water mark are [1, 5, 7, 13, 19, 25, 32]
restored shorts unchanged: True
  workspace 6 brought pane short(s) [1]
  workspace 7 brought pane short(s) [5]
  workspace 8 brought pane short(s) [7]
panes created after the restore took [1, 5, 7], lowest-free order [1, 5, 7]
```

Every recorded number came back on the number it was written with, and the three
assignments after the restore walked the holes the stand-in had left, in order,
before going anywhere near 32. That is both halves of 04 §6 at once — *restored
numbers are held* and *the next number is the lowest free* — measured on a file
the implementation under test did not write, which is the one thing no test in
the repository can arrange any more.

### 2.4 X02's surface, answered by the binary

Every row is reachable by name and refuses in the shape the plan specifies:

```
$ amx agent list
amx: call agent.list: call failed: agent.list is tabled but not wired yet; X10 owes it (-32099)

$ amx agent list --params '{"workspace":"…"}'
amx: call agent.list: call failed: agent.list is tabled but not wired yet; X10 owes it (-32099)

$ amx agents
amx: `amx agents` is not wired yet; X16 owes it

$ amx keys
amx: `amx keys` is not wired yet; X07 owes it
```

`-32099` and not `-32000`: X02's divergence, and it is the right one — `-32000`
is `WAIT_ABANDONED`, whose contract is *redial and ask the same question again*,
so an unwired row answering it would put a caller in a loop.

**`PaneState.mouse` is absent for a pane that asked for nothing**, which is what
an additive optional field must look like on the wire:

```
the pane's state entry keys: ['agent', 'cols', 'cwd', 'history_floor',
                              'history_head', 'label', 'pane', 'rows', 'short']
```

**`agent.next`'s scope parses and is ignored, and that is now measured rather
than asserted.** With the only blocked agent in `api`, asking for `web`'s queue:

```
$ amx agent next --workspace <web>
{"pane": "e1b6e865-…", "seq": 157, "waiting": 1, "workspace": "c9da964b-…"}   ← api's pane
```

X02 reported the flag as already built; this is the "before" X17's acceptance is
measured against — a scoped call that focuses another workspace's agent and
reports the global queue depth.

### 2.5 `attach --pane <short>`, the ten lines nobody covered

X05's own hand-off names this: the parse is covered inline, the resolution rule
is covered over a socket, and the code between them — `resolve_pane` asking
`session.state` and handing a pane id to `viewport::one_pane` — is not, because
`crates/amx/tests/**` is X07's this wave. An integration seam with no owner for a
wave is what X00 is for. Run on a real 80×24 pty against pane short `2`:

```
(nineteen blank rows)
✻ Worked for 3s
──────────────────────────────────────
❯
──────────────────────────────────────
  ⏸ manual mode on · ? for shortcuts
```

A pane, full screen, no border and no status line, addressed by a number a human
would type — and `prefix+q` detached it with exit **0**. The chord is the
single-pane mode's own (`DETACH_PANE`, `crates/amx/src/cmd/viewport.rs:35,57`),
not the `prefix+d` the full client uses, and the difference cost this run one
wrong reading before the source was checked. Recorded because it is the sort of
thing a `--watch`-style consumer or a doc will get wrong next.

Short-number resolution over the same socket:

```
$ amx pane read 1              # after the pane numbered 1 was closed
amx: call pane.read: call failed: no pane is numbered 1 (-32602)
$ amx agent start 3 --kind fake
amx: call agent.start: call failed: "3" is a short number, which is checked first and would shadow it (-32602)
```

A released number refuses rather than resolving to whatever the slot held next,
and the resolution order is defended at the naming boundary rather than at
lookup — X05's recorded behaviour change, working.

### 2.6 The integration break, and whether the ledger would have caught it

One break at the merge, fixed by the orchestrator in `6f4fb5d` rather than by a
wave task: X02 added `mouse: Option<MouseMode>` to `PaneState`
(`crates/amx-proto/src/control/session.rs:128`, honouring X01's "cannot be a
boolean" hand-off), and X05's **new** fixture
`crates/amx-server/tests/short_numbers.rs:56` builds a `PaneState` literal. The
workspace did not compile until `mouse: None` was added — one line.

That is seam 1 firing exactly where D-M4-10 says it should. **The field ledger
would not have caught it, and could not have.**

- `FIELD_LEDGER` (`tests/hygiene/ledgers.rs:53-84`) asserts that the *declaring*
  file still contains `pub mouse: Option<MouseMode>`. That was true before the
  fix and after it. The ledger's own doc says so: what it checks mechanically is
  that a frozen field still exists where its row claims, and "whether the reader
  arrived — that is the integration owner's".
- The ledger is a test, and the break was a **compile** error in a test target.
  Nothing under `tests/` runs on a tree that does not build.

What caught it is what should catch it. `PaneState` is a plain struct — no
`#[non_exhaustive]`, no `Default` — so `rustc` names every construction site that
has not been updated, at every site, with no test needed. The ledger is not a
substitute for the compiler and was never meant to be one.

**The gap the break exposes is in the plan, not in the ledger.** X02 found the
field's other construction sites and filled them, including two outside its own
file list — `crates/amx-server/src/actor/core/view.rs:98` (X05's file this wave)
and `crates/amx/tests/layout_file.rs:44` — and declared both in
[m4-wave-outcomes.md](m4-wave-outcomes.md). It could not declare the last one,
because the file did not exist when X02 was written: X05 created
`short_numbers.rs` in the same wave. §6's declared-hand-off mechanism enumerates
files that exist at planning time; a *new* file a concurrent task creates against
a shape another task is changing in the same wave is outside its reach by
construction.

No new mechanism is proposed for it, deliberately. The cost was one line, found
by the first build of the merge; a rule that made it findable earlier would have
to predict files nobody has written yet. What is worth carrying forward is the
shape, because M4 freezes six additive fields and four of them are on structs
built by literal in suites: **an additive field on a wire struct costs an edit at
every construction site in the workspace, and the ones in a wave-mate's
not-yet-written test files are the integration owner's to expect at the merge.**

### 2.7 DR-11's watch: four more clean stops

| Run | Stops | Exit | `drain-census` | Census log line |
|---|---|---|---|---|
| §2.2's baseline re-run | 2 | **0**, **0** | absent | none |
| §2.4–2.5's delta session | 1 | **0** | absent | none |
| §2.3's restore session | 1 | **0** | absent | none |

Six clean stops across the milestone so far, over sessions of 2, 3, 25 and 26
panes. Nothing has fired.

### 2.8 What this delta hands the later waves

1. **X17's "before" is on the record** (§2.4): a scoped `agent.next` today
   focuses another workspace's agent and reports the global depth.
2. **X10 still has the table's cost to beat.** §1.4's 161 ms and this run's
   214 ms bracket what the machine does to the figure; neither is within an
   order of magnitude of the state read beside it.
3. **X07 inherits a smoked path, not an assumption** (§2.5). Whether
   `attach --pane <short>` also gets a suite test is X07's call; it is no longer
   a path nobody has run.
4. **No wave-1 field has a reader yet, and that is correct.** `reason`, `since`,
   the attention identity block, `now` and `mouse` are all frozen, all in the
   ledger, and every reader is wave 2 or later. The ledger is walked and
   non-empty; X00 closes no row at this boundary.

## 3. The wave-2 delta — 2026-08-09

**Subject.** amx at `335cc27` — wave 2 merged: X06's hub facts, X07's
keybindings-as-config, X08's retired `Scrolled` surface and unbound-channel
decision, X09's one client `Effect` and retriable refusal. Same machine, same
isolation, same driver: §1's six items are re-run *unchanged* beside the wave's
own new surface.

`cargo test --workspace` on this tree: **128 suites, 854 passed, 0 failed**;
`cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` clean.

### 3.1 Verdict

| # | What | Result |
|---|---|---|
| 1–6 | §1's baseline, re-run whole | **holds** — every item, every number inside §2's spread (§3.2) |
| 7 | X06: `reason` names the detector that fired | **holds** — the hook event on a hook-asserted entry, the manifest rule on a screen-owned one, absent for a probe-derived state (§3.3) |
| 8 | X06: `since` is an entry edge, not a clock tick | **holds** — unmoved across 1.5 s of re-evaluation, moved by a real transition (§3.3) |
| 9 | X06: an attention event renders with no follow-up call | **holds** — `api/tests blocked (PermissionRequest)` off one event (§3.4) |
| 10 | X06: the names mirror survives a cold restore | **holds** — R-M4-4's load-bearing seed, measured on a restored pane (§3.4) |
| 11 | X07: `amx keys`, a rebound prefix, a malformed section | **holds** — all three, the last one inert as the lenient rule promises (§3.5) |
| 12 | X07: the rebound prefix takes effect on a real pty | **holds** — `ctrl+a d` does nothing, `ctrl+t d` detaches with 0 (§3.5) |
| 13 | X08: a client under an 8 MiB flood | **holds** — alive, still painting, history window advanced (§3.6) |
| 14 | X09: `attach --pane` rides a server swap | **holds** — DR-16's headline claim, live (§3.7) |
| 15 | DR-11's watch | **holds** — five more clean stops, no census (§3.9) |

Three findings came out of it, none of them a wave-2 regression and all three
outside any wave-3 task's scope: §3.7's second handoff, §3.8's dropped first
keyframe, and §3.5's two prefixes. They are in
[m4-wave-outcomes.md](m4-wave-outcomes.md) under this boundary.

### 3.2 The baseline, re-run

```
                          §1 (0721ac2)      §2 (6f4fb5d)       §3 (335cc27)
session.state x7          7 8 8 8 8 9 10    7 8 9 11 12 13 14  9 9 11 11 11 13 13 ms
agent.next x3             5 6 6             10 8 10            6 6 7 ms
state + 25 pane.read      161 ms            214 ms             164 ms
close api-4, start api-6  short 31          short 1            short 1
```

Twenty-five agents ready in 0.5 s, the same five-blocked distribution across
four workspaces, both ptys, the same letterbox — `api-1` at 17×21 inside a
45-column client while `api-5` keeps the 47×10 it was last commanded
(`crates/amx-server/src/actor/core/view.rs:192-198`, deliberate) — the event
stream 1234 lines with 0 gaps and 0 non-NDJSON, every short number unchanged
across the restart, both servers exit 0. Nothing wave 2 landed moved any of it.

### 3.3 `reason` and `since`, on the wire

The `agent` block on `session.state`, as three reads found it:

```
idle, straight after agent.start
  {"cause":"probe","kind":"fake","since":1786233621913,"state":"idle","transition_seq":25}

blocked, after `ask`
  {"attention":0,"cause":"hook","kind":"fake","reason":"PermissionRequest",
   "since":1786233622088,"state":"blocked","transition_seq":76}

idle again, after `stop`
  state idle, reason 'prompt_box_idle', since 1786233623735
```

Three things worth naming, because a doc reading D15's sketch would guess two of
them wrong.

**`reason` is the detector's own name and the detector that won is the hook.**
D-M4-3 says the string is "the winning manifest rule for a screen-owned state,
the hook event for a hook-asserted entry", and that is exactly what arrived:
`PermissionRequest` — the `HookEvent` variant
(`crates/amx-core/src/agent/mod.rs`'s `reason`, written by the hub) — and not
the `permission_dialog` rule that 10 §D15's sketch and this plan's own D-M4-3
example both use. Both are correct and they are different strings for the same
screen: tier 1 gets there first (V01 §3 M3 measured the hook 8–14 ms ahead of
the paint), so on any agent that emits a `PermissionRequest` hook the wire will
say `PermissionRequest`. `permission_dialog` is what a pane that blocks *without
a hook* will report. §7's by-hand item 1 — "every `reason` names the rule or
hook that actually fired" — should be read expecting both vocabularies, and
X14's and X16's renderers must not assume the manifest one.

**A probe-derived state carries no `reason`.** The opening `idle` above has
`cause: probe` and no `reason` at all, and the `idle` after a real `stop` has
`prompt_box_idle`. Absent-for-tier-3 is what D-M4-3 asked for; absent-for-probe
is the same rule reaching one case the decision did not name.

**`since` is an entry edge.** Held at `…622088` across 1.5 s and a `pane.read`
that re-evaluated the screen, moved to `…623735` by the transition to idle.

### 3.4 An attention event a notifier can render alone, restore included

```
attention_enqueued  {"name":"backend","pane":"31f2…","reason":"PermissionRequest",
                     "since":1786233622088,
                     "workspace":{"id":"871e…","name":"api"}}
attention_dequeued  {"name":"backend","pane":"31f2…","reason":"prompt_box_idle",
                     "since":1786233623735,
                     "workspace":{"id":"871e…","name":"api"}}
attention_enqueued  {"name":"tests","pane":"03b6…","reason":"PermissionRequest",
                     "since":1786233623809,
                     "workspace":{"id":"871e…","name":"api"}}

rendered from the event alone: 'api/tests blocked (PermissionRequest)'
```

D15's requirement, met: no follow-up query. The dequeue carries the identity
block too, and carries the *new* reason and entry edge rather than the ones the
pane was queued with — which is the honest reading of "what its status was
asserted by when it left the queue".

**R-M4-4's load-bearing half, measured.** The plan says a label that only
arrives by rename event would be absent for every restored pane, and that the
seed at `PaneStarted`/`inherit` is therefore load-bearing rather than a
convenience. The session was stopped, restarted from the snapshot, and the
restored pane blocked again:

```
attention_enqueued  {"name":"backend","reason":"PermissionRequest",
                     "workspace":{"id":"871e…","name":"api"}}
  a restored pane renders as: api/backend (PermissionRequest)
```

Both names came back. Nothing in this session ever sent a rename after the
restore, so the mirror is being seeded on the import path and not folded off a
`PaneRenamed` that never came.

**And `since` after a cold restore is the restore, said out loud.** The restored
pane's block reads `since 1786233634350`, which is when the new server derived
the status — not when the agent went idle before the stop. That is R-M4-4's
"since this server started tracking it" fallback, and it is what the wire now
carries. A surface rendering `now − since` will say a freshly restored agent has
been idle for seconds. Correct for the field's definition, and worth one
sentence wherever an age is rendered (X11, X14, X16).

### 3.5 `amx keys`, and a prefix that is data

```
$ amx keys                                    $ amx keys        # with [keys]
prefix ctrl+a, from shipped                   prefix ctrl+t, from config.toml

key     action            source              key     action            source
ctrl+a  literal           prefix escape       ctrl+t  literal           prefix escape
a       next-attention    shipped             a       next-attention    shipped
d       detach            shipped             d       detach            config.toml
p       picker            shipped             j       picker            config.toml
v       split-vertical    shipped             p       picker            shipped
w       navigate          shipped             v       split-vertical    shipped
x       split-horizontal  shipped             w       navigate          shipped
z       zoom              shipped             x       split-horizontal  shipped
                                              z       zoom              shipped
```

The prefix moved, the escape row moved with it, and the source column says which
line came from the file. 04 §7's promise, finally answerable by a command.

**A malformed section is inert, and says so.** With `prefix = 42` and
`bind = "not a table"`:

```
prefix ctrl+a, from shipped
… the shipped table, unchanged …
keys: invalid type: string "not a table", expected a map
  in `bind`
```

`amx keys --json` either side differs in exactly one key — `rejected` — and not
in a single binding. That is the config module's lenient per-section rule
(`crates/amx-core/src/config/mod.rs:8-23`) working through a second reader, and
the diagnostic is on the surface rather than in a log.

**On a real pty**, with `prefix = "ctrl+t"` and a 24×100 client attached:
`ctrl+a` then `d` left the client running — the old prefix is an ordinary byte
now — and `ctrl+t` then `d` detached it with exit **0**. With no `[keys]` at all,
`ctrl+a ctrl+a` still forwards one literal prefix and does not detach.

**Two prefixes now exist and only one of them is data.** `amx attach --pane`
runs no mode machine and recognises its own chord out of two constants —
`PREFIX = 0x01` and `DETACH_PANE = b'q'`
(`crates/amx/src/cmd/detach.rs:12-15`) — which X07 did not make configurable and
was not asked to. So a user who rebinds the prefix to `ctrl+t` rebinds it for
`amx attach` and not for `amx attach --pane`, where it stays `ctrl+a q`.
Verified from both sides: `ctrl+a q` detached a `--pane` client with exit 0 on
this tree, with and without a swap in between. Recorded for X20, which documents
the `[keys]` section, and for whoever owns `detach.rs` next.

### 3.6 A client under flood, after the `Scrolled` retirement

DR-12's original symptom was a frame on a channel the client did not know, three
rounds in thirty, under an 8 MiB flood. The refusal it now gets is a reader-level
one (`NetError::UnboundChannel`) and the drop is `apply`'s
(`crates/amx-client/src/stream.rs:13-43`), and both are CI's —
`crates/amx-client/tests/unbound_channel.rs`. What the smoke can watch from
outside is the consequence, so it watched that: an attached 30×120 client while
8 MB of `/dev/urandom` went through `od -c` in a neighbouring pane.

```
8 MiB of flood: the client is alive: True; it took 1075838 bytes
backend's history window before: head 229 floor 108
backend's history window after:  head 367 floor 240
```

Alive, still painting, and the committed window advanced and evicted under it —
which is the path DR-7's retired `Scrolled` notice used to feed and that
`history.committed` now feeds alone (`stream.rs`'s `apply_grid`, whose scroll arm
went with the tag). No client died in any run of this note's driver.

### 3.7 `attach --pane` rides a swap, and the second swap that is refused

A `--pane` client on a 30×100 pty, a `session.handoff` to a staged copy of the
same binary, and `pane.run`s issued in a loop while it ran:

```
`session handoff` exit 0: {"accepted": true, "seq": 392}
3 `pane.run`s issued across the swap, 1 failed
  (1, 'amx: the session went away while pane.run was in flight, and re-issuing
       it could repeat what it already did: … Broken pipe (os error 32)')
the exporter exited 0
ms      path_exists  listening
    95  True        True
session id before ffa357a7-… after ffa357a7-… (same: True)
the --pane client survived the swap: True
  a post-swap paint reached the client: True
```

DR-16's headline claim, live: the single-pane attach came back on the successor,
was repainted, and a sentinel painted *after* the swap reached the screen — so it
is live rather than frozen, which is the M3 failure this milestone inherited the
memory of. `prefix+q` detached it afterwards with exit 0.

**The one failure is the documented refusal, not a defect.** `pane.run` is
deliberately not re-issued across a lost connection — its first act is to type
into a pane and a dead socket cannot say whether it typed
(`crates/amx/src/cmd/call.rs`'s `reissuable`) — and the message says exactly
that. The *other* half of DR-16, the server's retriable code
(`RETRIABLE = -32001`, `crates/amx-proto/src/rpc.rs:187`), was not caught by
hand: it needs the call to reach a quiesced server rather than a dead socket,
and that window is the same milliseconds-wide one M3 could not aim at either.
CI owns it; the smoke records that it did not see it.

**A session can be handed over once.** Run from a clean start, twice in a row:

```
handoff 1: accepted true   → ping answers, same session id, no losses
handoff 2: accepted false  → "this server was assembled without a handoff path"
           the session is still answering; `session report` names the refusal
```

`session/serve.rs:193` calls `set_handoff`; `session/import.rs` does not, so a
server that arrived by import has no export path and refuses the next handoff
(`crates/amx-server/src/actor/core/handoff.rs:106-112`). The refusal costs
nothing — it is a pre-flight, the panes are untouched, and `session report` says
outcome, stage, binary and reason — but it means `amx update apply` succeeds once
per server lineage and refuses until the session is restarted. M3's smoke did one
upgrade and could not have seen it. Nothing in M4's plan depends on a second
upgrade; recorded because "amx upgrades without dropping a pane" is a sentence a
reader will apply twice.

### 3.8 A first keyframe that can be dropped, and the load that shows it

Found chasing a blank `attach --pane` screen, and worth the chase for what it
turned out **not** to be. Under load, `amx attach --pane` shows an empty terminal
until the pane next paints:

```
                                    unloaded   under 12 busy loops
amx at 335cc27 (wave 2)             6/6        2/8 painted on attach
amx at 6f4fb5d (wave 1)             —          1/8 painted on attach
```

Both binaries clean-built from their own trees, same driver, same session shape.
**It is not a wave-2 regression**: `bind()` in `crates/amx/src/cmd/viewport.rs`
is byte-identical at `6f4fb5d` and `335cc27`, and both fail the same way.

The mechanism is in the client, and it is one line of policy. A `--pane` attach
binds two streams through `Session::call`, whose `on_frame` is `|_, _| {}`
(`crates/amx-client/src/net/mod.rs:296-301,328-330`) — so a stream frame that
arrives *while a control reply is outstanding* is read and discarded. The grid
stream is bound first and the raw stream second, so the pane's first keyframe has
a whole round trip in which to land and be thrown away. Nothing recovers it: the
next paint is the next thing the pane does, which for a quiet agent is never.
`--takeover` always paints because its viewport declaration resizes the pane and
buys a fresh keyframe.

**The full client does not have this bug**, and the contrast is the fix written
out: `App::call` uses `call_with` and applies the frame, folding the result into
the client's `Effect` (`crates/amx-client/src/app/wired.rs:349-355`) — which is
X09's type doing exactly the job DR-10 argues for. So X15's peek, which binds a
grid stream for a non-focused pane on the full client, is not exposed to this.
`crates/amx/src/cmd/viewport.rs` is the only caller that binds the bare way, and
no wave-3 task owns it.

Recorded here rather than fixed, and one process note with it: the first four
measurements of this were taken while two `cargo build`s were running on the same
box, and they said the opposite thing — that wave 2 had broken a path wave 1
rendered fine. The repeat-count protocol above is what turned a plausible
regression into a pre-existing race. This machine's own recorded lesson about
load-sensitive reads applies to the smoke as much as to the suites.

### 3.9 DR-11's watch: five more clean stops

| Run | Stops | Exit | `drain-census` | Census log line |
|---|---|---|---|---|
| §3.2's baseline re-run | 2 | **0**, **0** | absent | none |
| §3.3–3.4's delta session (restart in the middle) | 2 | **0**, **0** | absent | none |
| §3.7's double-handoff session | 1 | **0** | absent | none |

Eleven clean stops across the milestone, over sessions of 2, 3, 4, 25 and 26
panes, plus three handoffs whose exporters all exited 0. Nothing has fired.

### 3.10 The two ledgers at this boundary

**The seam ledger stays open with one row, correctly.** `SEAM_LEDGER` names
`amx-server/src/actor/core/route.rs`, owed by X10 — wave 3, running now. The
`agent.list` row still answers by name and refuses through the seam helper, as
§2.4 recorded; nothing wave 2 landed touched it.

**The field ledger stays at six rows, and three of them changed state without
being closable.** A row is deleted when its *reader* lands, not when the field
does, so:

| Field | Writer | Reader | This boundary |
|---|---|---|---|
| `AgentSnapshot.reason` | X06 ✔ | X10, X11, X14, X16 | written and on the wire (§3.3); no reader yet |
| `AgentSnapshot.since` | X06 ✔ | X10, X11, X14, X16 | written and on the wire (§3.3); no reader yet |
| attention `workspace` | X06 ✔ | X16, `examples/notify.sh` | folded and on the wire (§3.4); no reader yet |
| `PaneState.mouse` | X13 | X13 | wave 3, gated (below) |
| `agent.list` `now` | X10 | X14, X16 | wave 3 |
| `NextParams.workspace` | X17 | X17 | wave 3 |

Three fields went from frozen to *written and observed over a socket* at this
boundary, which is the half of D-M4-10 the smoke can prove. No row is struck.

**X13's gate is still shut.** `m4-mouse-path.md` §7.3 has no dated heading:
this machine's graphical session is still locked (`LockedHint=yes`), so the
by-hand wheel observation X01 left as the one unobserved hop has not happened,
and X01's own outcome entry says X13 does not merge before it exists. X13 is
building against the outcome-(c) fallback. The gate is X00's to hold and it is
recorded open here so wave 3's boundary has to answer it.

## 4. The wave-3 delta — 2026-08-09

**Subject.** amx at `b698c51`, and that commit is wave **4**. Waves 3 and 4 both
merged before either was smoked, so this section and §5 are one run of one tree,
split by what they exercise rather than by what was in the binary. Saying so is
the point: §6's rule is a delta per boundary, and this milestone owes two of them
at once. Nothing below distinguishes a wave-3 regression from a wave-4 one, and
where that matters it is named.

Same machine, same isolation, same driver, with two repairs the run itself
forced (both in "Re-running this"): the scratch `HOME` now carries a `.zshrc`
and `SHELL=/bin/sh`, because a restored pane is a shell that types its agent's
argv (D-M2-7) and zsh's newuser wizard was eating it — for two runs that looked
exactly like a restore that had stopped bringing agents back.

`cargo test --workspace` on this tree: **141 suites, 966 passed, 0 failed**;
`cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` clean; the
module budget 34 over soft, 0 over hard.

### 4.1 Verdict

| # | What | Result |
|---|---|---|
| 1–6 | §1's baseline, re-run whole | **holds** — every item (§4.2) |
| 16 | X10: `agent.list` answers with every field, in queue order, `last_line` matching `pane.read` | **holds** — 25/25 identical, and it costs 10–13 ms where the shape it replaces costs 230 (§4.3) |
| 17 | X11: a per-workspace breakdown whose counts sum to the global one | **holds** — and the compact form at 45 columns (§4.4) |
| 18 | X11: the queue head with an age that advances | **holds only while something is painting** — frozen at `0s` for 14 s on a quiet session (§4.4) |
| 19 | X12: a 45-column client renders one pane at 45 columns | **holds** — 45×19, not a 28-column grid letterboxed, and crossing back mutates no layout (§4.5) |
| 20 | X13: a pane that asked for reports gets them, one that did not gets none | **holds** — byte for byte, in the pane's own frame (§4.6) |
| 21 | X13: wheel-up scrolls copy mode, wheel-down at the live edge leaves it | **holds** — and no positional value is parsed on that path (§4.6) |
| 22 | X17: `next-attention --workspace` clears one project's queue without leaving it | **holds** — three calls, `waiting` 2 → 1 → 0, focus never leaves (§4.7) |
| — | the attention queue itself | **a finding, and not a small one** (§4.8) |

### 4.2 The baseline, re-run

```
                          §1 (0721ac2)   §2 (6f4fb5d)   §3 (335cc27)   §4 (b698c51)
session.state x7          7 8 8 8 8 9 10 7 8 9 11 12 13 9 9 11 11 11 13 12 13 13 13 14 14
agent.list x7             —              —              —              10 11 11 12 12 12 13
state + 25 pane.read      161 ms         214 ms         164 ms         230 ms
25 agents ready in        0.5 s          0.5 s          0.5 s          0.4 s
short numbers over a restart  all kept   all kept       all kept       25/25 kept
```

Twenty-five agents across five workspaces, five blocked in the same distribution
(`docs` with none, `infra` with two), the queue in block order, both ptys, the
event stream **1061 deliveries, 0 gaps, 0 non-NDJSON lines and 0 sequence
discontinuities**, and the restart restoring 25 panes with every short number and
every workspace number unchanged, the stand-ins recording 25 resumes, and
`agent.list` back at 25 rows by the first read after the restore. Both servers
exited 0 with no drain census.

### 4.3 X10 — the reply, and what it costs

Every row carries `pane`, `name`, `kind`, `status`, `since`, `last_line` and
`workspace`; `reason` is on the five that a hook named and absent on the twenty
that nothing did, which is the blank column D-M4-3 asks a renderer for. The
`last_line` of all 25 is byte-identical to the bottom non-empty row `pane.read`
returns for the same pane, taken pane by pane.

The cost is the news. One `agent.list` at 25 agents is **10–13 ms**; the
`session.state` + 25 × `pane.read` shape it replaces is **230 ms** in the same
run on the same session. R-M4-7's arithmetic — that a per-pane refresh at 4 Hz
would be 100 calls a second each walking 25 panes — is the trap this reply
exists to avoid, and both surfaces that render it refresh once per window for all
rows (§5.2, §5.3).

### 4.4 X11 — the breakdown, and the age that stands still

At 200×50, verbatim:

```
 [api ⚑1] [docs] [exp ⚑1] [infra] [web ⚑1] · api-5 · idle ⚑3 api/api-1 0s
```

The per-workspace counts sum to the global `⚑3`, `docs` renders with no count at
all, and the head named at the end is the pane `agent.next` walks to. At 45
columns the same line is the compact form:

```
 api ⚑3 api/api-1 4s
```

**The age advances only when something else repaints.** Seven samples two
seconds apart on an otherwise quiet session all read `0s`, and a keystroke that
changes nothing does not move it; a resize does (`0s` → `7s` → `9s` in the same
attach). X11's clock is not what is standing still — it is a monotonic estimate
that only moves forward, and it is right the instant the line is rebuilt. What is
missing is a reason to rebuild it: X14 put a 250 ms tick in `wired.rs` gated on
the agents view being open, and the status line has none. §7's exit item 2 asks
for "an age that advances", and on a session where nothing is happening — which
is exactly the session a user glances at — it does not. **Unowned**: `app/status`
was X11's in wave 3 and is nobody's in wave 5.

### 4.5 X12 — forty-five columns

```
api's five panes at 200×50   98x47  48x47  23x47  11x47  10x47
the same client at 45×20     98x47  48x47  23x47  11x47  45x19
back at 200×50               98x47  48x47  23x47  11x47  10x47
```

The focused pane is sized to the whole content area — **45 columns wide, not 28
letterboxed inside 45** — and the four the projection does not draw keep the size
they were last commanded, which is the rule X12's outcome states. The screen
shows one pane's dialog and the compact status line and nothing else. The
`workspaces` block of `session.state`, serialized and compared before the first
crossing and after the second, is **identical**: crossing the threshold in either
direction mutates no layout, which is the half of D-M4-7 that a phone would
otherwise cost every other client in the session.

### 4.6 X13 — the mouse path, end to end

The gate §7.3 asks for is closed (`m4-mouse-path.md` §7.3, dated heading of
2026-08-09): both installed emulators send wheel-up as button 64 and wheel-down
as 65 in SGR, so this is outcome (b) and the row below is the one §7 item 7
licenses in full.

**What the server says about a pane's own mode**, over a real socket:

```
  plain-shell  1db07c12…  mouse=null
  mouse-app    fe23828a…  mouse={"events": "normal", "format": "sgr"}  118x17
```

**What the client asks its host terminal for**, every DEC private mode it wrote
in order, over a whole attach:

```
  no [client] mouse       ?1049h ?25l ?25h
  [client] mouse = true   ?1049h ?25l ?1006h ?1000h … ?1000l ?1006l ?25h ?1049l
```

The default asks for nothing; the request is released *before* the alt screen it
was asked for on top of; `1007` appears nowhere in either direction.

**What the pane that asked actually receives.** A 120×40 client with the
mouse app focused in the lower pane, reports fed into the client's pty:

```
  press   inside             '\x1b[<0;30;25M'   -> '^[[<0;29;4M'
  release inside             '\x1b[<0;30;25m'   -> '^[[<0;29;4m'
  wheel-up inside            '\x1b[<64;30;25M'  -> '^[[<64;29;4M'
  wheel-down inside          '\x1b[<65;30;25M'  -> '^[[<65;29;4M'
  press   in the other pane  '\x1b[<0;30;10M'   -> ''
  press   on the status line '\x1b[<0;30;40M'   -> ''
```

Same grammar, same button, coordinates moved into that pane's frame and nothing
else; a report outside the focused pane's interior is dropped rather than
delivered somewhere. A sweep down every other row of the terminal produced bytes
for rows 23–37 and silence everywhere else, which is the interior and its border
measured rather than asserted.

**And over a pane that asked for nothing**, the wheel is the exception and only
the wheel:

```
  before        '… docs-5 · idle'
  wheel-up      '… docs-5 · idle COPY'
  wheel-down 1  '… docs-5 · idle COPY'
  wheel-down 2  '… docs-5 · idle'
```

Copy mode opens on the turn, leaves when the second turn puts it back at the
live edge, and a click while it is open does nothing at all.

### 4.7 X17 — one workspace's queue, cleared without leaving it

```
  next --workspace infra -> infra/infra-3  waiting=2  focused=infra  queue=[api-1, web-2, infra-3, infra-4, exp-1]
  next --workspace infra -> infra/infra-4  waiting=1  focused=infra  queue=[api-1, web-2, infra-4, exp-1]
  next --workspace infra -> none           waiting=0  focused=infra  queue=[api-1, web-2, exp-1]
```

`waiting` counts the scope it was asked about and not the queue; the focused
workspace never moves; the three panes waiting in other projects are still
waiting when infra has nothing left. §7's exit item 5, measured.

### 4.8 A blocked agent goes idle after thirty seconds, and the queue empties under nobody

This is the finding of the boundary, it is not wave 3's or wave 4's, and it is
worth every line below because three of the four surfaces those waves built are
answers to "who needs me" and this is the thing that makes them answer "nobody".

**Measured.** One agent, blocked on a real permission dialog, watched once a
second and touched by nothing:

```
  [  0.0s] status=blocked reason=PermissionRequest queued=True
  [ 30.4s] status=idle    reason=None              queued=False
  explain: cause=staleness state=idle matched=permission_dialog
  the dialog is still the bottom of the screen: '  (esc to cancel)'
```

Thirty seconds after its last paint the pane is `idle`, it has left the
attention queue, its `reason` is gone — and `agent explain` on the same pane, in
the same second, still reports that the shipped `permission_dialog` rule matches
the screen it is looking at.

**And it comes back the moment anything repaints it.** One `amx attach` at a
size the panes were not already at, which resizes them and so produces damage:

```
  panes showing a dialog: api-1 idle, web-2 idle, infra-3 idle, exp-1 idle
  queue before attach: 0
  after one attach:    api-1 blocked permission_dialog, web-2 blocked …, exp-1 blocked …
  queue after attach:  3
```

Three of the four flipped straight back to `blocked`; the fourth is the one
whose size did not change, which is the mechanism stated by the exception.

**The mechanism, named.** `Deadline::Staleness` fires 30 s
(`fusion::STALENESS`) after the last thing that agreed with a held state, and
its arm transitions to `Idle` unconditionally
(`crates/amx-server/src/agent/fusion/tracker/inputs.rs:196-205`) — it does not
ask tier 2 on the way out. Tier 2 is evaluated on *damage*, and a permission
dialog waiting for a human produces none. So the module's own words for the
deadline — "a pane the screen keeps confirming as `Working` is never cleared for
staleness" (`fusion/mod.rs:124-126`) — cannot hold for a screen that has stopped
changing: "no new evidence" is being read as "no evidence". Every transition into
a held state arms it, so a *screen*-derived block decays the same way a
hook-asserted one does.

**Why no earlier smoke saw it.** M2's live smoke recorded that "the staleness
deadline was never reached on any transition in this run" — a real agent's screen
kept moving. M4 is the first milestone whose surfaces are read minutes after the
block, by a person who is deliberately *not* attached (D14's phone, `amx agents`
with no client). Every measurement in §1–§3 of this file, and every one in this
section, was taken inside the window because the run had just blocked the agents;
it is only the runs that left the session alone that found it.

**What it costs, concretely**: `amx agents` on a phone reports "no agents
blocked" about a session where four are; `⚑N` counts down to nothing; the agents
view's blocked band empties; `next-attention` finds nothing to jump to; and when
a repaint does revive the row, `since` is the repaint, so "blocked for 11m" reads
`0s`.

**Not taken.** `agent/fusion/**` and `actor/agent_hub/**` were X06's in wave 2
and are in no wave-5 task's list, and this predates M4. The cheap fix is one
that costs no I/O — on the staleness fire, evaluate the screen the tracker
already has and demote only if it does not corroborate — but it is a change to
the fusion state machine, which is a plan decision and not an integration
repair. Recorded in [m4-wave-outcomes.md](m4-wave-outcomes.md) under this
boundary with the same citations.

### 4.9 DR-11's watch

| Run | Stops | Exit | `drain-census` | Census log line |
|---|---|---|---|---|
| §4.2's restart | 1 | **0** | absent | none |
| §4.2's teardown | 1 | **0** | absent | none |
| the §5 session's teardown | 1 | **0** | absent | none |

Three more clean stops with an observed exit status, plus one live handoff whose
successor took the session (§5.2) and two stops of servers this driver had not
spawned, whose exit status is therefore not observable and which are not counted.
**Fourteen clean stops across the milestone**; nothing has fired.

## 5. The wave-4 delta — 2026-08-09

Same tree, same session, same run as §4. What is new here is that D15's three
surfaces are attachable for the first time, so the things §7 asks about them can
be driven rather than reasoned about.

### 5.1 Verdict

| # | What | Result |
|---|---|---|
| 23 | X16: `amx agents`, `--json`, `--workspace`, `--watch`, with no client attached | **holds**, all four (§5.2) |
| 24 | X16: `--watch` survives a server swap without ending its stream | **holds** — one frame of `reconnecting…`, then the same table with continuous ages (§5.2) |
| 25 | X14: the board opens, filters, groups, collapses, prompts, renames and kills on the second press | **holds**, every one (§5.3) |
| 26 | X14: it jumps | **fails** — it lands on the workspace's remembered pane, not on the selected agent (§5.5) |
| 27 | X15: peek shows a live pane from another workspace and forwards no keystroke | **holds** — and the cells arrive; what a peek of a tall pane *shows* is its middle, which is blank (§5.4) |
| 28 | detaching from the board | **fails** — `prefix+d` does not reach a client with the board open (§5.5) |

### 5.2 X16 — `amx agents`, in four forms

With no client attached anywhere in the session:

```
25 agents · 3 blocked
AGENT          STATUS   REASON             AGE  LAST LINE
api/api-1      blocked  PermissionRequest   1s  (esc to cancel)
web/web-2      blocked  PermissionRequest   1s  (esc to cancel)
exp/exp-1      blocked  PermissionRequest   1s  (esc to cancel)
api/api-2      idle                        11m  )
…
```

`--workspace api` scopes both the table and its count — `5 agents · 1 blocked`,
which is the queue counted *on the table* rather than the global queue described
in a two-row sentence. `--json` prints the reply verbatim, `now` and all.
`--watch --json` is refused with the spelling that works, which is the
documentation D15 already wrote:

```
--json prints one reply; --watch is the live screen. For a live machine-readable
stream use `amx events --json` and re-query on a `gap`, which is the contract
--watch packages for a person.
```

**Across a live handoff**, `--watch` on a pty while the session is handed to a
staged binary:

```
  footer before: q quits · seq 4808
  footer after:  q quits · seq 4862
  api/api-1  blocked  PermissionRequest  1s → 2s   (the same rows, ages continuous)
```

No `the session restarted` note, because it is the same session; the ages keep
counting because `since` is absolute and rides the manifest. `q` exits 0. §7's
exit item 4, both clauses.

### 5.3 X14 — the board

`prefix+g`, on a 120×40 attach, verbatim:

```
agents — 25 · ⚑3 · by workspace
api/api-1        ⚑ blocked PermissionRequest 3s   │   (esc to cancel)
  4 idle
web/web-2        ⚑ blocked PermissionRequest 3s   │   (esc to cancel)
  4 idle
exp/exp-1        ⚑ blocked PermissionRequest 3s   │   (esc to cancel)
  4 idle
  5 idle
  5 idle
 [api ⚑1] [docs] [exp ⚑1] [infra] [web ⚑1] · api-1 · blocked (PermissionRequest) ⚑3 api/api-1 3s
```

Blocked rows sort above idle ones and the projects holding them sort first; the
idle runs are collapsed; the board's age and the status line's age for the same
head agree, which is the disagreement seam 5 exists to catch, measured. Typing
`web` narrows to one project; `ctrl+b` narrows to the three blocked;
`ctrl+s` regroups to `by state` and collapses `22 idle` into one line.

`ctrl+r` opens the header as a rename line **prefilled with the current name** —
`rename web/web-1> web-1` — which is worth knowing before you type: ten
backspaces then `webby` leaves the pane labelled `webby` on the server, and
typing without erasing appends. `ctrl+p` opens `prompt …>` empty and a submitted
`stop` reached the agent without attaching: it went from `blocked` to `idle` with
the prompt box on its screen. `ctrl+x` arms — `kill docs/docs-5? ctrl+x again` —
and the pane count goes 25 → 24 only on the second press.

One thing a later surface should know: **the filter survives closing and
reopening the board.** Reopening with `prefix+g` and typing again appends to the
query that was there, which reads as "the board is showing nothing" until you
clear it.

### 5.4 X15 — the peek, and why it looks empty

`Space` on a row whose pane lives in a workspace this terminal is not drawing
opens the region, and the region is a bordered box with nothing in it. It is not
empty, and the proof is the pane's own geometry: `render::grid::blit`
centre-crops content taller than its slot, the stand-in agents write their
screens at the *bottom* after scrolling everything else off, and a 28-row peek of
a 47-row pane shows rows 10–37 — all of them genuinely blank on that pane.

The decisive measurement, because "the crop" and "no cells arrive" look
identical from outside: shrink the peeked pane to **one column**, so its content
is one character per row and reaches the top of its own grid, and peek it again:

```
┌───────────────────────────────────────────────┐
│                       e                       │
│                       d                       │
│                       ?                       │
│                       ❯                       │
│                       1                       │
…
```

The cells arrive, and they are centred in both axes. So the finding X14 raised
and routed to **X18** is exactly what it said it was — a crop rule, not a
delivery failure — and this is a second instrument agreeing with it.

**The peek forwards no keystroke**: with a peek open on a blocked agent, typing
`zzz` left its status, its `since` and the bottom line of its screen unchanged.
Typing goes to the filter, which is the surface's own rule.

### 5.5 Two defects at the join

**The board's `Enter` jumps to the workspace, not to the agent.** Selected
`exp/exp-3`, pressed `Enter`: the focused workspace became `exp` and the focused
pane was `exp-5`, the pane that workspace happened to remember. Repeated with
`exp-2` selected while `exp` was *already* the focused workspace: same answer,
`exp-5`. The mechanism is one line meeting another:
`agents_enter` sets the client's local focus for that workspace and then calls
`workspace.switch` (`app/agents/keys.rs:206-219`, whose comment says the pane
half stays local because no set-focus-to-pane op exists on the table); the server
answers by publishing `FocusChanged { workspace, pane }` naming the workspace's
*remembered* pane, and `app/events.rs:364-367` folds that over the local insert.
The local half is correct and is overwritten a few milliseconds later by the
call it makes itself.

It is a wave-4 defect in a shipped D15 surface — "jump to the agent that needs
you" is what the board is for — and it is nobody's in wave 5. Two cheap repairs
exist entirely inside `amx-client`: apply the local focus *after* the switch
reply (`SwitchReply.focused_pane` is already returned), or ignore a
`FocusChanged` for a workspace this client is itself jumping inside. The durable
one is a pane argument on `workspace.switch`, which is a wire change and belongs
in a plan rather than in an integration fix.

**`prefix+d` does not reach a client with the board open.** Measured: with the
board up, `ctrl+a d` does nothing and the client stays; `Esc` then `ctrl+a d`
detaches with exit 0. Small, and unpleasant exactly where D14 says this surface
matters — a phone whose user opened the board and wants out. Same ownership as
above.

### 5.6 The two ledgers at this boundary

**The seam ledger is closed in code and stayed closed**: `SEAM_LEDGER`'s one row
(`actor/core/route.rs`) went with X10 in wave 3, and
`no_dispatch_seam_outlives_the_milestone_that_opened_it` now asserts that no
`seam(…)` call site exists at all.

**The field ledger loses five of its six rows here, each struck against the
running binary rather than against a call site:**

| Field | Reader | Struck on |
|---|---|---|
| `AgentSnapshot.reason` | X10, X11, X14, X16 | a column on three surfaces (§4.3, §5.2, §5.3) |
| `AgentSnapshot.since` | X10, X11, X14, X16 | an age on three surfaces, agreeing across two (§4.4, §5.3) |
| `PaneState.mouse` | X13 | the gate on a relay watched delivering bytes (§4.6) |
| `agent.list` `now` | X14, X16 | every age on the board and the table (§5.2, §5.3) |
| `NextParams.workspace` | X17 | a scoped cycle driven to the end of a queue (§4.7) |

**The sixth stays open, and it is the row the ledger exists for.** The identity
block on `attention_enqueued`/`attention_dequeued` is written and on the wire —

```
{"event":"attention_enqueued","pane":"c336a545…","workspace":{"id":"f054855f…","name":"api"},
 "name":"api-1","reason":"PermissionRequest","since":1786251938774}
```

— and **neither reader the row named reads it**. `amx agents --watch` takes
`envelope.seq` off a delivery and re-queries `agent.list`
(`crates/amx/src/agents/watch.rs`'s `drain`), which is the right design for that
surface and reads none of the block; `examples/notify.sh` still prints
`pane <uuid> needs input`. D-M4-6 froze the block so that a notifier could render
`api/backend blocked (permission_dialog)` without a follow-up call, and nothing
in the tree does. That is the R-M4-14 failure mode — a field frozen ahead of a
reader that never came — caught by the instrument built to catch it, one wave
before the exit rather than one milestone after. `examples/` is **X20**'s in wave
5, and a two-line change to the reference notifier is the whole demonstration the
field was frozen for.

---

## Re-running this

The harness is the product plus a Python driver's worth of glue, and it is not
checked in — the same reason M2's and M3's were not: every part of it that is
not disposable is already a verb. What it does, in order:

1. **Isolate.** A scratch root on a disk-backed filesystem holding `HOME`,
   `XDG_RUNTIME_DIR`, `XDG_STATE_HOME` and `XDG_CONFIG_HOME`; every `AMX_*`,
   `CLAUDE*` and `XDG_*` variable removed from the driver's own environment
   before the child's is built.
2. **Install the stand-in agents.** Lift `SCRIPT` out of
   `tests/support/agent.rs` verbatim, write it executable, plant the registry
   stanza `plant_stanza` writes (`tests/support/agent.rs:184-215`) into
   `$XDG_CONFIG_HOME/amx/agents.toml`, and export `AMX_RIG_AMX` and
   `AMX_RIG_DIR`. Lifting rather than copying is the point: a change to the fake
   that breaks the suite breaks the smoke in the same run.
3. **Run `amx server` in the foreground as a child**, so the pid is owned and
   its exit status is a fact rather than an inference, with its stderr kept.
4. **Drive everything through the CLI.** The generated verbs print JSON, so a
   driver needs no wire code of its own.
5. **Allocate the ptys itself.** `openpty`, `TIOCSWINSZ` for the size, the slave
   as the client's stdin/stdout/stderr, and the master read back. The client's
   entire output vocabulary is CUP, SGR, `?25h/l` and `?1049h/l`
   (`crates/amx-client/src/render/mod.rs:54-116`,
   `crates/amx-client/src/term.rs:21-27`) and it positions every row explicitly,
   so cursor addressing plus printable text reconstructs the screen exactly.
   Detach with `ctrl+a d` (`crates/amx-client/src/input/mod.rs:325-328`) and
   read the exit code.
6. **Poll pane sizes until two consecutive reads agree** before recording them;
   the resize is debounced and a single read races it.
7. **Keep the event subscriber's stdout and stderr apart.** Merged, the relay's
   own stderr lines look like corrupt NDJSON, and the run would report a defect
   that belongs to the harness.

Three more the wave-3/4 runs added, each because the harness produced a false
finding before it produced a true one:

8. **Give the scratch `HOME` a `.zshrc`, and set `SHELL=/bin/sh`.** A restored
   pane is a shell that types its agent's argv (D-M2-7). With no rc file, zsh's
   newuser wizard owns that shell's stdin and eats the type-in, so a restart
   restores 25 panes and 0 agents — which reads exactly like a restore that has
   stopped bringing agents back, and cost two runs before it was named. With the
   rc file: 25 panes, 25 shorts, 25 recorded resumes, `agent.list` back at 25.
9. **Never give a long-lived relay a pipe nothing is draining.** `amx events
   --json` writes a line per delivery and flushes; through a 64 KiB pipe read
   only at the end it blocks on `write` after about 560 lines and looks precisely
   like a relay that stopped at a server swap. Redirect it to a file.
10. **A pane whose input you want to read must set `stty raw -echo` first, and
    its reader must be unbuffered.** In canonical mode the line discipline holds
    a mouse report — no newline in it — until something submits a line, and
    `cat` block-buffers into a file. Both make a working relay look dead: the
    first cost a wrong "the relay delivers nothing" reading, and the second cost
    the run that checked it.

**Read the queue within thirty seconds of a paint, or with a client attached.**
Every measurement of `attention`, `⚑N` or a blocked count in this file is inside
that window; §4.8 is what is outside it.
