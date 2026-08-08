# Roadmap — build order

Ordered so every milestone ends in something usable daily, and the riskiest
architecture bets (event bus, smart-client protocol, PTY layer) are proven
before features pile on. Each milestone lists its exit criteria.

## M0 — Skeleton that multiplexes (foundation)

The core loop: server, PTYs, attach, workspaces, splits. No agents yet.

- Cargo workspace: `amx-core` (state, layout), `amx-vt` (libghostty-vt FFI,
  build-time bindings + static assertions), `amx-proto` (wire types, derive
  pipeline), `amx-server`, `amx-client`, `amx` (CLI bin).
- Vendor libghostty-vt with herdr-style patch manifests.
- PTY actor (poll loop, wake pipe, ordered responses, published POD cell
  snapshots — parser exclusively owns the VT instance).
- Actor runtime: Core/Gateway/PaneHost, CancellationToken + JoinSet shutdown,
  `Ctx` (no env-var globals), event bus with cursors + gap events + the
  state-predicate wait contract.
- **Stable UUIDs** assigned at workspace/pane creation — the API/protocol
  identity from day one.
- Protocol v1: framing with size caps, Hello/Welcome capability negotiation,
  control channel (JSON-RPC), pane-delta streams, history-range requests, the
  scrollback identity model (monotonic row ids, `history.invalidated`,
  eviction floor), **and flow control** — per-client damage coalescing,
  keyframes (`grid.reset`), strict channel priority, chunked bulk transfers.
  These are wire-visible and must exist before goldens freeze the protocol.
- Client: attach, raw mode, chrome (borders + status line), grid blitting,
  local scrollback cache + copy mode, kitty keyboard passthrough, SGR
  mouse-event **forwarding** to panes that enabled mouse reporting (chrome
  never interprets mouse).
- Input: terminal/prefix/navigate modes; BSP splits; zoom; pane swap/move;
  splits inherit foreground cwd; picker primitive (sources: workspaces, panes,
  commands).
- **Workspaces**: create/rename/kill/switch (picker + prefix keys); one BSP
  layout tree per workspace (D13 — no tabs).
- Sessions: auto-detect-or-daemonize `amx`; named sessions (`--session`,
  `AMX_SESSION`); `amx session list|attach|stop|delete`; single-pane
  `amx attach --pane`; clean detach.
- Test rig from day one: integration tests in `tests/` over the real socket,
  protocol goldens, the N/N−1 skew harness (current-vs-current until a second
  version exists), and the flow-control test: a `cat /dev/urandom` pane with a
  stalled client neither grows server memory unboundedly nor corrupts any
  client grid.

**Exit:** I can live in amx as my daily multiplexer over tmux — multiple
workspaces, keyboard only, kill -9 the client anytime, reattach from a second
terminal — and the flow-control test passes.

## M1 — Durability

- UUID-keyed snapshot (the M0 UUIDs), debounced, atomic + fsync (file + dir).
- Restore: workspace tree + respawn shells in saved cwds, prune-and-report
  losses, `amx session report`.
- Pane/workspace labels persisted; rename verbs.
- Scrollback sidecars (opt-in), linked by pane UUID.
- Config TOML: load, file-watcher hot reload, per-section lenient fallback.
- Crash suite: kill -9 server mid-write, restore goldens, power-loss
  simulation (fsync verification).

**Exit:** reboot the machine, run `amx`, everything is back where it was —
and anything that isn't is listed in `amx session report`.

## M2 — Agents (the differentiator)

- Agent registry (`agents.toml` → generated lookup/labels/resume/fusion
  config), conformance test across all generated surfaces.
- **Hook-coverage spike, before committing fusion classes**: empirically
  validate per-agent hook behavior (does Claude Code's Stop fire on
  Esc-interrupt? any event on permission-dialog cancel? SubagentStop ordering;
  Codex `[features] hooks` status). Registry coverage classes
  (full/edges/identity/none) are set from measurements, not hope — herdr's
  hooks-authoritative attempt failed on exactly these gaps.
- Identity detection: process-tree foreground job + wrapper unwrapping.
- `amx _hook` binary + integration installers for Claude Code and Codex;
  `amx integration uninstall|status` with version markers, `status` run
  automatically after self-update.
- Tier-2 manifest engine: port herdr's rule grammar (priority/region/gates/
  skip_state_update), bottom-buffer snapshot, `amx agent explain`.
- The fusion state machine (typed, property-tested): hook edges apply
  instantly; exits confirmed by screen/timeout; subagent events never revive
  an idle pane.
- Attention queue + `next-attention` key + `⚑N` indicator + client-emitted
  OSC 9/99 notify on enqueue.
