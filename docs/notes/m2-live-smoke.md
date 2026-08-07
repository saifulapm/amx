# The M2 live smoke: amx driving real Claude Code

[hook-coverage.md](hook-coverage.md) §11 states M2's other exit criterion in one
sentence — "M2 does not exit on green tests. It exits on green tests **plus this
checklist, run by hand against real Claude Code**" — and why: the rig's five
scripted agents reproduce the payloads the spike recorded, but they are shell
scripts, and twice now a green suite has hidden a feature that did not work.

This is that run.

**Subject.** amx at `f8f41a3` (branch `worktree-m2-verify`, debug build) driving
**Claude Code 2.1.224** on Arch Linux, kernel 7.1.5, x86_64, on 2026-08-07.
Three real, separately authenticated Claude Code sessions in three amx panes,
one per workspace.

**Isolation.** The session under test must not be able to disturb the machine
owner's own work, and the agent under test must be a real, authenticated,
*top-level* Claude Code. Both, at once:

- amx's `XDG_RUNTIME_DIR`, `XDG_STATE_HOME` and `XDG_CONFIG_HOME` point at a
  scratch tree, and the session is named `live`.
- `CLAUDE_CONFIG_DIR` points at a scratch config directory whose
  `.credentials.json` starts as a **symlink** to the real one — the same
  borrow-by-symlink the spike's Codex harness used. So `amx integration install
  claude` writes its hooks into *that* `settings.json`, and the developer's
  `~/.claude/settings.json` is never opened. Auth works: `claude -p` answered
  from the scratch directory before anything else was tried. (Mid-run, an agent
  refreshed the token and *replaced* the symlink with a file of its own, so the
  borrow lasts until the first refresh and the real file is never written
  through. Worth knowing; it changes nothing here.)
- The harness is itself driven from a Claude Code session, and its markers
  propagate: a pane spawned with `CLAUDE_CODE_CHILD_SESSION` inherited turns the
  agent under test into a *child* session with transcript saving off, which the
  agent says out loud in its own footer. Every `CLAUDE_CODE_*`, `CLAUDECODE`,
  `CLAUDE_PID` and `CLAUDE_EFFORT` is unset before the amx server starts, so the
  panes' agents are top-level. **This is a live-harness trap worth naming: the
  first attempt ran with them set and the agent was quietly degraded.**

**Method.** Everything the driver does goes through amx's own control surface —
`agent.start`, `agent.prompt`, `pane.run`, `pane.send_keys`, `pane.read`,
`session.state`, `agent.explain`, `wait`, `events` — over the real socket, so
what is exercised is the product. A real client (`amx attach`) runs in a
detached tmux session at 200×50, and its status line is read with `tmux
capture-pane`; that is where the `⚑N` claims come from. Four recordings share
one millisecond clock and are merged for the transcripts below:

- `amx events --json`, the session's own event stream;
- `scripts/spike/hook-log.sh`, installed as a **sibling** hook entry beside
  amx's own on all eleven subscribed events — ground truth for what Claude Code
  emitted, independent of what amx made of it. Without it, "amx dropped an edge"
  and "no edge was ever sent" are the same observation;
- `examples/notify.sh`, the shipped reference notifier, with a `notify-send`
  stand-in that prints;
- the driver's own marks, stamped either side of every keystroke.

---

## 1. Verdict

The measured design works end to end against the real agent. All five points
hold, with the numbers below; two defects were found, one fixed here.

| # | What was to be proven | Result |
|---|---|---|
| 1 | a pane running real `claude` is identified as the `claude` agent | **holds** — tier 3 names it 14 ms after spawn, tier 1 corroborates with a session ref 1.4 s later |
| 2 | a slow prompt drives `working`, on the hook edge | **holds** — 28 ms keystroke→hook, 33 ms keystroke→`agent_status` |
| 3 | a permission request drives `blocked`, queues, `agent next` focuses, `⚑N` counts | **holds** — 13 ms hook→`blocked`+`attention_enqueued`, `⚑1`/`⚑2` on a real terminal, and the walk visits the blocked set in block order |
| 4 | the silent transitions settle from tier 2 or the staleness deadline | **holds, and it is tier 2** — 116/142/117/61 ms, with the hook log empty across every one |
| 5 | `agent explain` names the evidence | **holds** — manifest, version, winning rule, its evidence line, and every losing rule |

Two defects:

- **Fixed.** An attached client never showed a pane created over another
  connection: `apply_event` dropped `pane_created` and `layout_changed`. Three
  `agent start` calls, three panes in the session, one pane on the screen. §7.1.
- **Diagnosed, not fixed.** `pane.run` loses its submit on about 3% of prompts
  that start a model turn, silently; the two-write path never lost one in 170
  paired trials. The fix contradicts a clause in 04 §8, so it is raised here
  rather than made. §7.2.

---

## 2. Point 1 — identity

```
$ amx integration status claude
claude  not installed  …/live/ccfg/settings.json
$ amx integration install claude
claude  installed  Claude Code · …/live/ccfg/settings.json
  note: hooks run in every folder you have already trusted; in a folder Claude
        Code has not seen before, no hook fires until you answer its trust prompt once.
