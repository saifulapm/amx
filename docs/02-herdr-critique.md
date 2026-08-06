# herdr critique — weaknesses to solve, strengths to keep

Every claim below was verified against herdr's source during exploration
(line counts, grep counts, and file references checked by a critic pass).

## Weaknesses

### Critical

**W1 — Everything waits by polling; no push primitive.**
`agent.wait`, `pane.wait_for_output`, and `events.subscribe` each spin a
detached thread re-dispatching `pane.read`/`pane.get` into the single App
channel every 100 ms (`CONNECTION_POLL_INTERVAL`). The EventHub is
`Arc<Mutex<Vec<(u64, EventEnvelope)>>>` capped at 512 entries — silent drops
under burst, up to 100 ms latency per event, and N waiting agents multiply load
on the runtime loop. For a tool whose pitch is agents waiting on agents, the
wait path is the weakest hot path in the system.

**W2 — God-object server loop with hand-tracked render dirtiness.**
`src/server/headless.rs` is 10,689 lines. The
`needs_render`/`needs_full_render`/`needs_graphics_render` trio appears 94
times, mutated across ~10 drain sites with parallel `*_with_render_impact`
variants. Dirtiness is convention, not structure; a missed site yields stale
frames or render storms.

**W3 — Zero protocol skew tolerance across overlapping IPC surfaces.**
Strict version equality in both directions; wire.rs comments say backward
compatibility is "not yet supported". Every protocol bump simultaneously breaks
all remote SSH hosts and running servers, forcing an interactive
stop-or-handoff flow through the 3,351-line `remote/attach.rs`. Three
programmatic surfaces exist (JSON API, bincode client protocol, and a CLI stdio
JSON adapter over the binary observe/control mode) — two of which maintain
independent framing/versioning schemes.

### High

**W4 — Dual-mode runtime.** The legacy `--no-session` monolithic loop and the
headless loop reimplement the same drain-phase semantics side by side, with
parallel key-dispatch variants (`handle_*_via_api`) already drifting.

**W5 — Screen-scraping is authoritative for the most popular agents.**
Claude Code and Codex hooks are classified identity-only, so Working/Blocked/
Idle for them comes from regexes over UI text (braille spinner classes, `❯`
prompt chars) that break when agent UIs change. Important history: herdr
*shipped* hooks-authoritative state for Claude Code and Codex in 0.3.0 and
deliberately reverted to identity-only (CHANGELOG 0.3.0 → the "session identity
only" reversion; `HOOK_REMOVALS` in `src/integration/claude_settings.rs` strips
the old state hooks on install) because the agents' hook systems miss
transitions — Esc interrupts and permission-dialog cancels emit no hook event,
and subagent Stop events falsely idled the parent pane. So the weakness is
real, but the naive fix (hooks-authoritative) is disproven; any successor needs
hook+screen fusion, not inversion. The orchestrating per-pane detection loop is
~400 lines with ~20 mutable locals; authority whitelists are split across two
files with subtly different sets.

**W6 — Hand-synced enumeration fan-out.** Adding one agent touches ~10 match
sites across 4 files (plus a Python re-implementation of the manifest grammar
in `scripts/`; separately, the marketplace worker re-implements
plugin-manifest parsing in TypeScript). Adding one API method touches
4 exhaustive lists; forgetting the `request_changes_ui()` whitelist silently
yields a stale UI. Consistency is enforced only by tests.

**W7 — Unsandboxed plugins.** Full-privilege child processes auto-triggered by
server events, gated by a single install-time confirmation, discoverable via a
bare GitHub topic. One confirmation away from arbitrary code execution.

**W8 — Persistence has silent-corruption and silent-loss modes.** Two
sequential writes (session + history) are individually atomic but not jointly;
history alignment zips workspaces and tabs by positional index (panes are
keyed by pane ID only within the positionally-matched tab) — a workspace/tab
desync silently drops history, and cross-restart pane-ID reuse can replay
scrollback into the wrong pane. No fsync anywhere in `src/persist`. Restore
drops failed tabs/workspaces with only a `warn!`. Up to ~5 s of layout changes
lost on hard crash.

### Medium

**W9 — Bespoke-bridge threading museum.** Async loops atop plain OS threads
bridged by blocking_send/std mpsc/Condvars; detached threads never joined;
process-global OnceLock registries; session/socket resolution via env vars +
an AtomicBool read at many call sites (tests need an env mutex).