- Agent addressing (name | pane UUID); `amx agent start <name> --kind K`
  with readiness handshake.
- Event-bus waits: `amx wait --until blocked|idle|exited`, `amx agent prompt
  --wait`, `amx pane wait-output --match/--regex` (await over pane damage, not
  polling), `amx events --json` (gap-resync contract documented).
- Pane driving: `pane send-text|send-keys|run|read`.
- Agent resume: validated session refs in snapshot, argv-as-data planner,
  dedupe reservations; `claude --resume` / `codex resume` on restore.
- Agent skill (in-binary, `amx skill install`), `AMX_*` env injection.
- **Reference notifier ships here** (~20-line script consuming
  `amx events --json` → desktop notification on blocked) — the out-of-terminal
  notification path is an extension, and it must exist the day the many-agents
  workflow does.

**Exit:** run 5 named Claude Code sessions; the status line always knows which
are blocked (validated against the spike's edge cases: Esc-interrupt, dialog
cancel, subagent noise); one key cycles through them; restart amx and every
conversation resumes.

## M3 — Continuity & reach

- Live binary-upgrade handoff (SCM_RIGHTS, staged commit, strict abort);
  manifest carries VT state + recent scrollback + event-bus seq + grid
  generations; client reconnect-resync (events-since or gap + keyframes);
  waits retry transparently across it.
- Self-update: channel manifest, sha256, package-manager detection, handoff.
- SSH remote: `amx --remote host` via `ssh … exec amx _bridge`, remote binary
  seeding, protocol skew window honored (no forced restarts within N/N−1);
  skew CI extended to the bridge path.
- Worktree-native flow: `amx work <branch>` = worktree + workspace + agent;
  `amx work done` collapses all three.
- Declarative sessions: `amx layout export` (capture live session to file) and
  `amx apply layout.toml`.

**Exit:** upgrade amx under 5 running agents — none die, no visible screen
content lost, waits keep waiting. Attach to the home machine from a laptop
over SSH.

## M4 — Polish & ecosystem

- `amx api schema`, stable v1 API declaration, docs generated from the method
  enum.
- More extension examples: a status-bar feeder (tmux/waybar), an editor jump
  helper.
- Registry stanzas for the long tail of agents (port herdr's 21, fusion where
  hook coverage measures out).
- Theme = six colors in config; OSC 10/11 host-theme sync into panes.
- Performance pass: damage-rate benchmarks, latency budget (key→echo round
  trip p99 < 5 ms local), flow-control tuning under adversarial output.
- Kitty graphics: implement the reserved graphics stream kind (passthrough to
  clients), or formally defer again with a written revisit condition.
- **Attention surfaces & small screens** (D14/D15,
  [10-attention-surfaces.md](10-attention-surfaces.md)): `agent.list` +
  last-line, per-workspace status-line breakdown, the agents view with live
  peek, `amx agents --watch`, workspace-scoped `next-attention`, narrow-
  viewport projection, wheel → copy-mode scroll.
- **Design-review register paydown**
  ([notes/design-review.md](notes/design-review.md)): M3 cleared the
  critical path (DR-1/2/3/8); what remains is DR-15 stale prose, DR-6
  ShortNumbers, DR-16's retriable-error-code decision, the DR-7/9/10 batch,
  DR-19 flakes, and the M3 residuals DR-17/18/20/21. The M4 plan adopts
  DR-4's standing integration owner + wave-1 live smoke and DR-5's leaner
  plan format.

**Exit:** a stranger can read the docs, extend amx with a shell script, and
add their agent with one registry stanza.

## Non-goals (revisit only with evidence)

- Windows port (behind platform traits when it comes)
- Any chrome mouse interaction
- Tabs (workspaces are the layer; see D13)
- Plugin manifests/marketplace
- Predictive local echo (capability-negotiated extension if ever)
- Web/GUI clients — though the smart-client protocol deliberately leaves the
  door open: a GUI client is "just" another protocol consumer.

## Standing risks

| Risk | Mitigation |
|---|---|
| libghostty-vt API churn (Superlogical now drives it) | vendored + patch manifests; upstream tracking is M0 work, benefits from their upstreaming commitment |
| Smart-client protocol is the novel part — most design risk | M0 proves it *including flow control*; golden + skew tests from M0 |
| Agent hook coverage worse than expected (herdr's revert is the precedent) | fusion never trusts hooks for exits; the M2 spike sets coverage classes from measurement; tier-2 manifests remain fully capable alone |
| Agent UIs change under tier-2 manifests | catalog updates; fusion means hooks still catch entry edges even when a manifest lags |
| Scope creep toward herdr's surface | the non-goals list + module budgets are the contract; new chrome needs a doc PR first |
