# amx architecture — the design

This is the design as I'd build it: keep herdr's proven bones (K1–K10 in
[02-herdr-critique.md](02-herdr-critique.md)), fix every structural weakness
(W1–W12) at the architecture level rather than by patching, and let minimalism
delete whole subsystems instead of trimming them.

## 1. Process model

One binary, two roles (herdr's three minus the legacy monolith):

```
amx            → probe socket; daemonize `amx server` if absent; attach
amx server     → the daemon: owns PTYs, terminal state, agent state, persistence
amx <verb> …   → CLI: one-shot or streaming calls over the same socket
```

- **One socket per session** (`$XDG_RUNTIME_DIR/amx/<session>/sock`, 0600).
  Not two, not three (fixes W3's surface sprawl). Stale-socket disambiguation
  by connect probe (herdr's lock-free single-instance trick, kept).
- **No `--no-session` mode** (fixes W4). The server is the only runtime; a
  "local" run is just server + client in one process tree, same code paths.
- **Named sessions with a lifecycle**, as in herdr: `--session work` /
  `AMX_SESSION` select a named server instance (socket + state under its own
  directory); `amx session list|attach|stop|delete` enumerate and manage
  running servers. A daemon that outlives terminals must be discoverable and
  stoppable from the CLI.
- **Single-pane attach**: `amx attach --pane <target> [--takeover]` attaches
  the current terminal full-screen to one pane with no chrome (a degenerate
  one-pane client viewport), detach with prefix+q — herdr's direct
  terminal-attach mode, kept. This is how you hand one agent's terminal to a
  plain SSH window or another tool.

## 2. Runtime: actors + one event bus (fixes W1, W2, W9)

The server is a set of **tokio actors with typed mailboxes**, supervised by a
root task with `CancellationToken` + `JoinSet` (structured shutdown; nothing
detached, everything joined):

| Actor | Owns |
|---|---|
| `Core` | session state tree (workspaces → panes; tabs are deliberately flattened away, see D13), layout, focus, labels |
| `PaneHost` (per pane) | PTY I/O actor thread + VT state + status tracker |
| `Gateway` | socket accept, connection tasks, protocol negotiation |
| `Persist` | debounced snapshot capture, fsynced writes, restore |
| `AgentHub` | agent registry, hook-report ingestion, status fusion, attention queue |

**The event bus** is the spine: one publish point read by per-subscriber
cursors over a bounded replay ring. Every state transition (`pane.created`,
`agent.status{working→blocked}`, `pane.exited`, …) is one typed event with a
monotonic sequence number. "Broadcast" here names the fan-out, not a
`tokio::sync::broadcast` channel — that primitive is not used anywhere in the
tree, because a subscriber that falls behind it learns only *how many* messages
it lost, and the gap contract below hands it the sequence range instead.

**The wait/gap contract** (loss must be visible *and* recoverable):

- Subscribers that fall behind the replay buffer get an explicit
  `gap{from,to}` event — never a silent drop.
- **Waits are state predicates, not event predicates.** `amx wait --until
  blocked` evaluates the predicate against current state at subscribe time,
  then uses events as notification; on any `gap` it re-evaluates state before
  consuming events after `to`. A transition falling inside a gap can therefore
  never hang a wait.
- Every state-query response (and the subscribe reply) carries the bus
  sequence number at which it was captured, so an external `amx events --json`
  consumer that sees a gap re-queries state and resumes from the returned seq
  without racing new transitions.
- Wait targets: `--until blocked|idle` are agent statuses; `--until exited`
  is the `pane.exited` event (process end). There is no `done` agent status.

**Render/output dirtiness is structural, not conventional** (fixes W2): every
message handler returns an `Effect` value (`Nothing < PaneDamage(id) < Layout
< Full`); the loop folds effects and schedules output. There are no mutable
`needs_render` flags to keep in sync — forgetting is a type error, not a stale
frame.

Session/socket paths live in a `Ctx` struct passed explicitly — no env-var
globals, no test mutexes (fixes W9).

## 3. Rendering: smart client, server-owned state (the Superlogical insight)

Herdr renders the entire TUI server-side and streams UI frames; clients are
dumb blitters. That makes clients trivial but couples the wire protocol to
every UI pixel, prices scrollback interaction at a round trip, and re-renders
chrome server-side for every attached client size.

amx splits authority differently:

- **Server owns truth**: PTYs, VT grids (libghostty-vt), scrollback, layout
  tree, agent status. This is non-negotiable — it's what makes persistence,
  handoff, and detection possible.
- **Client owns presentation**: it receives the layout tree + status model as
  state (not pixels), draws its own chrome (borders, status line, picker), and
  receives **per-pane grid deltas** (damage rectangles of cells, cursor state)
  only for panes visible in *its* viewport.
- **Scrollback is locally cached**: the client requests history ranges by
  stable row id, caches them, and scrolls locally at memory speed.
- Copy-mode selection runs client-side over the cached grid in stable-row
  coordinates.

**Scrollback identity model** (a cache needs an invalidation contract, like
the event bus's `gap`):

- Per-pane **monotonic row ids**, assigned when a line is committed to
  history, never reused across trimming or `clear`.
- A `history.invalidated{pane, from_row}` event fires on width-change reflow
  (only width changes reflow) and on `clear`: clients drop cached ranges and
  cancel any copy-mode selection anchored at or beyond `from_row`.
- Pane metadata carries an eviction floor (`oldest_row`) so clients know which
  ranges the byte-limited scrollback has evicted and cannot re-fetch.
- Rows scrolling out of the live grid into history are announced on the pane's
  delta stream (id + content hash; content push for panes the client is
  actively scrolled back in), so scrolling up during heavy output is defined.

**Client sizes.** The pane's PTY grid size follows the most-recently-active
client — herdr's foreground-client rule, kept; it is strictly better than
tmux's smallest-client-wins. What amx removes is the foreground-client
compromise for *views*: non-active clients letterbox/clip the server-sized
pane grids inside their own locally-computed chrome and keep independent
scroll positions. Viewport-visibility subscription is defined by the client's
own layout projection. Switching between differently-sized active clients
triggers resize→reflow→invalidation churn for other clients' caches; the
server debounces grid resize on active-client switch to bound it. Theme and
keybinding authority also stays with the client: each client renders chrome in
its own theme and may bring local keybindings at handshake (herdr's
`ClientKeybindings::Local`, kept).

**Terminal core**: **libghostty-vt**, now explicitly a public building block.
FFI bindings are generated at build time and mapped into Rust enums through
exhaustive matches with round-trip tests. This fixes both of herdr's binding
failure modes: its bindgen output is committed with no regeneration check
against re-vendored headers, and a handful of enum values are hand-copied in
`ghostty/mod.rs` on top (W10). Vendoring follows herdr's patch-manifest
discipline (K2).

**PTY hot path** (keeps K4, fixes W10's lock contention — precisely): the
parser I/O thread **exclusively owns the libghostty-vt instance**; VT state is
never shared or swapped (the grid is one mutable FFI object holding paged
scrollback, modes, and dirty flags — it cannot be double-buffered). At frame
boundaries the parser copies damaged visible rows + cursor into a derived,
double-buffered POD cell snapshot published lock-free for render and tier-2
detection (including the bottom-buffer detection region). Scrollback-dependent
reads — history ranges, persistence snapshots — are served as commands
executed on the parser actor thread, serialized like herdr's PTY actor
commands. Readers therefore never contend with the parser on a pane-state
mutex; the narrow `response_order` lock spanning the read callback and
out-of-band query replies is retained verbatim, preserving herdr's
reply-ordering guarantee (including substituted OSC 10/11 replies).

## 4. Protocol: one surface, skew-tolerant (fixes W3)

One transport, two channel kinds multiplexed over the session socket:

```
frame  = [u32 len][u8 channel][payload]     len capped: 1 MiB control frames,
chan 0 = control: JSON-RPC 2.0              per-stream negotiated caps for
chan N = binary stream bound by a           binary channels (K7 kept)
         control call: pane grid deltas,
         history ranges, raw pane I/O
         (observe/control), reserved kind
         for pane graphics (kitty images)
```

- **Hello/Welcome negotiates capabilities**, not equality: client sends
  `{proto: [min, max], features: [...]}`; server picks the highest common
  version and echoes the feature intersection. Unknown JSON fields are ignored
  by contract; binary stream types are versioned per-stream.
- **Compatibility window**: server supports current and previous protocol
  version, minimum. A release never strands every remote host at once.
- **Flow control is part of protocol v1, not a later optimization.** Damage
  deltas are incremental — unlike herdr's full-frame diffs they cannot be
  dropped without corrupting the client grid — so:
  - Per client, per visible pane, the server keeps an accumulated dirty-region
    set (rects + grid generation), not a queue of deltas. When the client's
    writer is not ready, new damage coalesces into that set; on drain the
    server emits one delta built from the current authoritative grid. Cost is
    O(dirty bookkeeping) per client — cell contents are always read from the
    single authoritative grid at send time (this generalizes herdr's proven
    single-slot droppable writer from full frames to damage sets).
  - When accumulated damage crosses a threshold (or on reconnect/resync), the
    server sends a **keyframe** (`grid.reset` + full visible grid). Keyframes
    exist in protocol v1 so overflow recovery is skew-compatible forever.
  - The connection writer has strict channel priority (control > grid deltas >
    history/bulk), and history transfers are chunked — a big scrollback fetch
    can never head-of-line-block a keystroke's response.
- **Predictive local echo is deliberately out of v1.** Keystrokes round-trip;
  the M4 latency budget (p99 < 5 ms local) is a round-trip budget. If echo
  prediction is ever wanted (mosh-class speculation + reconciliation), it
  needs input sequencing in the protocol and enters only through a
  capability-negotiated extension.
- The control schema is one annotated Rust enum from which we **derive**: serde
  wire names, method dispatch, the JSON Schema artifact, CLI clap tree and
  parsing, and docs (fixes W6's four hand-synced lists — adding a method is one
  enum variant + one handler). Payload types live in per-domain modules; only
  the variant list lives on the enum (herdr's `schema/*` layout, which is what
  keeps the enum itself small).
- Remote stays herdr's best trick (K8): the same protocol over
  `ssh host exec amx _bridge` stdio. The smart-client rendering model makes it
  bandwidth-cheap without a special encoding.

## 5. Agent layer: hook+screen fusion, one registry (fixes W5, W6; K5 kept)

**Why not hooks-authoritative: herdr tried it.** herdr 0.3.0 shipped
hook-driven working/blocked/idle for Claude Code and Codex and later reverted
both to identity-only (its installer now strips the old state hooks —
`HOOK_REMOVALS` in `claude_settings.rs`), because the agents' hook systems
have coverage gaps that no transport can fix: Esc interrupts and
permission-dialog cancels emit **no hook event**, and subagent stop events
arrive out of order and falsely idled the parent pane. A better hook *binary*
(amx ships `amx _hook`, one static binary instead of sh+python heredocs —
fixing herdr's silent no-op when python3 is missing) fixes delivery, not
coverage.

**So amx fuses, with per-transition precedence** (herdr rejected
dual-authority because "two competing sources of truth" — fusion answers that
by defining which source wins where):

- Hooks assert **entry edges** with high confidence and zero latency: turn
  start → `working`, permission request → `blocked`, turn stop → `idle`.
  These apply immediately.
- **Exits from working/blocked are confirmed**, not trusted: a hook-asserted
  state is cleared by (a) a matching hook event, (b) tier-2 screen detection
  contradicting it, or (c) a bounded staleness timeout — whichever comes
  first. User interrupts and dialog cancels, invisible to hooks, are caught by
  (b)/(c). The M2 spike widened this: on both shipped agents **every**
  user-initiated exit is silent — Esc during generation, Esc during a tool
  call, a dialog answered "No", a dialog cancelled with Esc — so for an `edges`
  agent clause (a) is unreachable and tier 2 owns the exits outright. It
  narrowed one thing too: `PermissionRequest` fires *before* its own dialog
  paints, so the `blocked` **entry** is trustworthy on its own.
- Subagent-scoped events never override the parent turn's state (herdr's
  "never revive an idle pane" lesson, kept as a rule of the fusion machine).

**The registry encodes coverage, not assumptions.** Each agent's stanza
carries a hook coverage class:

| Class | Meaning | Examples (per herdr's production data) |
|---|---|---|
| `full` | hook system covers the complete lifecycle; hooks may own state outright | Pi, OMP, OpenCode, Kilo — herdr's production data, **never measured here** |
| `edges` | hooks see turn/tool/permission edges but miss interrupts/cancels → fusion | Claude Code; Codex (hooks are `stable` and on by default as of 0.147.0, behind `[features] hooks`) |
| `identity` | hooks carry session identity only → screen detection owns state | agents with no usable lifecycle hooks |
| `none` | tier-3 heuristics only | unknown programs |

An **M2 spike validates coverage empirically** (does Claude Code's Stop fire
on Esc-interrupt? any event on dialog cancel? Codex flag status) before any
agent is promoted a class — classes are measured, not aspirational. It ran:
`docs/notes/hook-coverage.md` is the measurement, and both shipped agents came
out `edges`. Two clauses above are its corrections. Codex's hooks are no longer
experimental and the flag is the one named here, so the "experimental, behind a
feature flag" reading is retired; and the `full` row is herdr's inheritance
rather than an amx finding — no agent has been measured into it, and none may
be shipped into it without its own matrix.

**Tier 2 — screen manifests.** herdr's TOML rule engine (priority, region,
gate trees, `skip_state_update` freezes, bottom-buffer snapshot, `agent
explain` debuggability) is genuinely good — kept, running continuously as the
fusion partner for `edges` agents and the sole source for `identity` agents,
hot-reloadable from a catalog.

**Tier 3 — heuristics.** Process-tree foreground job + prompt detection for
unknown programs: `busy/quiet`, never fake `blocked`.

**One agent registry.** A single declarative file (`agents.toml`, compiled in,
overridable) is the *only* place an agent is defined: id, aliases, executable,
label, resume argv template, hook coverage class, manifest, integration asset.
Every consumer — lookup, resume planner, fusion configuration, integration
installer, docs table — **derives from it at load**: the file is embedded with
`include_str!`, parsed once into a `Registry` at server start, and merged with
the user's override. "Generated" here means *no second list anywhere*, not a
codegen step; M2 weighed a macro and rejected it, because every consumer is
runtime data and the override path forces a runtime parser to exist in any case
(D-M2-2, and R-M2-9 flagged this wording before V03 shipped). Adding an agent =
one stanza (fixes W6). What replaces the compile-time guarantee is a
conformance test that walks the registry and asserts every derived surface
agrees — including that the shipped coverage classes equal the ones
`docs/notes/hook-coverage.md` measured, so a stanza cannot drift from the
experiment.

**The per-pane status tracker is an explicit typed state machine** — states
and transitions as data, property-tested — not 400 lines of mutable locals
(fixes W5's fragility).

**Agent addressing.** Every agent verb targets by user-assigned name or pane
UUID: `amx agent start dev --kind claude -- …` starts an agent in a pane and
returns only when it owns the terminal and is ready for input (readiness
handshake with timeout, herdr semantics); `agent prompt|wait|read|rename dev`
address it thereafter. Five identical Claude panes are only orchestratable if
they have names.

**The attention queue** lives in `AgentHub`: agents entering `blocked` enqueue
(ordered by block time), leaving it dequeue. `next-attention` (one prefix key)
focuses the head; the status line renders `⚑3` from the same state; the client
emits an OSC 9/99 notification to the host terminal on enqueue (the one
built-in notify path — see 03). Exposed over the API so external tools
consume the identical queue.

**Resume** keeps herdr's rigor wholesale (K5): refs validated, argv as data,
allowlisted sources, dedupe reservations with rollback — but the tables come
from the registry.

## 6. Persistence: one snapshot, fsynced, UUID-keyed (fixes W8)

- Every workspace/pane has a **stable UUID at creation**, used across
  snapshot, scrollback, handoff, and the API from M0 (user-facing short
  numbers are a display mapping, stable across restarts as in herdr).
- **One snapshot file** (`session.json`): layout tree, per-pane
  cwd/argv/**label**/agent identity/session refs, keyed by UUID. Written
  atomic tmp+rename **+ fsync of file and directory**. Version field with an
  N/N−1 read window.
- **Scrollback sidecar per pane** (`history/<pane-uuid>.rows`, not `.ansi`:
  history is served as unstyled text rows, so the file holds packed rows and
  not a replay stream — see R-M1-1), linked by UUID
  — no positional zipping, desync is structurally impossible. Opt-in, as in
  herdr (secrets), wiped independently.
- Client presentation state (status-line options, picker history) lives
  client-side, not in the server snapshot (herdr persisted sidebar widths in
  the server — their own guardrail said not to).
- **Restore reports loss**: panes/workspaces that fail to respawn produce a
  restore report shown in the status line and queryable via
  `amx session report` — never log-only.
- Live **binary-upgrade handoff** keeps herdr's SCM_RIGHTS design (K3):
  quiesce PTY actors, pass fds + manifest over a token-authenticated socket,
  staged ready/restored/committed/owned handshake, strict abort on partial
  import. The manifest carries per-pane serialized VT state + recent
  scrollback (herdr's `initial_history_ansi` mechanism), keyed by UUID —
  handoff never depends on the opt-in history sidecars, so default-config
  users don't lose screen contents on upgrade.

**Clients across handoff/restart.** Client connections do not survive the
process swap (sockets die with the old server); continuity is a defined
resync, not an accident: the handoff manifest carries the event-bus sequence
counter and per-pane grid generations, so the successor continues both without
reset. A reconnecting client re-Hellos presenting its last cursor and
generations; the server replies with events-since (or `gap`) and keyframes for
stale grids. Scrollback row ids are continuous across handoff (they ride the
VT state in the manifest). In-flight `amx wait` calls retry transparently
inside the CLI across the reconnect.

## 7. Input model: modal, keyboard-only

- **Terminal mode** (default): keys go to the pane, kitty-protocol aware,
  per-pane keyboard flags tracked as in herdr.
- **Prefix mode** (`ctrl+a`, configurable): one-shot commands — split, zoom,
  kill, detach, next-attention, picker, rename.
- **Navigate mode** (prefix + `w`, sticky): vi/kakoune-style — `hjkl` focus
  movement, `HJKL` resize, `x`/`v` splits, **`s` + direction swap panes**,
  `m` move pane (picker chooses target workspace), `d` close, numbers jump,
  `Esc` back. One sticky layer for movement/resize/rearrange (herdr also has a
  sticky resize mode; amx folds resize into the single navigate layer instead
  of a separate mode). Rearranging never restarts the process in the pane.
- **Copy mode**: client-local over cached scrollback; vi keys; selections in
  stable-row coordinates; `y` → OSC 52 + clipboard stream. `e` opens
  scrollback in `$EDITOR` (herdr feature, kept — peak keyboard workflow).
- **Picker**: one fuzzy-list primitive for every choose-one interaction
  (workspaces, panes, agents, commands, worktrees, move targets). No other
  dialog type exists.
- New splits inherit the source pane's **foreground process cwd** by default
  (config-overridable), and `foreground_cwd` is part of pane API state —
  "split and land in the same directory" is a many-times-a-day behavior.
- Keybindings in config TOML; the dispatch table is data, introspectable via
  `amx keys`. **`[[keys.command]]` bindings** map a key to an arbitrary argv
  (spawned detached or in a pane) — the escape valve an extension-by-API
  design leans on.
  - **Keybindings are client-side by construction**, and that is what the
    `[keys]` section built in M4 assumes: a client resolves the prefix and the
    prefix table out of its own `config.toml` and sends the resulting calls, so
    no binding table exists on the server and none is designed. The wire's
    `client::Keybindings` enum (`amx-proto/src/control/client.rs:34-46`) is
    therefore **documentary**, the shape `--remote` already uses on the CLI: its
    `Server` variant names a thing there is none of, a client has always been
    `Local`, and a field that can only carry one value carries nothing. Recorded
    rather than deleted — removing it is a wire change for no gain. The section
    itself is [12-config.md](12-config.md); `[[keys.command]]` above is still
    unbuilt, and that document says so.
- No chrome mouse handling. `mouse_forward = true` (default) forwards SGR
  events to applications that enabled mouse reporting; amx itself never
  interprets them **as chrome input** — no hit-rects, no drag states, nothing
  on screen means anything to a pointer.
  - **Forwarding is not byte-verbatim, and cannot be.** A report's coordinates
    are viewport-absolute; the application in a pane reads them as pane-local,
    and amx's panes are never at the origin — the content area is the terminal
    minus a status line (`amx-server/src/actor/core/view.rs:224-226`,
    `amx-client/src/model/mod.rs:362-368`) and every pane is inset one cell for
    its border (`view.rs:40-45`). Relaying a report to the pane under it
    therefore means subtracting the pane's origin, which is coordinate
    arithmetic and not interpretation: amx still assigns no *meaning* to a
    position. The same translation is what tmux does. This paragraph records
    the constraint; where the subtraction happens is an implementation
    decision, and until it does, the honest description of the path is that it
    is untranslated.

## 8. Extensions: the API is the plugin system (fixes W7)

No manifests, no marketplace, no auto-installed hooks, no sandbox problem.

- Any program can `amx events --json` (subscribe stream) and call any API
  method. Long-running helpers are run and supervised by the *user* (systemd
  unit, shell &, another pane) — amx never executes third-party code on its
  own events.
- The full pane-driving surface is API + CLI: `pane send-text`,
  `pane send-keys` (key-combo grammar: `ctrl+h`, `f1`, …), `pane run`
  (bracketed-paste-aware **queue-order atomic** text+submit — the text and its
  submit are queued back to back on the pane's ordered input queue, placed
  under its ordering lock so nothing can interleave, not even the keystrokes an
  attached connection forwards to the same pane; a single `write()` is
  deliberately *not* used, because a paste-aware TUI can swallow a `CR` that
  shares a read with the paste terminator), `pane read`,
  `pane wait-output --match/--regex` (an event-bus await over pane damage, not
  polling) — the primitives for driving ordinary terminals and tests, distinct
  from `agent prompt`.
- `amx api schema` emits the machine-readable contract (derived from the
  method enum, always in sync).
- The **agent skill** ships in-binary (herdr's K10): `amx skill install`
  teaches agents to drive amx — spawn panes, send input, prompt siblings,
  `wait --until blocked`, read outputs — gated on `AMX_ENV=1` with pane/
  workspace identity env vars.
- Integrations have a lifecycle, not just an install: `amx integration
  install|uninstall|status` with version markers in installed hook assets, and
  `status` runs after self-update — a stale `amx _hook` is worse for amx than
  for herdr, since hooks feed the fusion tier.

If curated extension distribution is ever wanted, it's a doc page listing
repos — not a crawler, a registry format, and a consent dialog.

## 9. Code structure & testing (fixes W2, W12)

- Module-size budget: soft 500 lines, hard 1000, enforced in CI. **Generated
  code is exempt** (bindgen output, schema artifacts). The libghostty-vt safe
  wrapper gets a planned decomposition (grid, scrollback, key-encoding,
  queries) rather than an arbitrary CI-appeasing cut. The budget is an
  architectural forcing function — headless.rs cannot happen.
- Integration tests live in `tests/`, driving the real socket protocol against
  a real server (spawn, attach, split, kill -9, restore, handoff) with virtual
  time where possible; inline unit tests only for pure helpers.
- Protocol golden tests **and the N/N−1 skew harness exist from M0**
  (current-vs-current until a second version exists); flow-control behavior
  (`cat /dev/urandom` into a stalled client) is part of the M0 suite.
- One `platform` trait seam (`Pty`, `Ipc`, `ProcessTree`) with the Unix
  implementation first; a future Windows port implements traits and runs the
  same conformance suite (fixes W11's approach, defers its cost).

## 10. Decision summary

| # | Decision | Replaces (herdr) |
|---|---|---|
| D1 | Actors + a cursor-over-replay-ring event bus with typed gaps; waits are state predicates with gap-resync | 100 ms polling over a 512-entry seq-numbered ring behind a `Mutex`, with no per-subscriber cursor and no gap signal |
| D2 | Structural `Effect` render dirtiness | 94-site manual flags |
| D3 | One socket, JSON-RPC control + bound binary streams, capability negotiation, N/N−1 window, v1 flow control (coalesced damage, keyframes, channel priority) | 3 surfaces, strict version equality |
| D4 | Smart client: state + pane deltas + local scrollback cache with an explicit invalidation contract | server-rendered full-TUI frames |
| D5 | Hook+screen **fusion** with per-agent measured coverage classes | screen-scraping-authoritative for Claude/Codex (after herdr's hooks-authoritative attempt failed) |
| D6 | Single generated agent registry | ~10 hand-synced match sites |
| D7 | UUID-keyed fsynced snapshot + per-pane history sidecars + restore report | positional workspace/tab zip, no fsync, log-only loss |
| D8 | API-only extensions, user-supervised | unsandboxed auto-run plugin processes |
| D9 | Keyboard-only chrome; mouse forwarded (coordinates translated to the pane's frame, never given meaning) | hit-rects, drag states, mobile fork |
| D10 | No monolithic mode; one runtime | `--no-session` dual loop |
| D11 | Unix-first behind platform traits | inline cfg-forked Windows |
| D12 | Size budgets (generated code exempt) + protocol goldens + skew CI from M0 | 10k-line files, inline test inflation |
| D13 | Two-tier tree: workspaces → panes; tabs flattened into workspaces | workspace → tabs → layout |
| D14 | Narrow-viewport single-pane projection, declared to the server so the shown pane is sized to the whole viewport, + compact status line; wheel-only mouse concession, opt-in and off by default (wheel → copy-mode scroll in panes without mouse reporting); touch clients are separate protocol consumers, never TUI chrome — [10-attention-surfaces.md](10-attention-surfaces.md) | amends D9's letter; herdr's mobile layout fork stays deleted |
| D15 | Attention surfaces: `agent.list` + identity-bearing attention events, per-workspace status-line breakdown, agents view as the picker's one extension (live detail line + peek region), `amx agents` CLI, workspace-scoped `next-attention`; queue ordering stays global, grouping is display-only; no generated summaries, no launcher, no filter syntax in core — [10-attention-surfaces.md](10-attention-surfaces.md) | extends the status line and picker (03 §2) |
