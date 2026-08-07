# Hook coverage: what agent hook systems actually emit (V01 spike)

The M2 plan ([08-m2-plan.md](../08-m2-plan.md) §2) hands V01 ten questions the
agents' documentation does not answer, and one rule: measure, never infer. This
is the measurement.

**Subjects.** Claude Code **2.1.224** and Codex CLI **0.147.0**, both installed
on the dev machine (Arch Linux, x86_64, kernel 7.1.5), driven on 2026-08-07
through a real PTY by `scripts/spike/`. Codex was not installed when the plan
was written; `npm i -g @openai/codex` succeeded, and the machine's owner logged
it in, so Codex is **measured**, not deferred (this retires the fallback branch
of R-M2-1).

**Method.** A scratch project whose hook settings subscribe every lifecycle
event to one logging command (`scripts/spike/hook-log.sh`). The command stamps
itself with bash's `$EPOCHREALTIME` before doing anything else, then appends one
JSON line carrying the event tag, the full stdin payload, every `AMX_*`
environment variable it can see, and its own `/proc` ancestry. The PTY driver
appends `MARK` lines to the *same* log with the *same* clock, so a keystroke and
the hook it provoked are subtractable. `scripts/spike/analyze.py` reads the log
back against the scenario script.

**How to read a claim here.** Every row is one of:

- **measured** — reproduced by a scenario in `scripts/spike/`, with the
  recording under the run's `hooks.jsonl` and the terminal's own paint log
  under `dumps/`.
- **read from the shipped artifact** — taken out of the installed binary
  (Claude Code's bundle is readable JS; Codex's is a Rust binary with legible
  string tables). Corroboration, never a substitute.
- **unmeasured** — stated as such, with what would measure it.

A docs claim that was not reproduced is cited as a docs claim.

---

## 1. Verdict

| Agent | Proposed `coverage` | Why |
|---|---|---|
| `claude` | **`edges`** | Entry edges are complete, fast (median 26 ms) and precede the UI; **every exit-by-user is silent** — Esc during generation, Esc during a tool call, a dialog answered "No", a dialog cancelled with Esc all emit nothing at all |
| `codex` | **`edges`** | Same shape: `UserPromptSubmit`/`PreToolUse`/`PermissionRequest`/`Stop` all fire and carry session identity, but hooks are **inert until a human approves them interactively**, and the approval is per-config-hash |

Both stanzas need tier-2 screen detection as an equal partner, which is what
04 §5's fusion design already says. Nothing measured here promotes an agent to
`full`; nothing demotes one to `identity`.

**The four gates for V04** (M1, M2, M4, M6) are all **settled by measurement**.
V04 may merge.

---

## 2. The matrix — Claude Code 2.1.224

Every cell is what the hook log contains for that transition, in order. `—`
means *nothing fired*, which is a finding, not a gap in the recording: the
scenario ran, the screen confirmed the transition happened, and the log stayed
empty.

| Transition (what the user did) | Hook events, in order | Scenario |
|---|---|---|
| Agent starts in a trusted folder | `SessionStart{source: startup}` | `clean-turn` |
| Agent starts in a **never-trusted** folder | *nothing* until the folder-trust dialog is answered; then `SessionStart` 0.57 s later | `fresh-trust` |
| Prompt submitted | `UserPromptSubmit` | all |
| Turn ends normally | `Stop{stop_hook_active: false}` | all |
| Pre-approved tool call | `PreToolUse` → `PostToolUse` → `PostToolBatch` | `tool-turn` |
| Tool call fails on its own | `PreToolUse` → `PostToolUseFailure{is_interrupt: false}` → `PostToolBatch` | `tool-error` |
| Tool call refused by a `permissions.deny` rule | `PreToolUse` → `PostToolBatch` (**no** `PostToolUse`, **no** `PermissionDenied`) | `deny-rule` |
| Tool call needs permission | `PreToolUse` → `PermissionRequest` (8–14 ms **before** the dialog paints) | `permission-*` |
| …dialog answered **"No"** by a human | **—** | `permission-deny` |
| …dialog dismissed with **Esc** | **—** | `permission-esc` |
| …dialog left unanswered 6 s | `Notification{notification_type: permission_prompt}` | `blocked-notification` |
| **Esc during generation** | **—** | `esc-generation` |
| **Esc during a running tool call** | **—** (no `PostToolUseFailure`, no `Stop`) | `esc-tool` |
| Subagent turn (Task tool) | `PreToolUse{tool_name: Agent}` → `SubagentStart` → `SubagentStop` → `PostToolUse` → `PostToolBatch` → `Stop` | `subagent` |
| …plus, after **almost every tool-using turn** | a second, anonymous `SubagentStop` **1.9–3.0 s after the parent's `Stop`** | 6 occurrences |
| Idle at the prompt for 60 s | `Notification{notification_type: idle_prompt}` | `idle-notification` |
| `/clear` | `SessionEnd{reason: clear}` → `SessionStart{source: clear}` **with a new `session_id`** | `clear-command` |
| `/compact` (enough history) | `PreCompact{trigger: manual}` → [24 s] → `SubagentStop` → `SessionStart{source: compact}` → `PostCompact{trigger: manual}`, **same `session_id`** | `compact-command` |
| `/compact` (too little history) | `PreCompact` only — the compaction aborts with "Not enough messages to compact" and `PostCompact` never fires | first `compact-command` run |
| `claude --resume <id>` | `SessionStart{source: resume}` with the **same `session_id`** | `resume-session` |
| `/exit` | `SessionEnd{reason: prompt_input_exit}` | all |

