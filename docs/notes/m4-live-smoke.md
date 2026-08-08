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
