# M2 execution plan

The build plan for **M2 — Agents** ([05-roadmap.md](05-roadmap.md)). Binding
design is [04-architecture.md](04-architecture.md) §5 (the agent layer) and §2
(the actor runtime); this document does not change them. Where research
contradicted or complicated a decision in 04/05, it is recorded in
[§7 Risks](#7-risks--findings) rather than silently redesigned.

Everything below that states a fact about the amx tree, herdr, Claude Code, or
Codex was read from source or from the tools' own documentation during
planning. The single largest unknown — how agent hook systems behave on the
transitions their docs do not describe — is deliberately **not** resolved here:
that is V01, the empirical spike, and [§2](#2-the-hook-coverage-spike-v01)
states exactly which later decisions wait on its findings and what happens to
them under each outcome.

Process lessons inherited from M0/M1 and applied throughout: a named
integration task owns every cross-crate seam (06 §T19 — the seams the
file-ownership discipline leaves unowned by construction), the contracts task
splits budget-edge files *before* the waves need them (07 R-M1-3), the retired
`seam` helper comes back together with its hygiene exemption (07 §5), and the
suites smoke the real binary — twice now, a green suite has hidden a
non-working feature until a live run caught it.

---

## 1. Decisions taken during research

### D-M2-1 — The spike is task one, and it is a measurement, not a reading

05 M2 orders it: "Registry coverage classes (full/edges/identity/none) are set
from measurements, not hope." Reading the docs is not the measurement — it is
the inventory of what the docs refuse to say. Both were done during planning:

- **Claude Code** (2.1.224 installed on the dev machine) now documents a far
  richer event set than the herdr-era one: `SessionStart`, `SessionEnd`,
  `UserPromptSubmit`, `Stop`, `StopFailure`, `PreToolUse`, `PostToolUse`,
  `PostToolUseFailure`, **`PermissionRequest`**, **`PermissionDenied`**,
  `SubagentStart`, `SubagentStop`, `Notification`, and more
  (code.claude.com/docs/en/hooks.md). Every hook receives `session_id`,
  `transcript_path`, `cwd`, `hook_event_name` on stdin; subagent-scoped events
  carry `agent_id`; `SessionStart` matches on
  `source ∈ {startup, resume, clear, compact, fork}`; hooks run in parallel;
  project-level `.claude/settings.json` hooks need no interactive approval per
  the docs. The docs are **silent** on exactly the transitions that killed
  herdr's hooks-authoritative attempt: whether `Stop` fires on an
  Esc-interrupt, what (if anything) fires when a permission dialog is cancelled
  by hand, and whether hook processes inherit the launching terminal's
  environment (which the whole `AMX_PANE_ID` identity scheme rides on).
- **Codex** is *not installed* on the dev machine. Third-party documentation
  (mid-2026) says hooks shipped experimentally in v0.114 behind
  `[features] codex_hooks = true` — note the key is `codex_hooks`, not the
  `[features] hooks` that 04 §5's table and herdr's installer both use (R-M2-2)
  — with a small event set (`SessionStart`, `UserPromptSubmit`, `PreToolUse`,
  `PermissionRequest`, `PostToolUse`, `Stop`), payload schema undocumented, and
  a **trust gate**: Codex requires interactive review of each hook definition,
  recorded against the hook's content hash, before it will run it. `codex
  resume <id>` / `codex exec --resume-session-id <id>` exist.

Section 2 turns the silences into the measurement protocol. The plan's shape
is deliberately class-agnostic: whatever V01 finds changes *data* (registry
stanzas, fusion constants, the exit test's edge list), never task boundaries.

### D-M2-2 — Registry: one embedded `agents.toml`, parsed once at startup

**Decision:** the registry is a declarative TOML asset embedded with
`include_str!`, parsed once into a `Registry` value at server start, merged
with an optional user override (`$XDG_CONFIG_HOME/amx/agents.toml`), and handed
to every consumer as data. No proc-macro, no build-script codegen, no
`macro_rules!` table.

The alternative weighed was M0's method-table precedent (`method_table!` in
`amx-proto/src/control/mod.rs:52-189`). The method table earns its macro
because its consumers are *compile-time artifacts*: an enum the compiler
matches exhaustively, a dispatch trait whose missing handler is a build error.
The registry's consumers — name/alias lookup, labels, resume argv templates,
coverage classes, manifest bindings, integration asset selection — are all
runtime data, and 04 §5 requires the compiled-in registry to be *overridable*,
which forces a runtime parse-and-merge path to exist anyway. Two parse paths
(macro for the builtin, TOML for the override) would be W6 wearing a new hat.
One TOML shape, parsed once, is the honest model; "generated from it" in 04 §5
is satisfied by every consumer *deriving* from the one file at load, with no
second list anywhere (R-M2-9 flags the wording).

A stanza carries: `id`, `aliases`, `label`, `executables` (basenames the
identity tier matches), `coverage` (`full|edges|identity|none` — set by V01),
`start` argv, `resume` argv template with exactly one `{ref}` placeholder,
`ref_kind` (`id|path`), `hook_events` (which events the installer subscribes),
and `manifest` (file name under the bundled manifest dir). What replaces the
compile-time guarantees is the **conformance test** (V03): it walks the parsed
embedded registry and asserts every generated surface agrees — ids and aliases
unique across stanzas, every named manifest present and compiling, every
resume template well-formed (exactly one `{ref}`, absolute or bare program
name, no shell metacharacters — argv is data, never a shell string), every
`edges`/`full` stanza naming an integration asset, every `coverage` value
paired with the tiers it needs. A stanza that lies fails the build's test run,
which is as close to a compile error as data gets.

M2 ships two stanzas, `claude` and `codex`, matching 05 M2's installer scope.
The override merge is also the test seam: the rig plants an override stanza
for a scripted fake agent instead of patching the binary (V17).

### D-M2-3 — Detection reads the published snapshot; the text view is new

What tier-2 needs to read exists structurally and is missing textually:

- `SnapshotFeed` (`amx-server/src/actor/pane_host/mod.rs:309-348`) is a
  cloneable, lock-free reader handle over the parser thread's double-buffered
  POD snapshot — `latest()`, `frame()` (snapshot + generation as an atomic
  pair), `changed().await`. The per-client grid pumps already consume it
  (`conn/streams.rs:132-137`); a detector consuming it the same way contends
  with nothing, exactly as 04 §3 promises.
- But there is **no text surface anywhere**: `Row::text(&cell)` yields one
  cell's UTF-8 bytes at a time (`amx-vt/src/snapshot.rs:89`), and nothing
  concatenates a row. Rule matching over per-cell fragments would allocate per
  evaluation or mis-split graphemes. V05 adds `Row::line() -> &str` (served
  from the row's existing contiguous `text: Vec<u8>` backing, which only
  `snapshot.rs` can reach — its fields are private) and
  `Snapshot::tail(n) -> impl Iterator<Item = &Row>`, in `amx-vt`, where the
  bytes already live.

herdr's hardest-won detection lesson is structural for amx rather than
vigilance: herdr anchors its detection buffer to the scrollback bottom and
regression-tests that a scrolled viewport never changes it
(`herdr/src/pane/terminal.rs:4798-4813`), because its server owns the scrolled
view. amx's published snapshot **is** the live visible grid — scrollback and
scroll position are client-side by design (04 §3) — so "detection never reads
the scrolled viewport" is true by construction, not by test. What amx keeps
from herdr instead: the whitelisted *region* vocabulary (`whole_recent`,
`bottom_lines(N)`, `bottom_non_empty_lines(N)`, `title`) so manifest rules
cannot drift into whole-scrollback greps, and evaluation gated on content
change — for amx that is the `PaneDamage` event stream plus per-pane
coalescing in `AgentHub`, not a 300 ms timer (03 §5: push, never poll).

### D-M2-4 — `amx _hook` speaks the control protocol; identity is env + token

The emitter is a hidden subcommand of the one binary: read the agent's hook
payload from stdin, map it to a single `agent.report` control call over the
session socket, exit 0 no matter what. No sh-plus-python heredoc — herdr's
hooks silently no-op when `python3` is missing and its `integration status`
still reports `current` because it only greps a version comment
(`herdr/src/integration/registry.rs:421-425`). amx's emitter *is* the binary
that already speaks the protocol, so the failure mode is deleted rather than
detected. (herdr's own Windows asset already works this way — shelling to the
herdr CLI — which is the strongest evidence the POSIX heredoc was never
necessary.)

**Wire path:** the existing session socket and control channel — one surface,
per 04 §4 / W3; no side-channel datagram socket. `agent.report` is an ordinary
method-table row, which means it owes the full coverage set: a proto golden, a
`sample_params` arm in the skew harness, and a dispatch handler (all V02). The
call sequence is connect → Hello/Welcome → one request → read reply → exit,
under a total budget of ~500 ms; any failure (no socket, refused, timeout,
error reply) is a silent success from the agent's point of view — a hook must
never break or slow a turn.

**Identity:** every pane spawn injects `AMX_ENV=1`, `AMX_SESSION`,
`AMX_SOCKET`, `AMX_PANE_ID`, `AMX_WORKSPACE_ID`, and `AMX_HOOK_TOKEN` (V07;
`pty_command()` currently spawns with an empty env-additions vec,
`core/pane.rs:495`). Hook processes inherit the agent's environment, the agent
inherits the pane's shell's, so a `claude` typed by hand is attributed exactly
like one `agent start` launched — *if* the inheritance chain actually reaches
hook processes, which the docs do not state and V01 measures (M6 in §2).
The token is a per-spawn random value the server remembers per pane;
`agent.report` carries pane id + token and `AgentHub` drops mismatches. This
is not a security boundary — the socket is 0600 and any same-user process can
already drive every pane — it is a *misattribution* guard: a stale hook config
in a nested or foreign session, or a pane id recycled across restarts, reports
into the void instead of into the wrong pane's status.

**Policy lives server-side.** The emitter forwards every event it is installed
for, tagged with what it knows (event name, `session_id`, `transcript_path`,
`source`, subagent scope from `agent_id`, an emitter-side monotonic `seq` from
`SystemTime` nanos for parallel-hook ordering). It filters nothing but
malformed input. herdr's scripts embed filtering policy (drop `SubagentStop`,
drop subagent-scoped events) in installed assets, so changing policy means
reinstalling hooks on every machine; amx's equivalent rules live in the fusion
machine (V04), property-tested, updated by shipping a binary.

### D-M2-5 — Events reach clients as control-channel notifications

There is **no server→client event path in the tree today**: the JSON-RPC
notification type exists (`amx-proto/src/rpc.rs:69-83`), the server never
sends one, and the client explicitly drops any it receives
(`amx-client/src/net.rs:206-210`). The client polls `session.state` after
every layout-mutating call (`app/wired.rs:200-202`) and on picker open, and
`app/wired.rs:355-357` carries an apology comment for the invalidation event
it "cannot yet hear". Everything M2 promises — `⚑N` without polling,
`amx events --json`, external notifiers — needs this path, so M2 builds it
(V11) rather than treating it as ambient (R-M2-4 flags the size).

Shape: an `events.subscribe` row; the reply carries the bus sequence at
subscribe time (the 04 §2 contract verbatim), after which the connection's
writer emits one JSON-RPC notification (method `"event"`) per
`Delivery` — envelope or `gap{from,to}` — on the control channel, which
already outranks grid and bulk traffic in the writer's strict priority. A
consumer that sees `gap` re-queries state (`session.state` carries its capture
seq) and resumes; `amx events --json` prints deliveries as NDJSON and
documents exactly that resync contract; the client applies the same rule
(gap → one `sync_state`). Subscriptions die with the connection — cursors are
per-connection state, nothing to persist, nothing to leak.

### D-M2-6 — Fusion: the typed tracker, precedence, and provisional constants

Per-pane status is an explicit state machine in `agent/fusion.rs` (V04): a
`Tracker` holding identity, `AgentState` (`Idle | Working | Blocked` for
identified agents; `Busy | Quiet` activity for unknown foreground programs —
tier 3 never fakes `blocked`, 04 §5), the provenance of the current state
(hook-asserted at instant T vs screen-confirmed N times), and pending
deadlines. Inputs are data (`HookEdge`, `ScreenVerdict`, `Deadline`,
`PaneExited`), outputs are data (new state + effects: publish event,
enqueue/dequeue attention). No I/O in the module, so property tests drive it
with arbitrary interleavings.

Precedence, per 04 §5, parameterized by coverage class:

- **Entry edges apply instantly** (class `edges`/`full`):
  `UserPromptSubmit`/`PreToolUse`/`PostToolUse` → `Working`;
  `PermissionRequest` → `Blocked`; `Stop` → `Idle`; `PermissionDenied` →
  `Working` (provisional — V01's M2 measures what actually follows a manual
  deny).
- **Exits from `Working`/`Blocked` are confirmed, never trusted**: cleared by
  a matching hook edge, or by tier-2 contradicting the held state for
  `CONFIRMATIONS` consecutive evaluations, or by the staleness deadline —
  whichever is first. Esc-interrupts and dialog cancels, invisible to hooks,
  land through the middle clause; a manifest lagging a UI redesign lands
  through the last.
- **Subagent-scoped events never touch the parent's state** — not on entry,
  not on exit. herdr's "never revive an idle pane" is a rule of the machine
  and a property test, not a script filter.
- **Screen-asserted idle honors the confirmation window** unless the matching
  rule is flagged `visible_idle` (the prompt box is actually on screen), which
  bypasses the hold — herdr's flicker fix
  (`herdr/src/pane/agent_detection.rs:23-76`), kept as data.

Provisional constants, inherited from herdr's production values and adjusted
by V01's measured hook latencies before V04 freezes them: confirmation window
3 consecutive screen verdicts at ≥100 ms spacing, capped at 700 ms; identity
startup grace 3 s (a booting TUI's splash matches nonsense); staleness
deadline 30 s for a hook-held state with no screen coverage. herdr has *no*
staleness expiry at all — state persists until contradicted — and 04 §5
mandates one; it is new ground and its property test pins it (R-M2-11).

### D-M2-7 — Resume: registry templates, snapshot fields at v1, type-in launch

herdr's rigor is kept wholesale (K5), with the tables moved into the registry:

- **Refs are shape-validated data**: `{kind: id|path, value}`, non-empty,
  length-capped (512/4096), no control characters, `path` must be absolute,
  `path` only for agents whose stanza says so. Constructors return `Option`;
  there is no other way to build one.
- **Sources are allowlisted**: a ref is only accepted from the hook path of
  the agent it claims (`source == "amx:<agent-id>"` cross-checked against the
  stanza), checked at report time, at snapshot-read time, and again at plan
  time — a hand-edited `session.json` cannot inject an argv (herdr checks all
  three gates too, `herdr/src/agent_resume.rs:217-237`).
- **argv is data**: `plan()` substitutes the ref into the stanza's template's
  single `{ref}` slot and returns `Vec<String>`; nothing is ever interpolated
  into a shell string.
- **Dedupe reservations with rollback**: restore reserves
  `source\0agent\0kind\0value` before spawning; a second pane claiming the
  same conversation restores as a plain shell; a failed spawn releases the
  reservation so a later pane can claim it.
- **Persistence**: `PaneSnapshot` gains `argv: Option<Vec<String>>` and
  `agent: Option<AgentSnapshot>` (`kind`, `name`, `ref`, `source`,
  `start_source`), both `#[serde(default, skip_serializing_if)]` — additive
  optional fields under the unknown-field contract, so **`VERSION` stays 1**
  and the read window stays `{1}`. This is exactly the precedent R-M1-8
  recorded and the field addition `persist/mod.rs:183-187` pre-announced
  ("argv-as-data is M2"). The persist golden regenerates; no version-window
  machinery moves.
- **Launch is type-in, not exec**: restore spawns the pane's saved shell,
  waits (bounded, condition-not-sleep) for the shell's first damage — the
  prompt painting is the readiness signal — then injects the planned argv
  through the same bracketed-paste-aware submit path `pane run` uses. The pane
  remains a shell when the agent exits (matching M1's respawn model — a pane
  is its shell, `PaneExited` on agent exit would otherwise close it), and the
  user sees what was run in their history. herdr types into the shell for the
  same reasons (`herdr/src/app/agent_resume.rs:204-284`).

### D-M2-8 — The attention queue is AgentHub state, exposed as state

`AgentHub` owns the ordered queue (enqueue on entering `Blocked`, by
transition time; dequeue on leaving it or on pane exit; re-block re-enqueues
at the tail). Exposure follows the restore-report precedent exactly
(`⚠N` in `app/status.rs:79-86`, fed by `StateReply.restore`):

- `session.state` gains an `attention: Vec<PaneId>` in queue order (and per-
  pane agent status on `PaneState`) — snapshot queries and the client model
  read the same truth.
- `attention_enqueued`/`attention_dequeued` events ride the bus and, through
  V11, reach external notifiers — the reference notifier consumes the
  identical queue the status line renders, which is the roadmap's requirement.
- `agent.next` (one method row) returns the head and focuses it
  (workspace switch + pane focus via Core); the client's `next-attention`
  prefix key is one call. No separate "query the queue" method exists —
  `session.state` is the query.
- The client emits OSC 9/99 to its host terminal on an enqueue notification —
  the one built-in notify path (03 §4), written into the existing `App.emit`
  buffer that OSC 52 already flushes (`app/wired.rs:147-150`); zero new
  plumbing.

### D-M2-9 — The agent's name is the pane's label

04 §5 addresses agents "by user-assigned name or pane UUID". amx already has a
persisted, renameable, user-visible per-pane name: the label (M1). Inventing a
second namespace would mean a second rename verb, a second persistence field,
and a picker that shows two names. So: `agent start dev` sets the pane label
to `dev`; every agent verb resolves its target as pane-UUID-or-label, where a
label match must be unique among *agent* panes (ambiguity is an error naming
the candidates); `agent rename` is `pane.rename` reached through the agent
verb's resolver — an alias, not a method row (R-M2-10 flags the reading of 04's
verb list). Five identical Claude panes are orchestratable because five labels
name them, and the labels survive restart because M1 already persists them.

---

## 2. The hook-coverage spike (V01)

The spike is an experiment against the installed tooling, producing
`docs/notes/hook-coverage.md` — a transition-by-event matrix with the raw
recordings that back it. Nothing in it is product code; its harness lives in
`scripts/spike/` so the measurement is repeatable after every agent update
(these tools ship weekly; the matrix will rot and must be cheap to refresh).

**Method.** A scratch project whose `.claude/settings.json` subscribes every
lifecycle event to one logging command that appends a line per invocation —
timestamp, event name, the full stdin payload, and the values of planted
`AMX_*` environment variables — to a log file. Drive `claude` in a real PTY
through scripted scenarios, then read the log against the scenario script.
Where a scenario needs a permission dialog, use a tool call that is not
pre-approved in the scratch project. Codex: attempt an install first
(`npm i -g @openai/codex` or platform equivalent); if the machine cannot run
it, that is a *recorded finding* — its stanza ships `coverage = "identity"`
with a comment naming the missing measurement, not a guess (R-M2-1).

**The questions.** Silences inventoried from the docs during planning, each
now an experiment:

| # | Question | Why it gates |
|---|---|---|
| M1 | Does `Stop` fire on Esc-interrupt — during generation, and during a tool call? Does anything else? | The single transition that broke herdr; decides whether interrupt-exit is a hook edge or screen-only |
| M2 | Manual permission deny/cancel: does `PermissionDenied` fire on a human "no"? On Esc from the dialog? What follows — `Stop`? nothing? | Decides the `Blocked` exit edges in V04's table |
| M3 | Does `PermissionRequest` fire exactly when the dialog paints? Payload enough to say *what* is being asked? | The `Blocked` entry edge's reliability class |
| M4 | `SubagentStop`/`SubagentStart`: ordering relative to the parent's `Stop`; is `agent_id` reliably present on subagent events and reliably absent on parent events? | The "never revive an idle pane" discriminator |
| M5 | `Notification`: when does it fire, and does its payload distinguish permission-wait from idle-wait? | Possible corroborating edge; herdr never had it |
| M6 | Do hook processes inherit the launching terminal's environment (`AMX_*` planted before `claude` starts)? | The entire identity scheme (D-M2-4); docs silent |
| M7 | Do project-scope hooks in `.claude/settings.json` run without an interactive trust prompt, freshly written? | Whether `integration install` is actually non-interactive |
| M8 | `session_id`: format, stability across `--resume`/`/clear`/compact; observed `SessionStart.source` values | Ref validation shape + which start sources amx accepts (D-M2-7) |
| M9 | Hook dispatch latency, event → process spawn (distribution, not anecdote) | Calibrates "instant" entry edges and V04's confirmation spacing |
| M10 | Codex: install viability; `[features] codex_hooks`; whether the trust-hash gate blocks a non-interactively installed hook until a human approves; which events fire; payloads; `codex resume` behavior | Codex's class, and V10's installer honesty |

**Deliverables.** The matrix; a proposed coverage class per agent with the
evidence line for each; recommended fusion constants (from M9's latencies);
the concrete edge-case list the exit test must replay (each M1/M2 finding
becomes a scripted scenario in V17); and the leftovers — every question the
machine could not answer, stated as such.

**Gating and fallbacks.** V03 consumes the classes as stanza data; V04
consumes the precedence outcomes and constants; V10 consumes the per-agent
`hook_events` list and the Codex trust finding; V17 consumes the edge list.
The outcome branches are all data:

- *Worse than hoped* (e.g. `PermissionRequest` unreliable, or env inheritance
  fails): the affected edges leave the hook column and `Blocked` detection
  falls to tier-2 alone — which is precisely what herdr ships in production
  today, so the floor is a working, known-quality system. If env inheritance
  fails, hooks still deliver session identity keyed by `session_id` cross-
  checked against the process tree (herdr's takeover check), and resume
  survives; only zero-latency edges are lost. Claude's stanza degrades toward
  `identity`; V04's table shrinks; no task is removed, V04/V08 simply carry
  fewer hook arms.
- *As documented*: `edges` for Claude Code, constants roughly as provisional.
- *Better than hoped* (e.g. Esc-interrupt does emit `Stop`): the interrupt
  edge moves into the hook column, confirmation windows shrink, and the exit
  test still keeps the screen-only scenario — the fusion machine must handle
  hookless interrupts regardless, because Codex-class agents and manifest lag
  both produce them.

The fusion design is **final only when** M1, M2, M4, and M6 have measured
answers (or measured "no signal", which is itself an answer — it means the
transition is screen-owned). V04 does not merge before that.

---

## 3. The AgentHub actor

Fifth of 04 §2's five actors, assembled in `session/serve.rs` on the Persist
template (bounded mailbox, handle held by Core, bus subscription taken at
assembly, spawned under the one `Runtime`):

- **Owns**: the parsed registry; per-pane `Tracker`s (V04's machine) with
  their `SnapshotFeed`s; the manifest engine and compiled rules; the attention
  queue; the session-ref table; per-pane hook tokens; the deadline wheel.
- **Mailbox** (`AgentCommand`, bounded at 64): `HookReport{pane, token,
  report}` from dispatch; `PaneStarted{pane, frames: SnapshotFeed, spawn:
  SpawnedIdentity}` and `PaneClosed{pane}` from Core (Core owns `PaneHost`s
  and is the only place a feed can be cloned from — the hub never asks for
  one, it is handed one, `try_send`, fire-and-forget); `Explain{pane, reply}`;
  `NextAttention{reply}`.
- **Bus subscription**: `PaneDamage` schedules a coalesced tier-2 evaluation
  for tracked panes (per-pane minimum spacing ~100 ms — an evaluation reads
  `frame()`, extracts regions via V05's text view, runs the compiled rules);
  `PaneExited` retires the tracker and dequeues; `PaneRenamed` updates name
  addressing; `ConfigReloaded` re-scans the manifest override directory
  (local hot reload rides the existing config watcher; a remote catalog is
  M4's, R-M2-13).
- **Publishes** `agent_status`, `agent_identified`, `attention_enqueued`,
  `attention_dequeued` — the hub is the *only* publisher of agent events, per
  04 §2's one-publisher rule (which pane events currently violate; R-M2-3).
- **Timers**: one deadline wheel (`tokio::time::sleep_until` to the nearest
  pending confirmation/staleness/grace deadline), armed only while a deadline
  exists. No idle tick, ever — a session of idle agents costs zero wakeups
  (03 §5), unlike herdr's permanent 300–500 ms scan loop.

**Two read models, and the ordering that keeps waits honest.** Wait predicates
must read live state (`wait.rs:16-18`: an event-history predicate "will hang
the first time its transition lands inside a gap"), and dispatch runs on
connection tasks, not inside the hub. So the hub maintains a shared
`StatusView` (`Arc<RwLock<HashMap<PaneId, AgentSnapshot>>>` — status, last-
transition seq, identity, queue position), and the write protocol is fixed:
**update `StatusView`, then publish the event.** A waiter woken by the event
therefore always observes a view at least as new as the event; the reverse
order can hang a wait forever (wake → read stale view → false → no further
event ever comes). Chrome and persistence take the second, slower path: the
hub `try_send`s a status/ref summary to Core, which folds it into pane state —
that mirror feeds `session.state`, the status line, and the final capture, and
its mailbox lag is harmless because nothing awaits on it. The two consumers
are named in the code comment so nobody "simplifies" them into one.

**Shutdown** is Persist's discipline under the standing wedge-flake constraint
(R-M1-2, still undiagnosed): on cancellation the hub drains its own mailbox to
closure, drops the deadline wheel, publishes nothing, writes nothing, and
**sends no request to any sibling**. There is nothing to flush: refs and
status already live in Core's mirror via fire-and-forget sends made during
normal operation, so Core's final capture carries them without asking the hub
anything. Detection state is derived and dies with the process.

---

## 4. Wire and event surface

Twelve method rows (all V02, table + params/reply types + seam-stubbed
handlers + goldens + skew arms; wave tasks replace the stubs):

| Wire | Params (essentials) | Reply | Filled by |
|---|---|---|---|
| `agent.report` | pane, token, agent, source, event, seq, session ref fields, subagent scope | ack | V09 |
| `agent.start` | name, kind, extra argv, cwd, timeout | pane id + readiness outcome | V13 |
| `agent.prompt` | target, text, wait: none\|blocked\|idle, timeout | status at completion | V13 |
| `agent.explain` | target | manifest source/version, matched rule, per-rule evidence | V06 |
| `agent.next` | — | focused pane (or empty-queue) | V08 |
| `wait` | until: blocked\|idle\|exited, target, timeout | terminal status/seq | V11 |
| `events.subscribe` | after_seq (optional) | seq at subscribe | V11 |
| `pane.send_text` | target, text | ack | V12 |
| `pane.send_keys` | target, keys (`ctrl+h`, `f1`, … grammar) | ack | V12 |
| `pane.run` | target, text (bracketed-paste + submit) | ack | V12 |
| `pane.read` | target, lines (optional) | text rows | V12 |
| `pane.wait_output` | target, match\|regex, timeout | matched line + seq | V11 |

`wait` and `pane.wait_output` are long-poll calls: the connection task builds a
`Waiter` whose predicate reads `StatusView` (agent statuses), Core state (pane
existence, for `exited`), or the pane's text view (`wait_output` — regex over
the visible grid at evaluation time, re-run per `PaneDamage`; the contract
documents that it matches the *screen*, not a byte stream — content that
scrolls through between damage batches is `pane.read`/history territory).
`Waiter` gains its first consumers; the gap-resync behavior it encodes is what
makes a wait across a busy bus safe, and the acceptance tests force the
overflow case exactly as T03's did.

Four event variants (V02): `agent_status{pane, from, to, cause}`,
`agent_identified{pane, kind}`, `attention_enqueued{pane}`,
`attention_dequeued{pane}`. Each owes the full coverage set the goldens law
demands: a `tag()` arm (the build stops until it exists,
`event/mod.rs:194-203`), an `every_event()` fixture, a
`tests/goldens/event/<tag>.json`, and a catch-all-tolerant consumer. The
notification envelope for `events.subscribe` deliveries (including `gap`)
gets a proto golden beside the existing `notification` one. `session.state`
grows `attention` plus per-pane `agent` fields — additive, inside protocol v1
per the R-M1-8 precedent.

One new dependency: `regex` (manifest `regex`/`line_regex` gates and
`pane.wait_output`; compiled once per rule/call, never per evaluation). The
commit that adds it carries the one-line justification HACKING.md requires.

---

## 5. Task DAG

Difficulty is `hard` when the task carries measurement, concurrency, syscall,
wire-compatibility, or restore-correctness risk; `normal` otherwise. Every
task lands with tests that fail without the change, and finishes with
`cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
green.

---

### V01 — Hook-coverage spike

- **Difficulty:** hard · **Wave:** 0 · **Depends on:** —
- **Goal:** the measured answers of §2, written down with their recordings.
- **Scope (owns exclusively):** `docs/notes/hook-coverage.md`,
  `scripts/spike/**`.
- **Acceptance:** the findings doc answers M1–M10 with a recording or an
  explicit "unmeasurable here because …" per row; proposes a coverage class
  per agent with evidence; proposes fusion constants from measured latency;
  lists the exit-test edge scenarios; `scripts/spike/` re-runs the Claude
  matrix unattended against a scratch project.
- **Prompt draft:** Run the M2 hook-coverage spike exactly as
  `docs/08-m2-plan.md` §2 lays it out. Build a tiny logging harness in
  `scripts/spike/` — a scratch project whose `.claude/settings.json`
  subscribes every lifecycle event to a command appending timestamp, event
  name, full stdin payload, and planted `AMX_*` env values to a log — and
  drive the installed `claude` (2.1.224) through scripted scenarios in a real
  PTY: a clean turn, Esc during generation, Esc during a tool call, a
  permission dialog answered no, a permission dialog dismissed with Esc, a
  subagent turn, `/clear`, and a `--resume`. Then attempt a Codex install and
  repeat what its trust gate allows. Write `docs/notes/hook-coverage.md` as a
  transition-by-event matrix answering M1–M10 from §2, each cell backed by a
  recording; where the machine cannot produce an answer, say so in the row
  rather than inferring one. Close with proposed coverage classes, fusion
  constants, and the edge-case list the M2 exit test must replay. Never guess:
  a docs claim you did not reproduce is cited as a docs claim, not a finding.

---

### V02 — M2 contracts: types, rows, splits, stubs

- **Difficulty:** hard · **Wave:** 1 · **Depends on:** V01 (class/edge
  vocabulary only — type shapes are outcome-independent)
- **Goal:** every shared surface later waves implement against, frozen; every
  budget-edge file split before the waves press it.
- **Scope:** `crates/amx-core/src/agent.rs` (new: `AgentState`,
  `AgentSnapshot`, `CoverageClass`, ref types), `amx-core/src/event/mod.rs`
  (+4 variants + tags), `amx-core/tests/{event_goldens.rs,contracts.rs}`,
  `tests/goldens/event/**`; `amx-proto/src/control/{mod.rs,table.rs (new —
  macro definition moves out),agent.rs (new),wait.rs (new),pane.rs,session.rs}`,
  `amx-proto/tests/goldens.rs`, `tests/goldens/proto/**`;
  `amx-server/src/actor/mod.rs` (`AgentCommand`, `AgentHandle`, `StatusView`),
  `actor/core/{mod.rs,route.rs (new — absorb match moves out),pane.rs,spawn.rs
  (new — spawn helpers move out)}`, `src/agent/mod.rs` (module skeleton),
  `dispatch/{mod.rs,agent.rs (new),wait.rs (new),pane.rs}` seam stubs,
  `persist/mod.rs` (+2 optional fields), `tests/goldens/persist/**`;
  `tests/skew.rs` (+12 `sample_params` arms), `tests/hygiene.rs` (seam helper
  exemption returns), `crates/amx/src/cli.rs` (routing arms for `events`,
  `_hook`, `integration`, `skill` — planted here so no two wave tasks touch
  `cli.rs`, the U01 precedent).
- **Acceptance:**
  - `the_m2_event_variants_round_trip_and_tag_themselves`
  - `event_goldens_cover_agent_status_and_attention_variants`
  - `method_goldens_cover_all_twelve_m2_rows`
  - `skew_calls_every_m2_row_and_none_is_method_not_found`
  - `pane_snapshot_with_agent_fields_reads_at_version_1`
  - `status_view_orders_write_before_event_publish` (doc-tested contract on
    the type)
  - module-size check green with `core/mod.rs`, `core/pane.rs` back under 450
- **Prompt draft:** Land M2's shared contracts exactly as
  `docs/08-m2-plan.md` §3–§4 define them, the way T01/U01 did for M0/M1:
  compiling type definitions and seam-stubbed handlers, no behavior. First the
  splits R-M2-5 demands — `core/mod.rs`'s absorb match to `core/route.rs`,
  `core/pane.rs`'s spawn helpers to `core/spawn.rs`, the `method_table!`
  definition to `control/table.rs` — because both core files sit at 499–500 of
  a 500 soft budget before you add a line. Then the twelve method rows with
  their payload types, the four event variants with `tag()` arms and goldens,
  `AgentCommand`/`AgentHandle`/`StatusView`, `amx-core/src/agent.rs`, the two
  additive `PaneSnapshot` fields (version stays 1 — cite R-M1-8), the skew
  arms, and the returned `seam(` helper with its hygiene exemption. Regenerate
  every golden the coverage law demands and leave each stub failing with
  `NOT_IMPLEMENTED`, never a panic. Doc comments quote the normative sentences
  from 04 §5 and this plan's §3 — later agents implement from these signatures
  in parallel, so precision beats brevity everywhere.

---

### V03 — The agent registry

- **Difficulty:** normal · **Wave:** 2 · **Depends on:** V01 (class values),
  V02
- **Goal:** one embedded `agents.toml`, parsed once, override-merged, with the
  conformance test that makes lying stanzas fail the build.
- **Scope:** `crates/amx-server/src/agent/registry.rs`,
  `crates/amx-server/assets/agents.toml`,
  `crates/amx-server/tests/registry.rs`.
- **Acceptance:**
  - `registry_parses_embedded_stanzas_for_claude_and_codex`
  - `alias_and_id_lookup_resolve_to_one_stanza`
  - `override_file_adds_an_agent_without_touching_builtins`
  - `conformance_every_stanza_names_a_present_compiling_manifest`
  - `conformance_resume_templates_carry_exactly_one_ref_slot`
  - `conformance_coverage_classes_match_spike_findings` (asserts the shipped
    values equal `docs/notes/hook-coverage.md`'s table — the doc is the
    fixture)
  - `malformed_override_is_rejected_with_the_builtin_kept`
- **Prompt draft:** Build amx's agent registry per D-M2-2 in
  `docs/08-m2-plan.md`: an `agents.toml` embedded with `include_str!`, parsed
  once at startup into a `Registry`, merged with an optional
  `$XDG_CONFIG_HOME/amx/agents.toml` override where override stanzas add or
  replace whole agents (never field-merge — partial stanzas are rejected with
  the builtin kept, matching M1's per-section lenient config rule). Ship
  `claude` and `codex` stanzas whose coverage classes are copied from
  `docs/notes/hook-coverage.md` — the conformance test literally reads that
  doc's table so the registry cannot drift from the measurement. The
  conformance test is the heart of the task: walk every stanza and assert
  every derived surface agrees, as §D-M2-2 enumerates. Study herdr's
  hand-synced fan-out (W6 in `docs/02-herdr-critique.md`) as the anti-pattern:
  if you find yourself writing an agent's name anywhere but its stanza, stop.

---

### V04 — The fusion state machine

- **Difficulty:** hard · **Wave:** 3 · **Depends on:** V01 (final gate — does
  not merge before M1/M2/M4/M6 have measured answers), V02, V03
- **Goal:** the typed, property-tested tracker of D-M2-6 — pure logic, no I/O.
- **Scope:** `crates/amx-server/src/agent/fusion.rs`,
  `crates/amx-server/tests/fusion.rs`.
- **Acceptance:**
  - `hook_entry_edges_apply_instantly_for_edges_class`
  - `esc_interrupt_with_no_hook_event_settles_idle_via_screen_confirmations`
  - `dialog_cancel_with_no_hook_event_unblocks_within_the_confirmation_cap`
  - `subagent_events_never_change_parent_state` (proptest over interleavings)
  - `visible_idle_bypasses_the_confirmation_hold`
  - `staleness_deadline_clears_a_hook_held_state_with_no_screen_coverage`
  - `identity_class_ignores_hook_state_and_full_class_trusts_it`
  - proptest `no_input_sequence_leaves_a_tracker_with_a_stale_deadline`
  - proptest `every_transition_emits_exactly_one_status_effect`
- **Prompt draft:** Implement the fusion state machine of
  `docs/08-m2-plan.md` D-M2-6 as pure data-in/data-out logic in
  `agent/fusion.rs`: `Tracker` × (`HookEdge` | `ScreenVerdict` | `Deadline` |
  `PaneExited`) → new state + effects, parameterized by the registry's
  coverage class. Take the precedence table and constants from
  `docs/notes/hook-coverage.md` — the spike's findings, not this plan's
  provisional numbers, are the authority, and if the two disagree the plan
  loses. The herdr mechanisms worth studying (never copying) are the
  pending-idle confirmation hold with its `visible_idle` bypass and the
  subagent rule (`docs/02-herdr-critique.md` W5 tells the history). Property
  tests are the deliverable as much as the machine: arbitrary interleavings of
  hook edges, screen verdicts, and deadline fires must never revive an idle
  pane from a subagent event, never wedge a tracker with an unfired deadline,
  and never emit two status effects for one transition. No tokio, no clocks —
  deadlines arrive as inputs, which is what makes the tests exhaustive.

---

### V05 — Snapshot text view

- **Difficulty:** normal · **Wave:** 2 · **Depends on:** V02
- **Goal:** the row/tail text surface tier-2 and `pane.read` consume.
- **Scope:** `crates/amx-vt/src/snapshot.rs`,
  `crates/amx-vt/tests/text_view.rs`.
- **Acceptance:**
  - `row_line_concatenates_cells_including_wide_and_combining`
  - `tail_returns_the_last_n_rows_top_to_bottom`
  - `row_line_borrows_and_does_not_allocate` (the backing `text: Vec<u8>` is
    served as `&str`, verified UTF-8 at publish)
  - `text_view_of_a_published_snapshot_is_stable_while_parser_writes` (reader
    holds the `Arc` across heavy output)
- **Prompt draft:** Add the text view to `amx-vt`'s snapshot per
  `docs/08-m2-plan.md` D-M2-3: `Row::line() -> &str` and
  `Snapshot::tail(n) -> impl Iterator<Item = &Row>`, inside `snapshot.rs`
  because `Row`'s contiguous `text` backing and `cells` are private to it —
  building this anywhere else means a per-cell `TextRef` walk and per-call
  allocation, which the detection path (called per damage batch) must not
  pay. The snapshot is already double-buffered and published as an `Arc`, so
  the only new invariant is that `line()` borrows; prove non-allocation the
  way `snapshot_does_not_allocate_after_the_first_frame` already does. Wide
  cells, combining marks, and trailing blanks are the edge cases — match the
  cell semantics the renderer uses, and document that the view is the visible
  grid, which in amx is by construction the live bottom (scrollback never
  enters the server's snapshot).

---

### V06 — Tier-2 manifest engine

- **Difficulty:** normal · **Wave:** 2 · **Depends on:** V02
- **Goal:** the rule grammar, compiled evaluation, and `agent.explain`, as an
  engine over fixture text (wired to live panes by V08).
- **Scope:** `crates/amx-server/src/agent/manifest/{mod,rule,region,compile,
  explain}.rs`, `crates/amx-server/assets/manifests/{claude,codex}.toml`,
  `crates/amx-server/tests/manifest.rs`, `dispatch/agent.rs` explain arm.
- **Acceptance:**
  - `rules_parse_with_priority_region_gates_and_skip_state_update`
  - `gate_semantics_and_of_contains_regex_line_regex_all_or_of_any_not`
  - `highest_priority_wins_ties_break_by_file_order`
  - `regions_slice_whole_bottom_lines_bottom_non_empty_and_title`
  - `skip_state_update_holds_the_previous_state_and_names_the_rule`
  - `regexes_compile_once_at_load_never_per_evaluation`
  - `explain_reports_every_rule_with_match_evidence_and_region_preview`
  - `claude_manifest_matches_captured_idle_working_and_blocked_screens`
    (fixtures recorded from the real UI during the spike)
- **Prompt draft:** Build amx's tier-2 manifest engine per
  `docs/08-m2-plan.md` D-M2-3, porting herdr's *grammar* — study
  `herdr/src/detect/manifest.rs` for rule fields, gate semantics (AND of
  `contains`/`regex`/`line_regex`-any-line, nested `all`/`any`/`not`),
  priority arbitration with file-order ties, `skip_state_update` as the
  assert-nothing third outcome, and the complexity caps — and write amx's own
  engine; never copy lines. amx's region vocabulary starts minimal:
  `whole_recent`, `bottom_lines(N)`, `bottom_non_empty_lines(N)`, `title`
  (fed from pane title state, herdr's `osc_title` analog); herdr's structural
  prompt-marker regions arrive only if the shipped manifests need them.
  Compile every regex at load and store beside the parsed rule. Write the
  `claude` and `codex` manifests against screen fixtures captured during the
  spike, biasing state rules to bottom-anchored regions — herdr's changelog
  documents `whole_recent` matching stale "esc to interrupt" scrollback as a
  real production bug. `agent.explain`'s reply reports every rule's verdict
  with evidence, because a detection you cannot interrogate is a detection
  you cannot fix.

---

### V07 — Spawn identity: argv, env injection, process-tree extension

- **Difficulty:** hard · **Wave:** 3 · **Depends on:** V02
- **Goal:** everything a pane knows about its child and a child knows about
  amx: recorded spawn argv, injected `AMX_*` env, and the argv-reading
  process-tree walk with wrapper unwrapping.
- **Scope:** `crates/amx-core/src/platform.rs` (trait: `argv`, `exe`),
  `crates/amx-server/src/platform/process.rs` (Linux `/proc/<pid>/cmdline`,
  darwin `KERN_PROCARGS2`, fallback), `crates/amx-server/src/agent/identity.rs`
  (foreground-job walk + wrapper unwrapping), `actor/core/spawn.rs` (argv
  recording + env injection + token mint — sequential edit of V02's file),
  `amx-core/src/state/pane.rs` argv field, `crates/amx-server/tests/identity.rs`.
- **Acceptance:**
  - `spawned_pane_records_its_argv_in_state`
  - `pane_child_env_carries_amx_ids_socket_and_token`
  - `process_tree_argv_reads_the_foreground_job_on_linux_and_darwin`
  - `wrapper_unwrapping_finds_the_agent_under_node_python_and_sh_dash_c_path`
  - `eval_flag_arguments_never_identify_an_agent` (`python -c "codex"` is not
    codex — herdr's test, re-derived)
  - `identity_prefers_the_group_leader_then_unwrapped_evidence`
  - `unknown_foreground_program_reports_busy_or_quiet_never_blocked`
- **Prompt draft:** Give amx spawn-side and probe-side identity per
  `docs/08-m2-plan.md` D-M2-4 and 04 §5's tier 3. Spawn side: record the
  argv `pane.split` passes into pane state (the field `persist/mod.rs:183-187`
  reserved), and inject `AMX_ENV`, `AMX_SESSION`, `AMX_SOCKET`, `AMX_PANE_ID`,
  `AMX_WORKSPACE_ID`, `AMX_HOOK_TOKEN` into every pane child via
  `core/spawn.rs` — the env travels on the `PtyCommand`, never the process
  env. Probe side: extend `ProcessTree` with `argv`/`exe` (Linux
  `/proc/<pid>/cmdline`; darwin `sysctl KERN_PROCARGS2`, which libproc does
  not wrap — budget real time for its layout, and keep the non-Unix fallback
  honest), then build the identification walk in `agent/identity.rs`:
  foreground group leader first, then job members scored by unwrapped
  evidence, unwrapping `node`/`bun`/`python`/`sh -c`/path tokens the way
  `herdr/src/detect/mod.rs:326-364` does — study the mechanism and its
  negative tests (eval-flag arguments must never identify) and write your
  own. Probes are triggered, rate-limited work — spawn, damage-after-quiet,
  and `agent start` readiness — never a free-running scan loop.

---

### V08 — The AgentHub actor

- **Difficulty:** hard · **Wave:** 4 · **Depends on:** V03, V04, V05, V06, V07
- **Goal:** §3 made real: the assembled hub — mailbox, bus wiring, detection
  scheduling, StatusView ordering, attention queue, deadline wheel, shutdown.
- **Scope:** `crates/amx-server/src/actor/agent_hub/{mod,run,detect}.rs`,
  `session/serve.rs` (assembly), `actor/mod.rs` handle bodies (sequential edit
  of V02's file), `dispatch/agent.rs` `agent.next` arm,
  `crates/amx-server/tests/agent_hub.rs`.
- **Acceptance:**
  - `hook_report_with_valid_token_updates_status_and_publishes_once`
  - `token_mismatch_is_dropped_and_counted`
  - `damage_driven_detection_coalesces_to_the_minimum_spacing`
  - `status_view_is_current_before_the_status_event_is_receivable` (the wait-
    hang ordering, tested with a subscriber racing the view)
  - `blocked_agents_enqueue_in_block_order_and_dequeue_on_unblock_and_exit`
  - `agent_next_focuses_the_head_and_reports_empty_honestly`
  - `idle_session_arms_no_timer` (instrumented: no wakeups without deadlines)
  - `shutdown_after_cancel_sends_no_sibling_request` (the R-M1-2 discipline,
    asserted like Persist's)
- **Prompt draft:** Build the `AgentHub` actor exactly as
  `docs/08-m2-plan.md` §3 specifies, assembling V04's tracker, V06's engine,
  and V07's identity behind one mailbox on the Persist template — read
  `session/serve.rs:114-122` and `actor/persist/actor.rs` for the assembly
  and shutdown pattern before writing anything. The two properties that are
  the point: the StatusView write-before-publish ordering (the doc explains
  the wait that hangs if you reverse it — encode the explanation as a test),
  and the shutdown discipline under the undiagnosed drain wedge (R-M1-2):
  after cancellation, drain your own mailbox and return — no timer, no
  publish, no request to Core or anyone. Detection is scheduled by
  `PaneDamage` with per-pane coalescing and executed against the
  `SnapshotFeed` Core hands you at `PaneStarted`; you never ask for a feed.
  Mirror status and refs to Core with `try_send` and let Core own what
  clients and captures see.

---

### V09 — `amx _hook` and report ingestion

- **Difficulty:** normal · **Wave:** 4 · **Depends on:** V02, V07
- **Goal:** the emitter subcommand and the `agent.report` path from stdin to
  hub mailbox.
- **Scope:** `crates/amx/src/cmd/hook.rs`, `dispatch/agent.rs` report arm
  (sequential edit of V02's stub), `crates/amx/tests/hook.rs`.
- **Acceptance:**
  - `hook_reads_claude_payload_and_sends_one_agent_report`
  - `hook_exits_zero_and_fast_with_no_socket_no_env_or_dead_server`
  - `hook_never_writes_to_stdout_or_stderr_on_failure` (agents surface hook
    noise to the user)
  - `subagent_scope_is_tagged_from_agent_id_not_filtered`
  - `report_reaches_the_hub_with_pane_and_token_intact` (over the real socket)
- **Prompt draft:** Implement `amx _hook` per `docs/08-m2-plan.md` D-M2-4: a
  hidden subcommand that reads one hook payload from stdin, resolves the
  session socket and pane identity from `AMX_*` env, and issues one
  `agent.report` control call — connect, Hello, request, exit — under a
  ~500 ms total budget, exiting 0 silently on every failure because a hook
  must never break an agent's turn. Forward, don't filter: tag subagent scope
  from `agent_id` and let the fusion machine own policy — herdr baked policy
  into installed scripts and paid for it with reinstalls
  (`docs/08-m2-plan.md` D-M2-4 tells the story). Fill V02's `agent.report`
  dispatch stub: decode, hand to the `AgentHandle`, ack. Test the miserable
  paths hardest — missing env, absent socket, server mid-shutdown — since
  silent-success is the contract, the tests must distinguish "silently
  succeeded" from "silently did nothing" via the hub's counters.

---

### V10 — Integration installers

- **Difficulty:** normal · **Wave:** 5 · **Depends on:** V01, V03, V09
- **Goal:** `amx integration install|uninstall|status` for Claude Code and
  Codex, with version markers that verify the thing that actually breaks.
- **Scope:** `crates/amx/src/cmd/integration.rs`,
  `crates/amx/src/integration/{mod,claude,codex,edit}.rs`,
  `crates/amx/tests/integration_install.rs`.
- **Acceptance:**
  - `install_writes_hook_entries_for_exactly_the_registry_event_list`
  - `install_preserves_foreign_hooks_and_reinstall_is_idempotent`
  - `uninstall_removes_only_amx_owned_entries`
  - `status_reports_current_outdated_and_not_installed_from_the_marker`
  - `status_fails_when_the_referenced_amx_binary_is_missing` (the check herdr
    lacks — its status greps a version comment while the hook no-ops)
  - `codex_install_sets_features_codex_hooks_and_prints_the_trust_caveat`
- **Prompt draft:** Build the integration lifecycle per `docs/08-m2-plan.md`
  D-M2-4 and V01's findings. Claude: edit the settings JSON to subscribe
  exactly the events the registry stanza lists, each entry running
  `amx _hook claude --marker <N>` — the marker in the command string is the
  version, and `status` verifies both the marker *and* that the referenced
  binary path exists and executes, because herdr's marker-only check reports
  `current` on an installation that silently does nothing. Preserve foreign
  hooks byte-for-byte; only amx-owned entries (matched by command shape) are
  ever touched, and reinstall over current is a no-op write. Codex: hooks
  file plus the `[features] codex_hooks = true` line-edit (the flag V01
  confirmed — not `hooks`, R-M2-2), and print the trust caveat honestly: Codex
  gates hooks on interactive hash-pinned approval, so install must tell the
  user their next `codex` run will ask, and `status` must say it cannot see
  trust state. Uninstall is symmetric and conservative; leave the features
  flag, remove only what amx wrote. `status` auto-run after self-update is
  M3's wiring; leave the function callable, not called.

---

### V11 — Waits and the event wire

- **Difficulty:** hard · **Wave:** 4 · **Depends on:** V02, V05
- **Goal:** `events.subscribe` end to end server-side, `wait`,
  `pane.wait_output`, and the `amx events --json` CLI with the documented
  gap-resync contract.
- **Scope:** `crates/amx-server/src/conn/events.rs` (new),
  `conn/{writer,reader,mod}.rs` edits, `dispatch/{wait,events}.rs` (fill V02
  stubs), `crates/amx/src/cmd/events.rs`, `crates/amx-server/tests/waits.rs`.
- **Acceptance:**
  - `subscribe_reply_carries_the_bus_seq_and_deliveries_follow_in_order`
  - `a_slow_subscriber_receives_gap_never_a_silent_drop_over_the_wire`
  - `wait_until_blocked_returns_the_instant_the_status_lands` (no poll
    interval anywhere — measured as sub-tick)
  - `wait_for_a_condition_already_true_returns_without_consuming_events`
  - `a_transition_inside_a_bus_gap_cannot_hang_a_wire_wait` (force the replay
    overflow between flip and resume, T03's discipline at the wire)
  - `wait_until_exited_completes_on_pane_exit_with_status`
  - `wait_output_matches_the_visible_screen_per_damage_batch`
  - `events_json_documents_and_survives_the_gap_resync_round_trip`
- **Prompt draft:** Build M2's event wire and waits per `docs/08-m2-plan.md`
  D-M2-5 and §4. `events.subscribe` registers a bus subscription on the
  connection; deliveries — envelope and `gap` alike — leave as JSON-RPC
  notifications on the control channel through the existing priority writer,
  and the subscribe reply carries the seq, which is what makes external
  resync possible (04 §2). `wait` and `pane.wait_output` are the first real
  consumers of `amx_core`'s `Waiter`: predicates read live state —
  `StatusView` for statuses, Core state for existence, the snapshot text view
  for output matching — never event history; `wait.rs:16-18` explains the
  hang you would otherwise build, and the acceptance test that overflows the
  replay buffer between flip and resume is the proof. `amx events --json`
  prints NDJSON deliveries and its help text states the resync contract.
  Timeouts are parameters, not sleeps; the hygiene suite will reject a nap.

---

### V12 — Pane driving

- **Difficulty:** normal · **Wave:** 3 · **Depends on:** V02, V05
- **Goal:** `pane.send_text|send_keys|run|read` — the primitives tests,
  `agent prompt`, and resume all ride.
- **Scope:** `dispatch/pane.rs` (fill V02 stubs),
  `actor/pane_host/{parser,mod,actor}.rs` (send-keys encoding and read
  commands on the parser thread), `crates/amx-server/tests/drive.rs`.
- **Acceptance:**
  - `send_text_delivers_bytes_to_the_child_verbatim`
  - `send_keys_grammar_encodes_ctrl_fn_and_kitty_aware_sequences`
  - `run_wraps_text_in_bracketed_paste_when_the_pane_enabled_it_and_submits`
  - `read_returns_the_visible_rows_as_text`
  - `driven_input_never_reorders_against_query_replies` (the response-order
    guarantee holds under `pane run` load)
- **Prompt draft:** Implement the pane-driving surface per
  `docs/08-m2-plan.md` §4, filling V02's dispatch stubs. `send_keys` parses
  the key-combo grammar (`ctrl+h`, `f1`, …) in the params type and encodes on
  the parser thread via `amx-vt`'s key encoder, because encoding depends on
  the pane's kitty-keyboard flags, which the parser owns — a new
  `ParserCommand`, serialized like `History`, which is also what keeps driven
  input ordered against query replies. `run` is the bracketed-paste-aware
  atomic text-plus-submit 04 §8 describes: bracket only when the application
  enabled paste mode, then submit. `read` serves V05's text view through the
  snapshot feed — no parser round trip. These four verbs are load-bearing for
  V13's `agent prompt`, V15's resume injection, and the whole M2 exit suite,
  so their tests drive a real child (the rig's `MarkerShell` pattern) and
  assert what the *child* received, not what the server sent.

---

### V13 — Agent verbs and addressing

- **Difficulty:** normal · **Wave:** 5 · **Depends on:** V08, V12
- **Goal:** `agent start` with the readiness handshake, `agent prompt
  [--wait]`, name-or-UUID addressing, and the CLI aliases.
- **Scope:** `dispatch/agent.rs` (sequential after V09),
  `crates/amx-server/src/agent/address.rs`,
  `crates/amx-server/tests/agent_verbs.rs`.
- **Acceptance:**
  - `agent_start_spawns_from_the_registry_labels_the_pane_and_returns_ready`
  - `readiness_is_identity_confirmed_plus_idle_observed_with_timeout`
  - `start_timeout_reports_failure_but_leaves_the_pane_running`
  - `addressing_resolves_uuid_then_unique_label_and_names_ambiguity`
  - `agent_prompt_submits_via_run_and_wait_blocked_returns_on_the_next_block`
  - `prompt_wait_uses_transition_seq_so_a_prior_blocked_state_cannot_satisfy_it`
- **Prompt draft:** Implement the agent verbs per `docs/08-m2-plan.md` D-M2-9
  and 04 §5. `agent.start` resolves the kind through the registry, spawns via
  Core's ordinary spawn path with the stanza's argv plus the caller's extras,
  sets the pane label to the requested name, and completes when the pane is
  *ready*: identity tier confirms the agent binary owns the foreground and a
  status of `Idle` has been observed (hook `SessionStart` or screen), bounded
  by the timeout — on expiry, report failure honestly and leave the pane
  alive for inspection, herdr's semantics. Addressing lives in
  `agent/address.rs`: UUID wins, then a label match that must be unique among
  agent panes, with ambiguity errors that name the candidates. `agent.prompt`
  rides V12's `run` and, with `--wait`, builds a Waiter whose predicate
  requires the target status *and* a transition seq later than the submit —
  the acceptance test that pre-blocks the pane and asserts the wait does not
  return early is the one that matters.

---

### V14 — Client: status, attention, notifications

- **Difficulty:** normal · **Wave:** 5 · **Depends on:** V02, V11
- **Goal:** the client hears events, renders `⚑N` and per-pane status, cycles
  attention on one key, and emits OSC 9/99.
- **Scope:** `crates/amx-client/src/{model.rs,app/status.rs,app/events.rs
  (new — notification handling, split from wired.rs),app/wired.rs,input/mod.rs,
  net.rs}`, client tests.
- **Acceptance:**
  - `client_subscribes_and_folds_status_events_without_polling`
  - `gap_notification_triggers_one_state_resync`
  - `status_line_renders_attention_count_and_updates_on_dequeue` (the
    `StatusLine` cache guard gains the fifth input — the render-once-and-
    freeze bug is the test)
  - `next_attention_key_calls_agent_next_and_focus_follows`
  - `enqueue_notification_emits_osc_9_into_the_host_terminal`
  - `unknown_event_tags_are_ignored_not_fatal` (the catch-all consumer rule)
- **Prompt draft:** Wire the client into M2's event stream per
  `docs/08-m2-plan.md` D-M2-5/D-M2-8. Subscribe after attach, consume
  notifications in a new `app/events.rs` (split the handling out of
  `wired.rs`, which is at 455 lines — R-M2-5), fold `agent_status` and
  attention events into the model beside `pane_labels` (that field's comment
  explains the pattern), and resync once on `gap`. Render `⚑N` from the
  mirrored queue exactly as `⚠N` renders from the restore summary
  (`app/status.rs:79-86`) — and extend the `StatusLine` equality guard with
  the new input, because the cached-refresh design renders once and freezes
  if you forget, which is the regression test to write first. The
  `next-attention` prefix key issues `agent.next` and lets `FocusChanged`
  move the client. On an enqueue notification, write OSC 9 (and OSC 99 where
  featured) into `App.emit` — the buffer OSC 52 already flushes; keep it a
  few dozen chrome-free lines, 03 §4's promise. Events from the future carry
  unknown tags: ignore them, the `#[non_exhaustive]` contract.

---

### V15 — Agent resume

- **Difficulty:** hard · **Wave:** 6 · **Depends on:** V08, V12, V03
- **Goal:** D-M2-7 end to end: refs captured to snapshot, planned from the
  registry, deduped with rollback, typed into restored shells.
- **Scope:** `crates/amx-server/src/agent/resume.rs`,
  `actor/core/restore.rs`, `actor/core/persist.rs` (capture fields),
  `crates/amx-server/tests/resume.rs`, `tests/goldens/persist/**` regen.
- **Acceptance:**
  - `hook_reported_refs_survive_into_the_snapshot_via_the_core_mirror`
  - `refs_are_shape_validated_and_source_allowlisted_at_all_three_gates`
  - `plan_substitutes_the_ref_into_exactly_the_template_slot`
  - `two_panes_claiming_one_conversation_resume_once_second_restores_a_shell`
  - `failed_spawn_releases_the_reservation_for_a_later_pane`
  - `resume_types_the_command_after_first_damage_and_the_child_receives_it`
  - `a_hand_edited_snapshot_ref_cannot_reach_an_argv` (control chars,
    relative paths, foreign sources — each rejected with the pane restored as
    a plain shell and the loss reported)
- **Prompt draft:** Implement agent resume per `docs/08-m2-plan.md` D-M2-7,
  keeping herdr's rigor with the tables in the registry. Capture: refs
  arrive in Core's pane mirror from AgentHub during normal operation and ride
  the existing capture path into `PaneSnapshot`'s new optional fields —
  version stays 1, regenerate the persist golden. Restore: validate shape
  and source allowlist *again* on read (a `session.json` is user-editable;
  the acceptance test hand-edits one and asserts nothing hostile reaches an
  argv), reserve the NUL-delimited dedupe key before any spawn, roll the
  reservation back on spawn failure, and plan argv by substituting into the
  stanza template — data end to end, quoting only at the injection boundary.
  Launch: spawn the saved shell, wait bounded-by-condition for its first
  damage, then inject via the `pane.run` machinery so the invocation lands in
  the user's shell history and the pane survives the agent's eventual exit.
  Report every degraded outcome through the restore report — a conversation
  that did not resume is a loss the user is told about, never a log line
  (04 §6).

---

### V16 — Skill and the reference notifier

- **Difficulty:** normal · **Wave:** 6 · **Depends on:** V11, V02
- **Goal:** `amx skill install` (in-binary asset, K10) and the ~20-line
  notifier the roadmap ships with M2.
- **Scope:** `crates/amx/src/cmd/skill.rs`, `crates/amx/assets/skill/**`,
  `examples/notify.sh`, `crates/amx/tests/skill.rs`.
- **Acceptance:**
  - `skill_install_writes_the_asset_and_is_idempotent`
  - `skill_content_names_only_verbs_that_exist_in_specs` (walked against
    `SPECS` so the skill cannot drift from the table)
  - `notifier_emits_one_desktop_notification_per_attention_enqueue` (driven
    against `amx events --json` with a stub notify command)
- **Prompt draft:** Ship the two extension artifacts per
  `docs/08-m2-plan.md`: the agent skill and the reference notifier. The
  skill is an in-binary asset `amx skill install` writes, teaching an agent
  to drive amx — spawn panes, `pane run`, `agent prompt`, `wait --until
  blocked`, read outputs — gated on `AMX_ENV=1` with the pane/workspace env
  vars V07 injects; test it by walking every verb the asset names against
  `SPECS`, so a renamed method breaks the build here instead of in an
  agent's hands. The notifier is `examples/notify.sh`: ~20 lines of POSIX sh
  consuming `amx events --json`, filtering `attention_enqueued`, and calling
  `notify-send`/`osascript` — it exists to prove the extension story (03 §4:
  out-of-terminal notification is an extension, and it must exist the day
  the many-agents workflow does), so keep it exemplary: no bashisms, honest
  gap handling (on `gap`, re-query and continue), comments telling a reader
  how to adapt it.

---

### V17 — Integration: the seams, and the M2 exit test

- **Difficulty:** hard · **Wave:** 7 · **Depends on:** V01–V16
- **Goal:** the wired product proven over the real binary, plus every
  cross-crate seam M2 created, named and owned.
- **Scope:** the seams by exception (may touch what integration requires):
  registry↔hub↔fusion↔manifest wiring gaps, hook emitter↔gateway↔hub over the
  real socket, event wire↔client model, resume↔persist↔restore; plus
  `tests/agents.rs` (new suite + `[[test]]` row in `tests/Cargo.toml`),
  fake-agent fixtures in `tests/support/`, and the live-smoke checklist
  appended to `docs/notes/hook-coverage.md`.
- **Why it exists:** M0 and M1 both proved the pattern — exclusive file
  ownership is what makes waves parallel, and it is also why nobody owns the
  places tasks meet (06 §T19, 07 §U10). M2's seams are wider than M1's: five
  subsystems meet in AgentHub, and the exit criterion spans all of them.
- **The exit test** (05 M2, over the real binary): fake agents in the
  `MarkerShell` tradition — POSIX-sh scripts that paint idle/working/blocked
  screens with the `\r`-overwrite vocabulary the rig's rasterizer accepts,
  invoke the *really installed* hook config (their "hooks" call the real
  `amx _hook` with payloads shaped exactly as V01 recorded — fidelity is
  anchored to the spike's recordings, R-M2-14), block on `read` lines never
  timers, and record a `--resume <ref>` invocation to a file when restarted.
  A registry override stanza (V03's seam) gives them a manifest. Then:
  five named agents via `agent start a1..a5`; scripted turns drive statuses
  through every V01 edge case — Esc-interrupt (working screen cleared to
  idle, no Stop report), dialog cancel (blocked screen cleared, no hook),
  subagent noise during and after the parent turn; the status line's `⚑N`
  and per-pane states are asserted through the rasterized real client;
  `next-attention` chords cycle exactly the blocked set in block order;
  `wait --until blocked` returns sub-tick; then SIGTERM, restart, and every
  one of the five fake conversations proves resume by its recorded ref.
- **Acceptance:**
  - `five_named_fake_agents_report_correct_status_through_the_spike_edge_cases`
  - `esc_interrupt_without_a_stop_event_settles_idle_within_the_bound`
  - `dialog_cancel_without_a_hook_event_unblocks_via_screen`
  - `subagent_stop_after_parent_stop_never_revives_the_idle_pane`
  - `next_attention_cycles_the_blocked_set_in_block_order`
  - `wait_until_blocked_races_no_poll_interval`
  - `restart_resumes_every_conversation_by_recorded_ref`
  - `hook_reports_from_a_foreign_token_never_touch_a_tracked_pane`
  - plus the **live smoke** (the standing lesson: green suites have twice hid
    non-working features): the checklist run by hand against the real
    installed Claude Code — install, five sessions, block/cancel/interrupt,
    cycle, restart, resume — recorded in `docs/notes/hook-coverage.md` with
    date and versions before M2 is called done.
- **Prompt draft:** You own M2's integration: wire what the waves could not
  touch together, then prove the milestone over the real binary. Read
  `docs/06-m0-plan.md` §T19 and `docs/07-m1-plan.md` §U10 first — this task
  exists because exclusive file ownership leaves seams unowned, and your
  scope is exactly those seams. Build the fake-agent fixtures on the rig's
  own conventions (`tests/support/` — `MarkerShell`, condition waits, short
  env tags, darwin drain rules, the rasterizer's CSI vocabulary; the support
  survey in `docs/08-m2-plan.md` V17 lists every trap), with hook payloads
  copied from the spike's recordings so the fakes cannot drift from the real
  agents' shapes. Then the exit suite as V17's scenario list states it, and
  the by-hand live smoke against real Claude Code recorded in the findings
  doc. Record what shipped differently than planned in a "wave outcomes"
  section of `docs/08-m2-plan.md`, as 07 §5 did — the next milestone plans
  from what happened, not what was hoped.

---

## 6. Waves and merge order

Merge in wave order; within a wave, any order — no two tasks in a wave touch
the same file.

| Wave | Tasks | Concurrency | Unblocks |
|---|---|---|---|
| 0 | **V01** spike | 1 | everything (the findings gate V03/V04/V10/V17) |
| 1 | **V02** contracts | 1 | all waves |
| 2 | V03 registry · V05 text view · V06 manifest engine | 3 | wave 3 |
| 3 | V04 fusion · V07 identity/spawn · V12 pane driving | 3 | wave 4 |
| 4 | V08 AgentHub · V09 `_hook` · V11 waits/events | 3 | wave 5 |
| 5 | V10 installers · V13 agent verbs · V14 client | 3 | wave 6 |
| 6 | V15 resume · V16 skill+notifier | 2 | V17 |
| 7 | **V17** integration + exit | 1 | M2 exit |

**File-ownership check for concurrent waves** (no overlaps):

- Wave 2 — V03: `agent/registry.rs`, `assets/agents.toml`,
  `tests/registry.rs`. V05: `amx-vt/src/snapshot.rs`, `amx-vt/tests/`.
  V06: `agent/manifest/**`, `assets/manifests/**`, `tests/manifest.rs`,
  `dispatch/agent.rs` explain arm — **moved**: to keep wave 2 disjoint from
  nothing (V09/V13 touch `dispatch/agent.rs` in later waves), V06's explain
  dispatch arm is declared a sequential fill of V02's stub, landing with V06
  but building only against types. `agent/mod.rs` was planted whole by V02 so
  neither V03 nor V06 edits it. Disjoint.
- Wave 3 — V04: `agent/fusion.rs`, `tests/fusion.rs`. V07: `amx-core/src/
  platform.rs`, `platform/process.rs`, `agent/identity.rs`, `core/spawn.rs`,
  `state/pane.rs`, `tests/identity.rs`. V12: `dispatch/pane.rs`,
  `actor/pane_host/**`, `tests/drive.rs`. Disjoint.
- Wave 4 — V08: `actor/agent_hub/**`, `session/serve.rs`, `actor/mod.rs`,
  `dispatch/agent.rs` next arm, `tests/agent_hub.rs`. V09: `crates/amx/src/
  cmd/hook.rs`, `dispatch/agent.rs` report arm — **conflict**: V08 and V09
  both fill arms of `dispatch/agent.rs` in one wave. Resolution: V02's stub
  file carries one arm per method, and the wave assigns the whole file to
  **V09** (report arm); V08's `agent.next` handler body lives in
  `actor/agent_hub/` with `dispatch/agent.rs` calling through the handle V02
  already typed — so V08 does not edit the dispatch file at all. V11:
  `conn/**`, `dispatch/{wait,events}.rs`, `cmd/events.rs`, `tests/waits.rs`.
  Disjoint after the resolution.
- Wave 5 — V10: `crates/amx/src/cmd/integration.rs`,
  `crates/amx/src/integration/**`. V13: `dispatch/agent.rs` (sequential,
  prior-wave file), `agent/address.rs`, `tests/agent_verbs.rs`. V14:
  `amx-client/src/**` (its listed files). Disjoint.
- Wave 6 — V15: `agent/resume.rs`, `core/restore.rs`, `core/persist.rs`,
  `tests/resume.rs`. V16: `crates/amx/src/cmd/skill.rs`, `crates/amx/assets/`,
  `examples/`. Disjoint.
- Wave 7 — V17 owns the seams by exception.

Cross-wave sequential edits are declared, not discovered: V07 extends V02's
`core/spawn.rs`; V08 fills V02's `actor/mod.rs` handle bodies and edits M1's
`serve.rs`; V09 then V13 fill successive arms of V02's `dispatch/agent.rs`;
V11/V12 fill V02's `dispatch/{wait,events,pane}.rs` stubs; V14 splits
`app/wired.rs` (M0's file). All sequential, never concurrent.

### Wave outcomes — where reality diverged

Written by V17 on the way out, the way 07 §5 did, so M3 plans from what
happened rather than from what was hoped. Only the divergences are here; the
tasks that landed as written are not listed.

**W-1 — The plan's biggest miss was a seam nobody's file contained.** Every
`agent.*` row and every wait worked in `crates/amx-server/tests/` and answered
a real client with a refusal. `session/serve.rs` built the hub with a
`StatusView` of its own, and `Router::attach_agent` had no call site outside a
test harness — so over a socket the hub's mailbox was unreachable and the view
every wait predicate reads was written by nobody. Both halves were correct and
merged; the join belonged to neither owner's files. V17 closed it (the gateway
carries the handle between the bind and the accept loop, and hands connections
the view the hub writes), and `tests/agents.rs` is the suite that fails
seventeen ways without it. **The lesson for M3's plan is a scope rule, not a
bug report**: a task that adds a capability to *connections* must name the
serve-path assembly in its scope, or the integration task inherits it — and it
inherits it silently, because the in-process rigs are green either way. This is
the third time the standing live-smoke lesson has been paid for.

**W-2 — Tier 3's probe walk shipped with no caller, and still has none.**
V07 built `agent/identity.rs` — the foreground-job walk, the wrapper
unwrapping, `ProbeGate` with its three triggers — and the hub identifies panes
from the *spawn argv* (`program_named`) and from the first hook report only.
The comment in `agent_hub/mod.rs` naming "the foreground-job probe V13 wires to
its readiness handshake" describes a wiring V13 did not do, and V17 deliberately
did not do either: the useful trigger is `Damage` (a pane whose argv does not
name its agent, painting for the first time), and reaching it means the hub
awaiting a pane-host round trip from inside its own loop — which is a change to
the actor's shape and to its shutdown discipline, both fixed by §3 and
constrained by R-M2-6. That is a design decision, not an integration one.
**What it costs today:** a `claude` typed by hand is identified by its first
hook report, which V01 measured arriving at `SessionStart`, so Claude Code is
unaffected. A **Codex** launched by hand is not identified until the user's
first prompt, because Codex fires no `SessionStart` before then (V01 §4) — the
exact case `identity.rs`'s own module docs were written about. M3 owns it.

**W-3 — `agent.start` readiness can be answered by the opening assumption.**
Identification transitions a quiet tracker to `Idle` with cause `probe`
(`fusion/tracker.rs`, deliberately: "output arriving is a turn in progress,
silence is a prompt waiting"), and `agent.start`'s readiness predicate asks only
for kind-plus-`Idle`. So for a stanza with `startup_grace_ms = 0` the handshake
can return before the agent has painted anything at all. The shipped stanzas'
3 s and 5 s graces hide it, and the grace is the mechanism the plan chose for
exactly this — but "readiness is evidence, not optimism" is not what the
predicate currently says. A predicate that also required a non-`probe` cause
would say it. Not changed in V17: it is V13's design and a behavior change to a
merged verb. The exit suite waits for the paint itself and says so where it does.

**W-4 — R-M2-2 is withdrawn; 04 §5 was right.** The plan recorded Codex's flag
as `[features] codex_hooks` from third-party documentation. On the 0.147.0 that
V01 actually installed and drove, hooks are `stable`, on by default, and the
flag is `[features] hooks` — 04 §5's spelling. `codex_hooks` is a crate path
inside the binary. 04 §5's "hooks experimental, behind a feature flag today"
clause was the stale one and V17 corrected it. R-M2-1's fallback branch is
retired with it: Codex is installed, measured, and ships `edges`.

**W-5 — R-M2-9 resolved as flagged, and 04 §5 now says so.** The registry
ships as parse-at-startup, not codegen, exactly as D-M2-2 argued. 04 §5's
"generated from it" is now spelled "derives from it at load", with the
conformance test named as what replaces the compile-time guarantee. No
behavior changed; the document stopped promising a mechanism the milestone
deliberately did not build.

**W-6 — The `full` coverage class has still never been measured.** It is in
04 §5's table on herdr's production data and nothing in M2 tested it; V01
recorded it as leftover L12. The class exists in the type and the fusion
machine has its arm, which is right — but no agent may be shipped into it
without its own matrix. 04 §5's table now says so in the row.

**W-7 — The seam ledger emptied on schedule.** V02 opened twelve; V12 closed
four, V09 one, V11 three, V13 two, and V17 the last two (`agent.explain`,
`agent.next`). The helper, its error code and the `tests/hygiene.rs` exemption
were deleted together, and `tests/skew.rs` gained the wire-side half — no row
may answer the retired code. Second milestone running that this discipline has
worked exactly as designed; it is worth keeping for M3.

---

## 7. Risks & findings

Flagged for the orchestrator, not silently resolved.

**R-M2-1 — Codex is not installed on the dev machine, and its trust gate may
resist automation.** The spike measures Claude Code fully; Codex only if an
install succeeds, and even then its hash-pinned interactive hook approval may
block unattended measurement. The fallback is honest: `coverage = "identity"`
in the stanza with the missing measurement named, resume still working (refs
ride `SessionStart`-equivalent identity), and the class promoted in a later
milestone when measured. 04 §5's table already lists Codex as
`edges (experimental)` — shipping `identity` first is a *narrower* claim, not
a contradiction, but it is a visible difference (flagged, and mirrored in the
conformance test's tie to the findings doc).

**R-M2-2 — WITHDRAWN (V01 measured it; see §6's W-4).** 04 §5 named the flag
correctly and the correction ran the other way: on Codex 0.147.0 hooks are
stable and on by default behind `[features] hooks`, and it is 04 §5's
"experimental" clause that was stale. The original text follows.

~~**R-M2-2 — 04 §5 names the wrong Codex feature flag.**~~ Third-party
documentation of Codex ≥0.114 says `[features] codex_hooks = true`; 04 §5's
class table and herdr's installer both say `[features] hooks` (herdr even
deletes a deprecated `codex_hooks` key — the flag has changed name at least
once *toward* `codex_hooks`, herdr's tree predating the rename). V01 verifies
against a real install; 04 needs a one-word doc PR once confirmed.

**R-M2-3 — The one-publisher rule is already broken, and AgentHub must not
make it worse.** 04 §2 and `actor/mod.rs:88-90` say Core publishes; in fact
the pane actor publishes seven event kinds directly *and* reports them to
Core, which republishes six (`pane_host/actor.rs:124-148` vs
`core/report.rs:32-79`) — every damage/title/history/exit transition gets two
sequence numbers today. M2 does not fix this (out of scope, wire-visible
behavior) but must not extend it: agent events have exactly one publisher
(the hub), and the duplication is recorded here for a dedicated cleanup with
its own golden/skew review. Consumers M2 adds (waits, the client) are written
against at-least-once delivery of pane events, which the state-predicate
contract already tolerates.

**R-M2-4 — The server→client event path is M2's largest unbudgeted piece.**
05 M2's wording ("`amx events --json`", "`⚑N` indicator") presumes events
reach consumers; the tree has no server-initiated notification anywhere and
the client drops any it sees (`net.rs:206-210`). V11+V14 build it. It is also
the pattern-setter for M3's reconnect-resync, so its gap contract gets the
same golden treatment as the M0 protocol surfaces.

**R-M2-5 — Module budgets bind before M2 writes a line.** At the soft budget
already: `actor/core/mod.rs` 500/500, `actor/core/pane.rs` 499/500,
`actor/persist/actor.rs` 505 (warning). Near it and certain to grow:
`actor/mod.rs` 471, `app/mod.rs` 482, `app/wired.rs` 455,
`control/mod.rs` 371 (+~110 lines of rows). V02 front-loads the splits
(`core/route.rs`, `core/spawn.rs`, `control/table.rs`) and V14 splits
`app/events.rs` out of `wired.rs` — the R-M1-3 rule: no split waits for the
hard limit.

**R-M2-6 — The shutdown wedge constrains AgentHub's design, again.** The
JoinSet drain hang (R-M1-2) remains undiagnosed. AgentHub follows Persist's
discipline to the letter — receive-only after cancel, no sibling requests, no
final flush (its durable state is already mirrored into Core during normal
operation, by design, so shutdown has nothing to save). The V08 acceptance
test asserts it the way the M1 tripwire does, and the V17 shutdown storm
inherits `shutdown_within`'s bounded diagnostics.

**R-M2-7 — The goldens coverage law bites twelve times plus four.** Every
method row owes a proto golden and a `sample_params` skew arm; every event
variant owes a `tag()` arm, an `every_event()` fixture, and an envelope
golden; `session.state`'s new fields and the persist snapshot's new fields
regenerate their goldens. All of it lands in V02 so no wave task ever
discovers the law mid-flight. The counts are the review checklist: 12 method
goldens, 12 skew arms, 4 event goldens, 1 notification-delivery golden, 2
regenerated reply/persist goldens.

**R-M2-8 — darwin is tier-1 CI and three of its differences sit exactly under
M2.** (a) `KERN_PROCARGS2` argv reading has no libproc wrapper and a
fiddly buffer layout — V07 budgets for it, and the identity suite runs on
both platforms. (b) Real agent tooling does not exist on CI runners, so the
exit suite runs on spike-anchored fakes everywhere, and the live smoke is a
by-hand exit gate (R-M2-14). (c) The rig's darwin lore applies with force to
fake agents: bash 3.2 scripts, `pgrep -f` marker uniqueness, pty drain rules,
the rasterizer's small CSI vocabulary (spinners as `\r`-overwrite), sun_path
budgets. All encoded in V17's fixtures, none discovered fresh.

**R-M2-9 — RESOLVED as flagged (see §6's W-5): 04 §5 now says "derives from it
at load".** The doc conversation this asked for happened, and the answer was to
correct the wording rather than build a codegen step. The original text follows.

~~**R-M2-9 — "Generated registry" is delivered as parse-at-startup, not
codegen.**~~ 04 §5/05 M2 say `agents.toml` → "generated"
lookup/labels/resume/fusion config. D-M2-2 satisfies the intent (one
declarative source, no hand-synced lists, conformance-tested) without a
codegen step, because every consumer is runtime data and the override path
forces a runtime parser to exist anyway. If "generated" is read strictly as
compile-time, that is a doc conversation before V03, not a silent redesign.

**R-M2-10 — `agent rename` is an alias, not a method.** 04 §5 lists
`agent prompt|wait|read|rename` as agent verbs; D-M2-9 delivers rename (and
read) by resolving the agent target and calling the existing `pane.*` rows,
because the agent's name *is* the pane label. The CLI surface matches 04; the
method table stays one-per-behavior. Flagged in case 04 intended a distinct
agent-rename semantic (it names none).

**R-M2-11 — The staleness deadline is new ground with no herdr precedent.**
herdr keeps a detected state forever until contradicted; 04 §5 mandates a
bounded staleness timeout as fusion's third exit. Sizing it wrong shows up as
either flapping (too short) or herdr's stuck-status bug (too long). V01's
latency data sizes it, V04's property tests pin its interaction with the
confirmation window, and the exit suite's interrupt scenario exercises it
end to end.

**R-M2-12 — One new dependency: `regex`.** Needed by manifest `regex`/
`line_regex` gates and `pane.wait_output`. Everything else in M2 rides the
existing tree (`toml` arrived in M1; property tests reuse the workspace's
`proptest`). The justification line lands in the commit that adds it, per
HACKING.md.

**R-M2-13 — Manifest hot reload is local-only in M2.** 04 §5 says manifests
are "hot-reloadable from a catalog"; 05 M2's task list names the engine,
the bottom-buffer read, and `agent explain`, but no catalog. M2 ships the
override directory re-scanned on the config watcher's reload event; the
remote catalog (herdr's `index.toml`-over-curl machinery) is M4's registry
work. Flagged as scope interpretation, consistent with the roadmap's M4 line
("registry stanzas for the long tail").

**R-M2-14 — The exit criterion is only as strong as fake-agent fidelity.**
"Status always correct" in CI is measured against fakes; the fakes are
anchored to V01's recorded payloads and screens, and the conformance tie
(V03) plus the recorded live smoke (V17) close the loop against the real
tools. The standing lesson stands: twice a green suite hid a non-working
feature, so M2 does not exit on green tests alone — the by-hand smoke with
real Claude Code, dated and versioned in the findings doc, is part of the
exit definition.