Per-scenario event sequences, generated from the log:

```
clean-turn             SessionStart UserPromptSubmit Stop SessionEnd
tool-turn              SessionStart UserPromptSubmit PreToolUse PostToolUse PostToolBatch Stop SubagentStop SessionEnd
esc-generation         SessionStart UserPromptSubmit SessionEnd
esc-tool               SessionStart UserPromptSubmit PreToolUse SessionEnd
permission-deny        SessionStart UserPromptSubmit PreToolUse PermissionRequest SessionEnd
permission-esc         SessionStart UserPromptSubmit PreToolUse PermissionRequest SessionEnd
deny-rule              SessionStart UserPromptSubmit PreToolUse PostToolBatch Stop SubagentStop SessionEnd
tool-error             SessionStart UserPromptSubmit PreToolUse PostToolUseFailure PostToolBatch Stop SubagentStop SessionEnd
subagent               SessionStart UserPromptSubmit PreToolUse SubagentStart SubagentStop PostToolUse PostToolBatch Stop SubagentStop SessionEnd
clear-command          SessionStart UserPromptSubmit Stop SessionEnd SessionStart UserPromptSubmit Stop SessionEnd
idle-notification      SessionStart UserPromptSubmit Stop Notification SessionEnd
blocked-notification   SessionStart UserPromptSubmit PreToolUse PermissionRequest Notification SessionEnd
```

The three `SessionEnd`s in the silent rows are the driver's own `/exit` at the
end of the scenario — they are not the transition under test.

### Payload shapes, as recorded

Every event carries `session_id`, `transcript_path`, `cwd`, `hook_event_name`.
Turn-scoped events add `prompt_id` and `permission_mode`; most add
`effort: {level}`.

```json
{"session_id":"a8e2f3d1-…","transcript_path":"…/a8e2f3d1-….jsonl",
 "cwd":"…/scratch","hook_event_name":"SessionStart","source":"startup",
 "model":"claude-opus-5[1m]"}

{"…,"hook_event_name":"PermissionRequest","tool_name":"Bash",
 "tool_input":{"command":"echo spike-permission-probe","description":"Echo probe string"},
 "permission_mode":"default","effort":{"level":"high"}}

{"…,"hook_event_name":"Notification","message":"Claude needs your permission",
 "notification_type":"permission_prompt"}

{"…,"hook_event_name":"SubagentStop","agent_id":"a7d7604b5dfd35b1e",
 "agent_type":"general-purpose","stop_hook_active":false,
 "agent_transcript_path":"…/subagents/agent-a7d7604b5dfd35b1e.jsonl"}
```

`transcript_path` is the session id: `<session_id>.jsonl` under the project's
history directory. The `--resume` ref and the `session_id` are the same string
(the TUI prints `Resume this session with: claude --resume <session_id>` on
exit, and the resumed session reports that id back).

---

## 3. The ten questions

### M1 — Does `Stop` fire on an Esc-interrupt? — **No. Nothing fires.**

**Measured, both phases.**

*During generation* (`esc-generation`): prompt submitted at +2.908, `Esc` at
+6.959, observation window held open to +16.960. The log between them is empty;
`SessionEnd` at +17.488 is the driver quitting. The terminal's paint log shows
`Interrupted · What should Claude do instead?` — the interrupt landed.

*During a running tool call* (`esc-tool`, a pre-approved `until [ -f … ]; do
sleep 2; done`): `PreToolUse{tool_name: Bash}` at +5.756, `Esc` at +9.853,
window to +19.853. No `PostToolUse`, no `PostToolUseFailure`, no
`PostToolBatch`, no `Stop`. Same `Interrupted · …` on screen.

The control that makes this a finding rather than a broken harness:
`PostToolUseFailure` *does* fire, in the same scratch project, with the same
settings file, when a tool fails on its own — `tool-error` recorded
`PostToolUseFailure{tool_name: Bash, is_interrupt: false}` 0.245 s after
`PreToolUse`. The event exists, is subscribed, and dispatches; a user interrupt
is simply not among the things it reports.

Read from the shipped artifact, and *not* reproduced: the bundle's
`PostToolUseFailure` emitter takes an `is_interrupt` argument, so some code path
does report an interrupt this way. No scenario found it. **Interrupt-exit is
screen-owned.**

### M2 — Manual permission deny/cancel — **Nothing fires, either way.**

**Measured.** With `permissions.ask: ["Bash(echo:*)"]` forcing the dialog:

- *Answered "No"* (`permission-deny`): `PermissionRequest` at +5.864, selection
  moved to "No" at +8.467, confirmed at +9.068, window to +23.068. Nothing.
  The paint log shows the selection moving (`Yes` / `❯No`) and then
  `Interrupted · What should Claude do instead?`.
- *Dismissed with Esc* (`permission-esc`): `PermissionRequest` at +5.695, Esc at
  +8.305, window to +20.306. Nothing. Same screen text.

`PermissionDenied` is subscribed in every scenario and **never fired once**,
including for a call refused by a `permissions.deny` rule (`deny-rule`: the
tool response was `"Permission to use Bash with command whoami has been
denied."` and the only events were `PreToolUse` → `PostToolBatch` → `Stop`).

So the plan's provisional edge **`PermissionDenied` → `Working` is dead
data** — the event does not fire on any deny path this machine could produce.
What amx can rely on instead: `Blocked` is *entered* by `PermissionRequest` and
must be *left* by tier-2 or by the staleness deadline. The two exits are
indistinguishable to hooks and, usefully, indistinguishable on screen too:
both leave `Interrupted · What should Claude do instead?`.