$ amx integration status claude
claude  current  marker 1, 11 events, …/target/debug/amx
```

`status` names the binary it wrote, which is the whole point of the verb
(herdr's bug is a `current` on an installation whose binary is gone). It stayed
`current` after the file grew a `theme` key, a `permissions` block and a foreign
hook entry on every event, and `install` left all three untouched.

Then three agents:

```
$ amx agent start --params '{"name":"a1","kind":"claude","cwd":"…/proj","workspace":"…","timeout_ms":45000}'
{"agent":{"cause":"probe","kind":"claude",
          "session_ref":{"kind":"id","value":"f814998f-320d-4b04-8a84-58ef7876a344"},
          "state":"idle","transition_seq":211},
 "pane":"2507960d-…","readiness":"ready","seq":233,"short":5}
```

Three starts, 3.03 s each, three **distinct** `session_ref`s, and `pane.read` on
each showed Claude Code's prompt box — not a splash mid-paint.

Both identity tiers are visible in the recording, and they arrive in the order
the design says they do:

```
+0.000  amx   agent.start (pane spawned)
+0.014  amx   {"event":"agent_identified","pane":"c8d1f00b-…","kind":"claude"}
+0.014  amx   {"event":"agent_status","to":"idle","cause":"probe"}
+1.377  HOOK  SessionStart  AMX_PANE_ID=c8d1f00b-…  AMX_HOOK_TOKEN=feae6d92…
```

Tier 3 answers first because it can: `claude` on this machine is a symlink to a
native binary, so the pane's foreground group leader has `argv[0] == "claude"`
and the registry's `executables` list matches it directly. Tier 1 lands 1.4 s
later and is what carries the resume ref; the `session_ref` in the `agent.start`
reply is hook-borne and could not have come from anywhere else.

**D-M2-4's identity scheme, measured on the real chain rather than the spike's
driver:** every hook invocation in the run carried `AMX_ENV`, `AMX_SESSION`,
`AMX_SOCKET`, `AMX_WORKSPACE_ID`, its own pane's `AMX_PANE_ID` and that pane's
own `AMX_HOOK_TOKEN`. Three panes, three different tokens, no crossover.

**Why the orchestrator's probe run saw `quiet` and blank rows.** Under an
isolated `HOME`, Claude Code has no credentials, no `hasCompletedOnboarding` and
no trusted folder, so the pane holds a first-run dialog — theme picker, then
login-method picker — and never becomes a session. That is environment, not amx:
with the config directory isolated but the credentials borrowed, the same build
identified the agent on the first try. Both dialogs were reproduced here and are
in the transcripts. The blank `pane read` rows have the same root: nothing had
painted yet at the moment it was read. The read path itself works — every screen
quoted in this document came out of `pane.read`.

Worth recording as a real observation of the product, though: **`agent start`
returned `readiness: "ready"` while the pane held a first-run modal dialog.**
Readiness is "the agent owns the pane's foreground and has been observed idle",
and a modal dialog waiting on a human satisfies both. It is not the failure
§11's step 2 names (a `ready` during the splash) but it is adjacent to it, and a
caller that then sends a prompt types into a dialog.

## 3. Point 2 — a prompt drives `working`

```
+0.267  mark  prompt a1 'Write a 400 word essay about the history of the bicycle…'
+0.295  HOOK  UserPromptSubmit
+0.300  amx   {"event":"agent_status","from":"idle","to":"working","cause":"hook"}
```

28 ms from the submit to the hook process's first statement — the spike measured
a 26 ms median for exactly this and it reproduces — and 5 ms more for amx to
turn it into a published transition. The whole chain, keystroke to status, is
**33 ms**. Across the run's controlled repeats the driver observed `working`
within 0.21–0.24 s of its own call, which is its 0.2 s poll interval plus the
33 ms.

## 4. Point 3 — `blocked`, the queue, `agent next`, `⚑N`

```
+0.923  mark  prompt a1 'Run exactly this bash command and nothing else: echo amx-live-probe'
+0.984  HOOK  UserPromptSubmit
+0.992  amx   {"event":"agent_status","from":"idle","to":"working","cause":"hook"}
+3.643  HOOK  PreToolUse tool_name=Bash
+3.677  HOOK  PermissionRequest tool_name=Bash
+3.690  amx   {"event":"agent_status","from":"working","to":"blocked","cause":"hook"}
+3.690  amx   {"event":"attention_enqueued","pane":"2507960d-…"}
+3.896  mark  screen: " Do you want to proceed?  ❯ 1. Yes / 2. No / Esc to cancel"
+3.905  mark  attention queue = ['2507960d-…']
+3.910  mark  client status line: ' amx ⚑1'
+9.710  HOOK  Notification notification_type=permission_prompt
```

13 ms from the `PermissionRequest` hook to `blocked` and the enqueue, both
published in the same instant, and the dialog is on screen 200 ms later. V01's
claim that the `blocked` **entry** may be trusted outright survives contact with
the real thing.

`⚑1` is a real client's real status line, read out of the terminal it painted
into. With a second pane blocked it read `⚑2`, and it counted back down —
`⚑2` → `⚑1` → ` amx` — as the two dialogs were answered.

The `Notification{permission_prompt}` at **+6.03 s** after the
`PermissionRequest` is V01 §3 M5's second witness, reproduced to the tenth of a
second.

**The walk (§11 step 8).** Two panes blocked in a known order — a2 first, then
a1, which is *not* creation order — then `agent.next` repeatedly, answering each
before asking for the next:

```
+1.930  mark  a2 blocked after 1.925s
+7.244  mark  a1 blocked after 5.304s
+7.251  mark  queue = ['a2','a1']
+7.255  amx   agent.next -> a2  waiting=2  workspace=9e85b43a-…   (a2's workspace)
+7.255  mark  keys a2 escape
+7.790  amx   agent.next -> a1  waiting=1  workspace=36e068d7-…   (a1's workspace)
+7.790  mark  keys a1 escape
+8.346  amx   agent.next -> empty queue, waiting=0
        walk visited ['a2','a1']; blocked order was ['a2','a1'] -> MATCHES
```

Block order, not creation order, across two workspaces, and an honest empty at
the end rather than an error. Note that `agent.next` focuses the *head* by
design (D-M2-8) — pressing the chord twice without answering lands on the same
pane both times, which the run also observed and which is the specified
behaviour, not the failure step 8 names.

**`wait --until blocked` from a second terminal (§11 step 11).** A `wait` was
standing on a2 before a2's prompt was sent:

```
+3.595  mark  prompt a2 …
+5.954  mark  a2 blocked after 2.350s cause=hook
+5.833  amx   wait returned: {"pane":"281e8b23-…","satisfied":true,
                              "agent":{"state":"blocked","cause":"hook","attention":1}}
```

It returned on the transition, 3.24 s after it was issued and inside the same
tenth of a second the dialog painted in — not on a poll, not late. Its reply
carries the pane's queue position too.

## 5. Point 4 — the silent transitions, which is the whole reason for fusion

Four exits, all four measured silent by V01, all four confirmed silent here —
the hook log holds **nothing at all** between the entry edge and amx's exit
transition — and all four settled from tier 2, in a tenth of a second.

| Transition | Hook events after the Esc/answer | amx left the state after | `cause` |
|---|---|---|---|
| Esc during generation (a1) | — | **116 ms** | `screen` |
| Esc during a running tool call (a2) | — | **142 ms** | `screen` |
| Permission dialog cancelled with Esc (a3) | — | **117 ms** | `screen` |
| Permission dialog answered "No" (a1) | — | **61 ms** | `screen` |

Esc during generation:

```
+0.267  mark  prompt a1 …
+0.295  HOOK  UserPromptSubmit
+0.300  amx   agent_status idle→working cause=hook
+5.496  mark  screen: "⏸ manual mode on · esc to interrupt · ← for agents"
+5.497  mark  keys a1 escape
+5.613  amx   agent_status working→idle cause=screen
+5.729  mark  screen: "⏸ manual mode on · ? for shortcuts · ← for agents"
```

Esc during a running tool call — `PreToolUse` fires, the interrupt lands, and no
`PostToolUse`, no `PostToolUseFailure`, no `Stop` ever arrives:

```
+0.756  mark  prompt a2 'Run exactly this bash command…: until [ -f /tmp/… ]; do sleep 2; done'
+0.797  HOOK  UserPromptSubmit
+0.802  amx   agent_status idle→working cause=hook
+3.355  HOOK  PreToolUse tool_name=Bash
+8.995  mark  keys a2 escape
+9.137  amx   agent_status working→idle cause=screen
```

Leaving `blocked`, both ways, with the queue draining behind them:

```
+46.502  mark  keys a3 escape            (the dialog)
+46.619  amx   agent_status blocked→idle cause=screen
+46.619  amx   attention_dequeued pane=68ff7584-…
+46.750  mark  queue = ['2507960d-…'], client ' amx ⚑1'
+48.158  mark  keys a1 enter             (on "No")
+48.219  amx   agent_status blocked→idle cause=screen
+48.219  amx   attention_dequeued pane=2507960d-…
+48.413  mark  queue = [], client ' amx'
```

116 ms is faster than three confirmations at 100 ms spacing would allow, and
that is not an accident: `prompt_box_idle` carries `visible_idle = true`,
herdr's flicker fix kept as data, which bypasses the confirmation hold when the
prompt box is actually on screen. **The staleness deadline was never reached on
any transition in this run** — tier 2 got there first every single time, which
is the outcome 04 §5 designs for and the one the exit suite cannot prove.

**The late `SubagentStop` did not revive an idle pane** (§11 step 9, and herdr's
scar):

```
+0.377  HOOK  UserPromptSubmit
+0.383  amx   agent_status idle→working cause=hook
+2.469  HOOK  PreToolUse  tool_name=Read
+2.518  HOOK  PostToolUse tool_name=Read
+5.708  HOOK  Stop
+5.833  amx   agent_status working→idle cause=screen
+7.567  HOOK  SubagentStop agent_id=a59f859c3223fd917      ← 1.86 s after Stop
…       pane stayed idle for 8.2 s
```

**One line of that transcript is worth reading twice:** the turn ended on
`cause: screen`, 125 ms *after* the `Stop` hook had already fired. That is not a
missing edge — `fusion/edge.rs` maps `Stop` to `Ignore` for an `edges` agent on
purpose, because 04 §5 says exits are confirmed rather than trusted and V01
found clause (a) unreachable. It is worth naming anyway: **every turn end in
amx, the most common transition in the system, is tier 2's**, and hooks
contribute nothing to it. The cost today is ~125 ms; the cost if a weekly
release moves the prompt box is 30 s of staleness on every turn instead. This is
a recorded consequence of a decision in 04/08, not a defect, and re-opening it
means re-opening D-M2-6.

## 6. Point 5 — `agent explain` names its evidence

```
$ amx agent explain --params '{"target":"a1"}'
{"pane":"2507960d-…","kind":"claude",
 "manifest":"bundled:claude.toml","manifest_version":"2026.08.07",
 "matched":"prompt_box_idle",
 "agent":{"state":"idle","cause":"screen","kind":"claude",
          "session_ref":{"kind":"id","value":"f814998f-…"},"transition_seq":2629},
 "region_preview":["✻ Crunched for 1m 49s","", …,
                   "─────────────…","❯","─────────────…",
                   "  ⏸ manual mode on · ? for shortcuts · ← for agents"],
 "rules":[
  {"rule":"title_spinner_working","priority":1100,"region":"title",
   "asserts":"working","matched":false,
   "evidence":"region does not match /^[\\x{2800}-\\x{28FF}]/"},
  {"rule":"permission_dialog","priority":900,"region":"bottom_non_empty_lines(8)",
   "asserts":"blocked","matched":false,
   "evidence":"no \"do you want to proceed?\" in the region"},
  {"rule":"footer_interrupt_hint_working","priority":800,"region":"bottom_lines(6)",
   "asserts":"working","matched":false,
   "evidence":"no \"esc to interrupt\" in the region"},
  {"rule":"spinner_line_working","priority":780,"region":"bottom_non_empty_lines(8)",
   "asserts":"working","matched":false,
   "evidence":"no line matches /^\\s*\\S+ \\S+… \\(\\d+s/"},
  {"rule":"prompt_box_idle","priority":200,"region":"bottom_non_empty_lines(5)",
   "asserts":"idle","matched":true,"evidence":"\"❯\" matches /^❯(\\s|$)/"}]}
```

The winning rule is the one the screen shows, every losing rule reports why it
lost against the same region, and the region the rules read is printed back
verbatim. **The manifest has not rotted against 2.1.224**: every rule's evidence
line above is a true statement about the screen beside it, and every screen
quoted in §5 was classified correctly.

## 7. Restart, and the notifier

**Step 12 — restart, and each agent resumes its own conversation.** Each of the
three was told a word only its own conversation heard (`ALPHA`, `BRAVO`,
`CHARLIE`), then `amx session stop`, then the server again:

```
$ amx session stop
stopped live (pid 1145029)
$ amx server --session live &
… 12 s later:
  a1   idle  cause=probe  ref=f814998f
  a2   idle  cause=probe  ref=f52db343
  a3   idle  cause=probe  ref=da9ee8e4
```

Three panes came back as agents, not as bare shells, each with **its own**
session ref, each at Claude Code's prompt with its own scrollback visible.
Asked "what word did I ask you to remember?", a1 answered `ALPHA`. a2 and a3
were asked too, and their prompts hit §7.2 — the text was typed and never
submitted — so the two conversations are unproven by this run's own answer, but
the refs, the labels, the layout and the per-pane transcripts all came back
distinct, and a1's answer is the case the step exists to catch (two panes
resuming one conversation would have made a1 answer with somebody else's word).

**Step 13 — `examples/notify.sh`.** Running against `amx events --json`
throughout, with a printing `notify-send` stand-in:

```
subscribed at seq 111
notify-send amx pane 68ff7584-8824-4fe7-8ba7-1569d8f7c368 needs input
```

One block, one notification, naming the pane that blocked — and the client's own
status line went `' amx · a1 · idle'` → `' amx · a1 · idle ⚑1'` → back, in the
same seconds. The external consumer and the status line are reading the same
queue, which is the roadmap's requirement.

---

## 8. What was found

### 8.1 Fixed — a pane another connection created never reached the client

Three `agent start` calls put three panes in the session. The attached client
went on painting one, in a frame the whole width of the terminal, with the
shell's prompt drawn at the wrong column because the pane behind it had been
resized to half of it. `⚑N` was correct throughout, which is what makes the
shape of the bug legible: agent events were folded, structural ones were not.

`App::apply_event` folds `agent_status`, `agent_identified`, the attention pair
and `focus_changed`, and drops everything else on its catch-all arm —
`pane_created` and `layout_changed` with it. The only other path into the mirror
is `mutates_layout`, which resyncs after **this client's own** layout calls. So a
pane minted over another connection was invisible, permanently.

`wired.rs` already asserted the opposite in a comment: "`agent.start` is the one
that mints a pane, and it does so through the same `pane.split` path, whose own
event the client already hears." The intent was written down; the arm was not.
Fixed by folding both events as `Resync` — the event says *that* a workspace
changed shape, never what it changed to, and a layout tree is not something a
mirror can reconstruct from a pane id. `crates/amx-client/src/app/events.rs`,
with the acceptance test in `crates/amx-client/tests/events.rs`
(`a_pane_another_connection_created_reaches_this_screen`: a second client
splits, and the first has to show both panes — it hangs on the deadline without
the fix).

The first cut of that fix broke something else, and the suite caught it, which
is worth recording because it is the shape of the whole design: `sync_state`
adopts the session's focused workspace, so re-reading state on *another*
connection's event yanked this terminal into that connection's workspace —
`tests/adversarial.rs`'s "its screen owes the flood pane nothing". The two
readings of a resync are now separate calls: `sync_state` adopts the focus (an
attach, or this client's own `workspace.switch`/`agent.next`), `resync_state`
keeps this terminal's presentation (a gap, or somebody else's structural
event). 04 §3's "the client gets its own presentation" is the rule; the
adversarial flood test is what pins it.

Live confirmation, client attached first and the split made afterwards from
another connection:

```
$ amx pane split --params '{"pane":"a1ecc86d-…","direction":"horizontal"}'
{"pane":"d643f594-…","short":8,"seq":3370}
```

and the client's screen, two seconds later, holds three panes.

### 8.2 Diagnosed, not fixed — `pane.run` loses about 3% of turn-starting submits

`agent.prompt` types its text into Claude Code's input box and submits it with a
trailing `CR`. Sometimes the `CR` does not take: the text sits in the box, no
hook fires, and `agent.prompt` returns `satisfied: true` — from amx's point of
view the agent is simply idle, which it is. Sending a bare `enter` afterwards
submits the text that was sitting there, so the paste arrived and only the
submit was lost.

It is specific, and the specificity is the finding. Paired A/B trials, same
pane, same text, alternating:

| Path | Trials | Lost |
|---|---|---|
| `pane.run` — `ESC[200~ text ESC[201~ CR`, **one** write | 170 | **6** |
| `send_text` then `send_keys enter` — **two** writes | 170 | 0 |

— two panes, 3 lost in 110 trials on one and 3 in 60 on the other — and the loss
only ever appears on a prompt that starts a **model turn**. 70 paired trials
with an unknown slash command — answered locally in milliseconds — lost nothing
on either path, which is why the first two rounds of probing proved nothing at
all. A quiet-pane hypothesis (both organic losses followed long idle stretches)
was tested at 5 s, 60 s, 180 s and 300 s and did not reproduce.

The mechanism this points at: `pane.run` writes the paste and its newline as one
`write()`, deliberately — 04 §8 calls it "bracketed-paste-aware **atomic**
text+submit", and `drive.rs` says "one atomic write, so nothing can land between
the text and its newline". A paste-aware TUI that treats a `CR` arriving in the
same read as the paste terminator as trailing whitespace *of the paste*, rather
than as a keypress after it, swallows exactly this, and would do it only when
the read boundary falls that way — which is the ~3%.

**Why this is raised and not fixed.** The change is one line: queue the `CR` as a
second chunk. The invariant 04 §8 actually needs survives it — the pane's input
queue is ordered and single-writer, so nothing can land between two chunks
queued back to back either, and "atomic" in the sense that matters is
queue-order atomicity rather than single-`write()` atomicity. But that is a
reading of a binding document, HACKING.md says not to contradict one without
raising it first, and a 3-in-110 live failure cannot be pinned by a test that
fails without the change — which is the other thing HACKING.md requires. So:
the evidence is here, the one-line change is named, and the decision is the
orchestrator's.

Until then the honest statement of the surface is that `agent.prompt` reports
that it *submitted*, not that the agent *received* — and `--wait` does not paper
over it, since a wait on an unsubmitted prompt correctly times out rather than
lying.

---

## 9. The §11 checklist

| # | Step | Result |
|---|---|---|
| 1 | `integration install` then `status` | **pass** — `current`, names the binary, survives foreign keys and a foreign hook entry |
| 2 | five real sessions, ready at the prompt | **partial** — three, not five (enough for the queue and the walk); all three at Claude Code's prompt. `ready` also returned on a first-run modal dialog (§2) |
| 3 | prompt, watch the status line | **pass** — 33 ms keystroke to `working` |
| 4 | Esc during generation | **pass** — 116 ms, `cause: screen` |
| 5 | Esc during a tool call | **pass** — 142 ms, `cause: screen` |
| 6 | permission request → `blocked`, `⚑1` | **pass** — 13 ms after the hook, `⚑1` on a real terminal |
| 7 | answer "No" | **pass** — 61 ms, `cause: screen`, `⚑` cleared |
| 8 | block several, walk `next-attention` | **pass** — two panes, block order not creation order, across two workspaces, honest empty at the end |
| 9 | tool turn, sit two seconds | **pass** — anonymous `SubagentStop` at +1.86 s, pane stayed idle 8.2 s |
| 10 | `agent explain` | **pass** — §6 |
| 11 | `wait --until blocked` from a second terminal | **pass** — returned on the transition, 3.24 s in, `satisfied: true` |
| 12 | `session stop`, restart, each agent resumes its own conversation | **partial** — three panes back as agents with three distinct refs and their own scrollback; one of three verified by asking for its word, the other two blocked by §8.2 |
| 13 | `examples/notify.sh` throughout | **pass** — one notification per block, naming the pane |

| Date | amx | Claude Code | Platform | Steps that failed | Notes |
|---|---|---|---|---|---|
| 2026-08-07 | `f8f41a3` | 2.1.224 | Arch Linux 7.1.5 x86_64 | none outright; 2, 12 partial | one client defect found and fixed (§8.1); `pane.run` submit loss diagnosed and raised (§8.2) |

---

## 10. Re-running this

The harness is the product plus a few scripts' worth of glue; it is not checked
in, because every part of it that is not disposable is already a verb. To repeat
it:

1. Point `XDG_RUNTIME_DIR`, `XDG_STATE_HOME`, `XDG_CONFIG_HOME` at a scratch
   tree and `CLAUDE_CONFIG_DIR` at a scratch config directory; symlink
   `.credentials.json` into it from the real one; unset every `CLAUDE_CODE_*`,
   `CLAUDECODE`, `CLAUDE_PID`, `CLAUDE_EFFORT`.
2. Seed that config directory's `.claude.json` with `hasCompletedOnboarding` and
   `projects.<cwd>.hasTrustDialogAccepted`, or answer the dialogs by hand the
   first time — V01 §3 M7 measured that no hook fires until the folder is
   trusted, and the theme and login pickers come before that.
3. `amx integration install claude`, then add `scripts/spike/hook-log.sh` as a
   **sibling** entry on each event (a handler merged into amx's own group makes
   `integration status` report `outdated`, correctly: reinstalling would move
   amx's handler into a slot of its own).
4. `amx server --session live`; `amx attach` inside a tmux session for the status
   line; `amx events --json` for the event transcript; `examples/notify.sh` for
   step 13.
5. Drive with `agent.start` / `agent.prompt` / `pane.send_keys`, and read the
   four recordings back on one clock.

What would settle §8.2, which this machine could not: a tee on the pane's input
queue, so the question "did the `CR` reach the child at all" can be answered
below the terminal instead of inferred from the screen. amx has no such seam
today, and adding one is a product change, not a harness one.