**W10 — PTY hot path contention + FFI drift.** The full tracker+parser pipeline
runs under one `GhosttyPaneCore` mutex that render, detection, snapshot, and
input encoding all contend on. Resize embeds heuristic recovery (replay ANSI if
the bottom went blank) encoding empirical libghostty reflow bugs. The FFI
surface drifts two ways: the bindgen-generated `bindings.rs` is committed with
no regeneration check against re-vendored headers, and `ghostty/mod.rs`
hand-copies a few enum values on top (`TERMINAL_DATA_COLOR_FOREGROUND = 18`,
`KITTY_PLACEMENT_DATA_* = 3/10/11`), aligned by eye.

**W11 — Windows is a parallel implementation, not a port.** cfg-forked accept
paths, input models (2,499-line windows_vti.rs), PTY actors, inline named-pipe/
DACL code, and no handoff. Two behavioral shapes kept equivalent by hand.

**W12 — Test placement inflates god files.** ~2,800 inline `#[test]` functions
drive files past 10k lines (while `tests/` holds ~17.5k lines across 8 files —
integration coverage exists, but the inline share dominates and inflates the
god files); timing-sensitive paths rely on wall-clock races.

### Bloat (fine for herdr, wrong for a minimal tool)

- Mouse-first chrome: clickable tabs/workspace cards, drag-resize borders,
  scrollbars, hover states — plus a parallel *mobile* layout codepath.
- Sidebar with workspace cards and section splits (its widths are even
  persisted in the *server's* session snapshot — a boundary violation herdr's
  own contributor docs flag).
- In-TUI settings editor with live theme preview/rollback; a theme preset
  gallery.
- Embedded mp3 sounds played by spawning OS media players.
- Product announcements fetched via the update manifest.
- The notification stack's heavy tiers: in-app toasts and spawned OS notifiers
  (`[ui.toast] delivery = herdr|system`, terminal-notifier/osascript). *Not*
  bloat: the `terminal` delivery tier — client-emitted OSC 9/99 escapes that
  reach the host terminal even over SSH (`terminal_notify.rs`) — is tiny,
  chrome-free, and serves the core "tells you which agent needs you" promise;
  it is a keeper candidate.
- A Cloudflare-Worker marketplace crawling GitHub.
- The `--no-session` legacy mode.

## What herdr gets right — keep these

- **K1 — The client/server split.** Server owns all PTYs, terminal state, and
  persistence; clients are thin. Right foundation for detach/reattach,
  multi-client, SSH remoting, agent survival.
- **K2 — Real terminal core.** Kernel PTYs per pane + libghostty-vt instead of
  a homegrown VT parser; query correctness (DA/DSR/XTGETTCAP/OSC) comes from
  Ghostty's battle-tested core. Keep the vendoring discipline too.
- **K3 — Live binary-upgrade handoff** via SCM_RIGHTS fd transfer with a staged
  commit handshake. Agents surviving a multiplexer upgrade is genuinely
  differentiating.
- **K4 — Ordered terminal-response injection** and the per-pane single-threaded
  PTY actor (race-free quiesce/dup/release by construction).
- **K5 — The layered agent-authority concept** (lifecycle hooks > identity-only
  hooks > screen detection), agent resume from persisted session refs, argv
  built as data with allowlists, and hot-reloadable detection manifests with
  `agent explain` debuggability — the *concept* is right even where the
  implementation sprawls.
- **K6 — The modal Mode state machine** (Terminal/Prefix/Navigate/Copy driving
  both key dispatch and overlay rendering). The other half of herdr's UI
  pattern — the compute/render split with hit-rect snapshotting — exists to
  serve mouse chrome and deliberately dies with it in amx (D9).
- **K7 — Protocol safety rails**: length-prefixed framing with hard size caps;
  wire types decoupled from the UI library.
- **K8 — SSH remoting as a byte bridge** (no ports, no daemon protocol) with a
  bandwidth-friendly encoding negotiated at handshake.
- **K9 — UX details**: auto-detect-then-daemonize (`herdr` just works),
  per-session socket namespacing, stable user-visible pane numbering across
  restarts, sha256-verified self-update that knows its package manager,
  config hot-reload that never nukes a live session on a typo.
- **K10 — Agents as first-class API consumers**: env-injected identity, bundled
  skill teaching the CLI, machine-readable API schema embedded in the binary.