**Unmeasured:** whether `PermissionDenied` fires when a `PreToolUse` hook
returns a deny decision (amx's hooks never will) or when the model's own
classifier denies. Would be measured by a scratch hook that emits
`{"permissionDecision":"deny"}`.

### M3 — Does `PermissionRequest` fire when the dialog paints? — **Yes, ~11 ms before it.**

**Measured**, three times:

| Scenario | dialog paint − hook start |
|---|---|
| `permission-esc` | +0.011 s |
| `permission-deny` | +0.008 s |
| `blocked-notification` | +0.014 s |

The hook process starts *before* the terminal is told to paint the dialog, so a
`Blocked` state asserted from `PermissionRequest` is never late relative to the
screen — the fusion machine can trust the edge and use tier-2 only to leave it.

The payload names what is being asked: `tool_name` plus the full `tool_input`
(for Bash, the exact command and its description). That is enough for a status
line to say *what* the agent is blocked on, not merely that it is.

### M4 — Subagent events and the parent's `Stop` — **`agent_id` is a reliable discriminator, and late subagent stops are real.**

**Measured.**

- Ordering for a real Task subagent (`subagent`): `PreToolUse{Agent}` +5.680 →
  `SubagentStart{agent_id: a7d7…, agent_type: general-purpose}` +5.715 →
  `SubagentStop{same agent_id}` +7.707 → `PostToolUse{Agent}` +7.756 →
  `PostToolBatch` +7.796 → `Stop` +8.934. Nested, in order, before the parent.
- **`agent_id` is present on every subagent-scoped event and absent from every
  parent event**: across the whole run, all 44 `Stop` payloads lack an
  `agent_id` key and all 8 `SubagentStop` payloads carry one. The bundle agrees:
  `Stop` and `SubagentStop` are built by the same function, and `agent_id` is
  only added on the `SubagentStop` branch.
- **The hazard is confirmed and it is not rare.** A *second*, anonymous
  `SubagentStop` — `agent_type: ""`, an `agent_id` no `SubagentStart` ever
  announced, an `agent_transcript_path` that does not exist on disk — arrives a
  couple of seconds **after the parent's `Stop`** on essentially every
  tool-using turn:

  | Scenario | after the parent's `Stop` |
  |---|---|
  | `subagent` | 1.90 s |
  | `tool-turn` | 2.00 s |
  | `deny-rule` | 2.07 s |
  | `resume-session` | 2.21 s |
  | `tool-error` | 3.04 s |
  | `esc-tool` (first run, tool call refused by the agent's own policy) | 5.11 s |

  It never appeared on a turn with no tool call (`clean-turn`, `clear-command`,
  `session-end`, `fresh-trust`, `idle-notification` — none). An eighth
  `SubagentStop`, 25 s into `/compact`, is the compaction worker: also anonymous,
  but its role is legible from where it sits.

This is exactly herdr's "falsely idled the parent" failure with the polarity
that matters for amx: a pane that has already gone `Idle` gets a stop-shaped
event 2 s later. If the fusion machine treated `SubagentStop` as a parent edge
it would churn state after every single tool turn. **The rule "subagent-scoped
events never touch the parent's state" is load-bearing, and `agent_id`'s
presence is the whole discriminator.**

**Unmeasured:** what that anonymous subagent is. Its transcript is not written,
and nothing in the payload names it. It does not change the rule.

### M5 — `Notification` — **Fires, and its payload distinguishes the two waits.**

**Measured.**

| Wait | Fires after | Payload |
|---|---|---|
| Permission dialog unanswered | **6.0 s** (`blocked-notification`: dialog +5.237, notification +11.239) | `notification_type: "permission_prompt"`, `message: "Claude needs your permission"` |
| Idle at the prompt | **60 s** (`idle-notification`: `Stop` +4.872, notification +64.918; `esc-tool` reproduced it at 60.1 s) | `notification_type: "idle_prompt"`, `message: "Claude is waiting for your input"` |

herdr never had this. For amx it is a genuine second witness: a
`permission_prompt` notification corroborates a held `Blocked` six seconds in,
and an `idle_prompt` notification is a positive assertion of idleness a full
minute after a turn — including, usefully, **after a silent Esc-interrupt**,
which is otherwise invisible to hooks entirely. It is a backstop, not an edge:
60 s is far too slow to drive a status line, but it is a free contradiction of
any `Working` state the machine is still holding.

### M6 — Do hook processes inherit the launching terminal's environment? — **Yes.**

**Measured, on the real chain.** The driver plants `AMX_ENV`, `AMX_SESSION`,
`AMX_SOCKET`, `AMX_PANE_ID`, `AMX_WORKSPACE_ID`, `AMX_HOOK_TOKEN` in the
*shell's* environment, then types `claude` at that shell — the same path a
hand-typed agent takes in an amx pane. All 185 hook invocations
recorded in this run carry all six variables verbatim. The logger's `/proc`
ancestry shows the chain:

```
2256660:bash        <- the hook process
2256617:claude      <- the agent
2256613:bash        <- the pane's shell (where AMX_* was planted)
2256610:python3     <- the driver
2256608:zsh
```

The same holds for Codex (§4). **D-M2-4's identity scheme stands as designed**,
and the degraded branch in §2's "worse than hoped" (identity by `session_id` +
process tree) is not needed.

One caveat worth an installer note: the hook *command string* must not depend on
the environment for anything it cannot do without. The spike's logger takes its
log path as an argument for exactly this reason — a design amx already matches,
since `amx _hook` needs `AMX_SOCKET` and exits 0 silently when it is absent.

### M7 — Do freshly written project hooks run without an interactive prompt? — **Yes, but only after the folder is trusted.**

**Measured** (`fresh-trust`, a scratch directory Claude Code had never seen):

```
+0.053  agent_launch
+0.407  trust_dialog_painted        "Yes, I trust this folder"
        … 8 seconds held, log stays empty …
+8.455  trust_accepted (Enter)
+9.020  HOOK SessionStart
```

No separate hook-approval step exists, and the settings file was written
seconds before launch. But the folder-trust dialog is a hard gate: during the
eight seconds it was up, **no hook fired at all**.

Two details for V10:

- The trust dialog itself enumerates what the project settings pre-approve
  ("⚠ This folder pre-approves 2 tool permissions in `.claude/settings.json`")
  and does **not** mention hooks.
- The bundle carries a matching diagnostic for the untrusted case — *"Ignoring
  N entries from .claude/settings.json: this workspace has not been trusted.
  Run Claude Code interactively here once and accept the trust dialog, or set
  `projects[<cwd>].hasTrustDialogAccepted: true`"* — emitted for
  `permissions.allow` and `permissions.additionalDirectories`. **Read from the
  artifact; not reproduced for the `hooks` key.**

So `amx integration install` *is* non-interactive, and honest status wording is:
installed hooks run in any workspace the user has already trusted; in a brand
new one, the user's next interactive launch asks once.

### M8 — Session identity — **UUID, stable across `--resume` and compact, replaced by `/clear`.**

**Measured.**

- Format: a v4-shaped UUID, e.g. `c9a3c73b-b184-4871-8e98-79b46b87b635`. The
  transcript is `<session_id>.jsonl`; the resume ref *is* the session id.
- `SessionStart.source` values observed: **`startup`**, **`resume`**,
  **`clear`**, **`compact`**. (The bundle also validates `fork`; not produced by
  any scenario — **unmeasured**.)
- `--resume <id>` → `SessionStart{source: resume}` with the **same** id, and the
  conversation carried (the resumed session recalled the word the first session
  was told to remember).
- `/clear` → `SessionEnd{reason: clear}` then `SessionStart{source: clear}` with
  a **new** id. **A ref captured before `/clear` is stale.**
- `/compact` → same id throughout; `SessionStart{source: compact}` does not
  change the ref.
- `SessionEnd.reason` observed: `prompt_input_exit` (a `/exit`), `clear`. The
  bundle's full set is `clear | resume | logout | prompt_input_exit | other |
  bypass_permissions_disabled` — read from the artifact.

For D-M2-7 this means: **amx must take the ref from every `SessionStart`, not
just the first**, because `/clear` mints a new conversation inside one process,
and the `source` field is the only warning.

### M9 — Hook dispatch latency — **median 26 ms, and two parallel hooks land 1 ms apart.**

**Measured**, 27 turns across the matrix. "Keystroke" is the driver's `write()`
of the Enter that submits the prompt; "hook" is `$EPOCHREALTIME` taken as the
first statement of the hook process.

| From → to | n | min | median | max |
|---|---|---|---|---|
| Enter → `UserPromptSubmit` hook starts | 27 | 0.008 s | **0.026 s** | 0.053 s |
| `claude` typed → `SessionStart` hook starts | 21 | 0.901 s | 1.076 s | 8.967 s |
| Two hooks subscribed to the same event | 57 | 0.0001 s | **0.0010 s** | 0.0052 s |
| `PermissionRequest` hook start → dialog paints | 3 | +0.008 s | +0.011 s | +0.014 s |

Only the first and third rows are dispatch latency. The `PreToolUse` and `Stop`
rows (2.3–2.7 s median from the keystroke) are dominated by model inference and
say nothing about the hook system.

Two consequences: hook edges are **instant** on any timescale the fusion machine
cares about (26 ms against a ≥100 ms screen evaluation spacing), and hooks that
run in parallel for one event arrive within ~1 ms, so an emitter-side sequence
number needs sub-millisecond resolution — `SystemTime` nanos, as D-M2-4
specifies, is right, and a whole-millisecond counter would tie.

### M10 — Codex — see §4. Installed, logged in, and measured.

---

## 4. Codex CLI 0.147.0

The plan's Codex research (D-M2-1, R-M2-1, R-M2-2) describes version 0.114 from
third-party documentation. Almost none of it survives contact with 0.147.0.

### Where the plan was wrong, measured

| Plan says | Measured on 0.147.0 |
|---|---|
| Hooks are experimental behind `[features] codex_hooks = true` (R-M2-2) | `codex features list` prints `hooks  stable  true` — **enabled by default, no flag to set**. There is no `codex_hooks` feature; the name exists in the binary only as the Rust *crate* path `codex_hooks::engine::command_runner`, which is the likeliest source of the third-party claim. 04 §5's `[features] hooks` was closer to right than the plan's correction |
| Small event set: `SessionStart`, `UserPromptSubmit`, `PreToolUse`, `PermissionRequest`, `PostToolUse`, `Stop` | **Eleven** events, listed by Codex's own hooks panel: `PreToolUse`, `PermissionRequest`, `PostToolUse`, `PreCompact`, `PostCompact`, `SessionStart`, `SessionEnd`, `UserPromptSubmit`, `SubagentStart`, `SubagentStop`, `Stop` |
| Payload schema undocumented | Payloads are Claude-shaped: `session_id`, `transcript_path`, `cwd`, `hook_event_name`, `model`, `permission_mode`, plus `turn_id` on turn-scoped events |
| Config lives in `config.toml` | **`$CODEX_HOME/hooks.json`**, a JSON file with PascalCase event keys, each an array of `{matcher?, hooks: [{type: "command", command: "…"}]}` — the same shape as Claude Code's `settings.json` block |
| A trust gate exists | **Confirmed, and stronger than described** — see below |

### The trust gate, measured

Starting `codex` with a freshly written `hooks.json` paints a **blocking**
startup prompt before the session begins:

```
Hooks need review
11 hooks are new or changed.
Hooks can run outside the sandbox after you trust them.
› 1. Review hooks
  2. Trust all and continue
  3. Continue without trusting (hooks won't run)
```

and a panel that scores them:

```
⚠ 11 hooks need review before they can run.
Event               Installed   Active   Review
PreToolUse              1          0        1     Before a tool executes
…
Stop                    1          0        1     Right before Codex ends its turn
Press t to trust all; enter to review hooks; esc to close
```

- **Declining is silent and total** (`untrusted-hooks` scenario): choosing
  "Continue without trusting", then running a full turn, produced **zero** hook
  invocations.
- `codex exec` (non-interactive) with an untrusted `hooks.json` also fired
  nothing, and printed no warning — an authenticated turn ran to completion with
  the hooks inert. There is no non-interactive way to grant trust that this
  spike found.
- The gate is keyed to content: the binary's config struct is
  `HookStateToml { enabled, trusted_hash }`. Re-writing the file re-arms the
  prompt (measured: a second scratch `CODEX_HOME` asked again).
- Malformed config *is* reported (`warning: failed to parse hooks config …:
  invalid type: integer 42, expected struct HookEventsToml`), so silence means
  "parsed and inert", not "not found".

**V10 must say this out loud**: `amx integration install` for Codex writes the
file and then tells the user their next `codex` run will ask them to trust the
hooks, and `amx integration status` must report that it cannot see trust state
(no user-readable trust record was found by this spike — **unmeasured** where
`trusted_hash` is persisted).

### What fires, measured

| Transition | Hook events | Scenario |
|---|---|---|
| Process start | **nothing** | all |
| First prompt submitted | `SessionStart{source: startup}` then `UserPromptSubmit`, ~25 ms apart | all |
| Turn ends | `Stop{stop_hook_active: false}` | all |
| Tool call (read a file) | `PostToolUse{tool_name: Bash}` — **`PreToolUse` did not fire** | `tool-turn` |
| Tool call needing approval | `PreToolUse{Bash}` → `PermissionRequest{Bash}`, 38 ms **before** the dialog paints | `approval-*` |
| …approved | `PostToolUse` → `Stop` | first `approval-deny` run |
| …answered **"No, and tell Codex what to do differently"** | **—** (nothing within 14 s) | `approval-deny` |
| …dismissed with **Esc** | **—** (nothing within 14 s) | `approval-esc` |
| **Esc, twice, during a running shell command** | **—**, and the command *kept running*: `PostToolUse` only arrived when the command's own exit condition was met, 11 s after the second Esc | `esc-tool` |
| Hooks configured but not trusted | **—**, for an entire turn | `untrusted-hooks` |
| `codex resume <id>` | `SessionStart{source: resume}`, **same session id**, conversation carried | `resume-session` |
| `/quit` | `SessionEnd{reason: "other"}` | `session-end` |

Codex's approval dialog is worth quoting, because its third option *is* Esc —
"No" and "cancel" are one action, not two:

```
Would you like to run the following command?
  Environment: local
  $ echo spike-codex-probe > /tmp/amx-spike-codex.txt
› 1. Yes, proceed (y)
  2. Yes, and don't ask again for commands that start with `…` (p)
  3. No, and tell Codex what to do differently (esc)
```

Choosing 3 paints `✗ You canceled the request to run …` and emits nothing. So
Codex's `Blocked` exits are exactly as hook-invisible as Claude Code's, and amx
does not need a per-agent rule for them.

Two Codex-specific findings that change how amx must treat it:

1. **The session does not exist until the first prompt.** `SessionStart` fired
   0.58 s *after* the prompt keystroke, 6.5 s after the process started
   (`clean-turn`: launch +0.053, ready +2.809, prompt +6.609, `SessionStart`
   +7.195). A pane running an idle Codex has **no hook-borne identity at all**;
   tier-3 process identity is the only thing that knows it is Codex until the
   user speaks. Claude Code fires `SessionStart` at launch (+1.08 s median), so
   this is not a shared property of hook systems and must not be assumed.
2. **`PreToolUse` is conditional.** A plain file read produced `PostToolUse`
   with no `PreToolUse`; a command needing escalation produced both. So for
   Codex, `PreToolUse` is not a dependable `Working` edge — but
   `UserPromptSubmit` is, and `Stop` closes the turn.

Session ids are UUIDv7 (`019fdbc0-…`), and `codex resume <id>` exists.

### Interrupt, and what the recording actually shows

`esc-tool` ran a shell command that blocks until a file appears, pressed `Esc`
five seconds in and again six seconds after that, and watched for ten more:
**no hook fired, and the command was still running** — `PostToolUse` arrived at
+36.3, one second after the harness created the release file at +35.4, eleven
seconds after the second Esc. So on 0.147.0, `Esc` did not interrupt a running
shell command in this configuration, and produced no event either way.

`esc-generation` is separately **inconclusive**: Codex paints a finished message
in one block, the recording shows all 400 lines printed with no `Interrupted`
marker, and the `Stop` in it lands 0.1 s after the driver's `/quit`. The Esc
most likely arrived after the turn was already over.

Either way the conclusion for the fusion machine is the same one `edges` encodes
— **Codex's interrupt is screen-owned** — but the honest statement is "no hook
observed on interrupt", not "Codex has no interrupt event". What is unmeasured
is which key actually interrupts Codex (its own footer says `esc to interrupt`)
and whether *that* key emits anything.

### Left unmeasured for Codex

- Subagent events (`SubagentStart`/`SubagentStop`) — no scenario spawned one.
- Compaction events (`PreCompact`/`PostCompact`).
- `Notification` — Codex has no such event at all, so a Codex pane gets no
  delayed corroboration of a block or an idle. That asymmetry is real and the
  fusion machine must not depend on the backstop Claude Code provides.

---

## 5. Proposed registry data

```toml
[[agent]]
id       = "claude"
coverage = "edges"
# Entry edges measured instant and complete; every user-initiated exit
# (interrupt, deny, cancel) measured silent. V01 §3 M1/M2.
hook_events = [
  "SessionStart", "SessionEnd", "UserPromptSubmit", "Stop",
  "PreToolUse", "PostToolUse", "PostToolUseFailure",
  "PermissionRequest", "SubagentStart", "SubagentStop", "Notification",
]

[[agent]]
id       = "codex"
coverage = "edges"
# Same edge set, but hooks are inert until a human accepts an interactive
# trust prompt, and SessionStart does not fire until the first prompt.
# V01 §4.
hook_events = [
  "SessionStart", "SessionEnd", "UserPromptSubmit", "Stop",
  "PreToolUse", "PostToolUse", "PermissionRequest",
  "SubagentStart", "SubagentStop",
]
```

Events deliberately **not** subscribed: `PostToolBatch` (fires even for calls
that never ran — see `deny-rule` — so it asserts nothing amx wants),
`PermissionDenied` (never fired once), `UserPromptExpansion`, `StopFailure`,
`PreCompact`/`PostCompact` (compaction does not change the pane's status, and
the ref survives it). Subscribing fewer events is not a coverage loss: each
subscription costs a process spawn per occurrence.

`Notification` is subscribed for Claude Code because it is the only positive
signal that survives an Esc-interrupt, and it names which wait it is reporting.

---

## 6. Recommended fusion constants

Derived from §3's M9 and the timings above; where the plan's provisional value
survives, that is a measured confirmation, not an assumption carried forward.

| Constant | Plan's provisional | Recommended | Why |
|---|---|---|---|
| Entry-edge treatment | apply instantly | **unchanged** | 26 ms median dispatch, and `PermissionRequest` beats its own dialog by 11 ms |
| `CONFIRMATIONS` | 3 consecutive verdicts | **unchanged (3)** | nothing measured argues against it; the interrupt screen (`Interrupted · What should Claude do instead?`) is stable text, so 3 evaluations is ~300 ms of certainty |
| Screen evaluation spacing | ≥100 ms | **unchanged** | 100 ms is ~4× the hook dispatch median, so a hook edge always wins a race it should win |
| Confirmation cap | 700 ms | **unchanged** | it bounds the interrupt case, which is now known to be the *common* case, not an exotic one |
| Identity startup grace | 3 s | **raise to 5 s for Codex, keep 3 s for Claude** | Claude Code is ready ~1.1 s after launch; Codex took 2.8–4.6 s to finish its startup gates in these runs, and emits no hook until the first prompt |
| Staleness deadline | 30 s | **keep 30 s, and add the `Notification` corroborations** | `idle_prompt` at 60 s and `permission_prompt` at 6 s are free second witnesses; 30 s still has to stand alone, since a pane with no screen coverage gets nothing else |
| Emitter `seq` resolution | `SystemTime` nanos | **required, not optional** | parallel hooks for one event land 1.0 ms apart (min 0.1 ms); millisecond resolution would tie |

One new constant the plan does not have: **`Blocked` entry may be trusted
outright** for both agents, because `PermissionRequest` precedes the dialog.
Tier-2 is only needed to *leave* `Blocked`.

---

## 7. The edge cases V17 must replay

Each of these is a measured transition; the exit test's fake agents must
reproduce the *hook silence* as faithfully as the hook traffic.

1. **Esc during generation** — `UserPromptSubmit`, then no event ever. The
   screen goes from `esc to interrupt` to `Interrupted · What should Claude do
   instead?`. Pane must settle `Idle` via screen confirmations.
2. **Esc during a tool call** — `PreToolUse`, then no event ever. Same screen
   transition. Pane must settle `Idle` without a `PostToolUse`.
3. **Permission dialog answered "No"** — `PreToolUse`, `PermissionRequest`, then
   nothing. Pane must leave `Blocked` via screen.
4. **Permission dialog cancelled with Esc** — byte-identical hook trace and
   screen text to (3). The fixtures must not distinguish them, because nothing
   downstream can.
5. **Anonymous `SubagentStop` 2 s after the parent's `Stop`** — with an
   `agent_id`, an empty `agent_type`, and a transcript path that does not
   exist. Must not revive the idle pane. This is the *default* behaviour of a
   tool-using turn, so the fixture should emit it after every tool turn, not as
   a special case.
6. **Nested subagent turn** — `PreToolUse{Agent}` → `SubagentStart` →
   `SubagentStop` → `PostToolUse` → `Stop`, all with the parent's `session_id`
   and the subagent's `agent_id`. Parent state changes only at `Stop`.
7. **`/clear` mid-session** — `SessionEnd{reason: clear}` → `SessionStart{source:
   clear}` with a **new** session id in the **same** process. The stored resume
   ref must be replaced, and a restore must resume the new conversation.
8. **`--resume` round trip** — same session id back, `source: resume`.
9. **Rule-denied tool call** — `PreToolUse` → `PostToolBatch` → `Stop`, no
   `PostToolUse`. A tracker that expects `PostToolUse` to close every
   `PreToolUse` will wedge.
10. **`Notification{permission_prompt}` 6 s into a block**, and
    **`Notification{idle_prompt}` 60 s into an idle** — including 60 s after a
    silent interrupt, which is the one hook-borne evidence that an interrupted
    turn ended.
11. **Codex: hooks configured but untrusted** — a whole turn with zero hook
    traffic while the process is plainly a live agent. Tier-2 and tier-3 carry
    it alone; `agent status` must not claim the agent is unidentified.
12. **Codex: no `SessionStart` until the first prompt** — a pane holding an idle
    Codex has no hook-borne session ref, so `agent start codex` readiness cannot
    wait on a hook.
13. **Codex: a denied approval leaves the turn open** — after the cancel,
    nothing arrived for the 14 s the scenario watched: no `PostToolUse`, no
    `Stop`. A tracker that waits for `Stop` to leave `Blocked` waits forever;
    the staleness deadline is what ends it.

---

## 8. Where measurement contradicts the binding documents

Recorded, not fixed — 04 and 08 belong to V02+ and the orchestrator.

1. **08 §D-M2-6's `PermissionDenied → Working` edge cannot be implemented.**
   The event never fires on any deny path measured here (human "No", Esc, or a
   `permissions.deny` rule). V04's table should drop the row rather than
   implement an arm that no input reaches.
2. **08 §D-M2-1 and R-M2-2 describe a Codex that no longer exists.** On 0.147.0
   hooks are `stable` and on by default, the flag is `hooks` (04 §5's spelling),
   the event set is eleven, and the config is `$CODEX_HOME/hooks.json`. The
   `codex_hooks` name is a crate path inside the binary. R-M2-2's "04 names the
   wrong flag" should be withdrawn; 04 §5 was right.
3. **R-M2-1's fallback is not needed.** Codex is installed, authenticated and
   measured; its stanza ships `edges`, not `identity`. 04 §5's table calls Codex
   "hooks experimental, behind a feature flag today" — that clause is now stale.
4. **08 §2's "worse than hoped" branch for environment inheritance is moot.**
   Inheritance works on both agents, through an interactive shell.
5. **04 §5 lists interrupts and dialog cancels as the coverage gap.** Measured,
   the gap is wider in one direction and narrower in another: *every* exit-by-
   user is silent (so tier-2 owns all of them), but `Notification` gives a
   delayed positive idle signal herdr never had, and `PermissionRequest`
   precedes its own dialog, so the `Blocked` *entry* is more trustworthy than
   04 §5 assumes.

---

## 9. Leftovers — what this machine did not answer

| # | Question | Why not | What would measure it |
|---|---|---|---|
| L1 | Does `PostToolUseFailure{is_interrupt: true}` ever fire? | The field exists in the shipped bundle; no scenario produced it | Interrupt paths other than Esc — SIGINT to the agent, a tool timeout, a `/` command mid-tool |
| L2 | Does `PermissionDenied` fire when a `PreToolUse` hook denies? | amx's emitter never denies, so the spike never emitted a decision | A scratch hook returning `{"permissionDecision":"deny"}` |
| L3 | `SessionStart{source: fork}` | The bundle validates it; nothing in the matrix forks a session | Drive whatever UI forks a session (`/subtask`? a worktree flow) and watch `source` |
| L4 | Auto-compaction (as opposed to `/compact`) | Needs a context large enough to trigger it; the scratch project is tiny | A long scripted session that fills the window, watching for `PreCompact{trigger}` ≠ `manual` |
| L5 | What the anonymous `SubagentStop` belongs to | No transcript is written and the payload does not name it | Reading the parent transcript for a matching internal turn, or a Claude Code release note |
| L6 | Which key actually interrupts Codex, and whether it emits anything | Two `Esc` presses neither interrupted a running command nor emitted an event; its footer still says `esc to interrupt` | Try `Ctrl-C` and `Esc Esc` against a blocking command, asserting on screen that Codex acknowledged the interrupt |
| L7 | Codex subagent, compaction and resume-after-`/clear` behaviour | No scenario spawned a subagent or filled the context; Codex has no `/clear` equivalent in this matrix | Extend `codex-matrix.py` with the three scenarios the Claude matrix already has |
| L8 | Where Codex persists hook trust (`trusted_hash`) | Not found in the throwaway `CODEX_HOME` this spike inspected | Diff a `CODEX_HOME` before and after answering "Trust all" |
| L9 | Whether Claude Code's `hooks` key is dropped in an untrusted workspace the way `permissions.allow` is | The diagnostic in the bundle names `permissions.*`; the trust dialog blocks the session anyway, so the case may be unreachable | Set `hasTrustDialogAccepted: false` for a workspace that already has hooks and watch stderr |
| L10 | macOS behaviour of any of the above | Linux only; darwin CI exists but has no interactive agent | Re-run `scripts/spike/` on the darwin box |
| L12 | **04 §5's `full` class has never been measured** — Pi, OMP, OpenCode and Kilo are listed there on herdr's production data, not on any experiment | Out of M2's scope: the milestone ships `claude` and `codex` stanzas only. `pi` **0.84.1 is installed on this machine**, and its extension surface (`pi install <source>`, settings under `~/.pi/agent/settings.json`) is not a hook-event list, so it needs its own config discovery before a matrix can run | A second spike per agent, reusing `scripts/spike/lib/` — the driver and analyzer are agent-agnostic; only the scratch-config builder and the startup gates are per-agent (`lib/scratch.py`, ~40 lines each) |
| L11 | Whether these findings survive the next weekly release | Agents ship weekly; this is 2.1.224 / 0.147.0 on 2026-08-07 | Re-run the harness — that is what it is for |

---

## 10. Re-running this

```sh
scripts/spike/claude-matrix.py --out /tmp/spike            # all Claude scenarios
scripts/spike/claude-matrix.py --out /tmp/spike --only esc-generation
scripts/spike/codex-matrix.py  --out /tmp/spike-codex
scripts/spike/analyze.py /tmp/spike                        # timelines
scripts/spike/analyze.py /tmp/spike --latency              # the M9 table
scripts/spike/analyze.py /tmp/spike --payloads Stop        # raw payloads
```

The Claude matrix is unattended and takes about twelve minutes; the two
90-second notification scenarios are most of it. The Codex matrix needs a
logged-in Codex and answers its trust gate itself, but that gate is a real
prompt — a Codex run is only unattended because the driver types the answer.

Each run writes, under `--out`: `hooks.jsonl` (hook invocations and driver
marks on one clock), `dumps/<scenario>.txt` (everything the terminal painted),
`rec/<scenario>.raw` + `.chunks.jsonl` (the raw byte stream with arrival
timestamps, so any timing question can be re-asked offline), and
`results.json`.

Nothing in the harness touches the user's own configuration: the Claude scratch
project is disposable, and the Codex run builds a throwaway `CODEX_HOME` that
borrows credentials by symlink.

---

## 11. The M2 live smoke

**Status: not yet run.** This is the outstanding half of M2's exit criterion.

Why it exists, in one sentence from the M2 plan (R-M2-14): "the exit criterion
is only as strong as fake-agent fidelity". `tests/agents.rs` drives five
scripted agents through every transition §7 lists, over the real `amx` binary
and the real socket — but the agents are shell scripts, and twice now a green
suite has hidden a feature that did not work. So M2 does not exit on green
tests. It exits on green tests **plus this checklist, run by hand against real
Claude Code, with its date and versions written into the table below.**

Each step names what would make it fail, because a checklist whose steps are
only "do X" gets ticked without being read.

| # | Step | Passes when | Fails as |
|---|---|---|---|
| 1 | `amx integration install claude` in a project, then `amx integration status` | reports `current`, and names the `amx` binary it wrote into the settings | `current` on an installation whose binary is gone — herdr's exact bug, which V10's status check exists to refuse |
| 2 | `amx agent start a1 --kind claude` … through `a5`, five real sessions | each returns ready with the pane really at Claude Code's prompt, not mid-banner | a `ready` that arrives while the splash is still painting (§6's grace, and W-3 in the plan's wave outcomes) |
| 3 | prompt `a1`, watch the status line | `working` within a frame or two of the keystroke | a status that lags the screen — the hook edge is not arriving |
| 4 | **Esc during generation** on `a1` | settles `idle` inside about a second, from the screen alone | still `working` thirty seconds later: that is the staleness deadline doing tier 2's job, and the manifest has rotted |
| 5 | **Esc during a tool call** on `a2` | same | same |
| 6 | ask `a3` for a tool call that needs permission | `blocked` as the dialog paints, `⚑1` on the status line | a block the screen shows and the status line does not |
| 7 | answer that dialog **"No"** | leaves `blocked`, `⚑` clears | stuck blocked: nothing fires on a deny, so this is tier 2 alone |
| 8 | block `a4` and `a5` too, then press the `next-attention` chord repeatedly, answering each | focus walks the blocked set in block order, across workspaces | an order that is creation order, or a chord that lands on a pane already answered |
| 9 | let one agent run a tool-using turn and sit for two seconds after it ends | stays `idle` | flips back to `working`: the anonymous `SubagentStop` (§3 M4) has revived the pane |
| 10 | `amx agent explain a1` | names the rule that matched and reports the others with evidence | a matched rule that is not the one the screen shows |
| 11 | `amx wait --until blocked` on a running agent, from a second terminal | returns the moment the dialog paints | returns late, or not at all |
| 12 | `amx session stop`, then `amx` again | all five panes come back and each one's Claude Code resumes **its own** conversation, visible in its transcript | a pane that comes back as a bare shell, or two panes resuming the same conversation |
| 13 | `examples/notify.sh` running against `amx events --json` throughout | one desktop notification per block | notifications for the wrong events, or none |

Record the run here:

| Date | amx | Claude Code | Platform | Steps that failed | Notes |
|---|---|---|---|---|---|
| — | — | — | — | — | not yet run |

A step that fails is a finding, not a blocker to be argued around: write it in
the row, and file it against the tier it belongs to. A manifest that has rotted
against a weekly release is the *expected* failure of steps 4–7, and the fix is
to re-record the fixtures under `crates/amx-server/tests/fixtures/manifest/`
and change the rule — which is what §10's harness and that directory's README
are for.
