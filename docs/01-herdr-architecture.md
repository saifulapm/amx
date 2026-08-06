# How herdr works — subsystem map

Findings from a full source exploration of `herdr` (Rust, ~235 files, ~8 MB of
`src/`). Eight parallel readers mapped every subsystem; file references point at
the herdr checkout in `herdr/`.

## 1. Process model & core architecture

One binary, three roles:

1. **`herdr server`** — a detached daemon (setsid, null stdio) that owns all
   PTYs, terminal state, agent detection, and persistence.
2. **`herdr` / `herdr client`** — a thin client per attached terminal. `herdr`
   with no args probes the per-session Unix socket, daemonizes a server if none
   answers, then attaches.
3. **CLI subcommands** — one-shot newline-JSON requests to the API socket.

Two sockets per session: `herdr.sock` (newline-JSON API + event subscriptions)
and `herdr-client.sock` (length-prefixed bincode binary protocol,
`PROTOCOL_VERSION = 19`). Single instance per session is enforced socket-side:
a connect probe distinguishes live sockets (AddrInUse rejection) from stale
files (ConnectionRefused ⇒ safe to remove) — no lock files.

The server core is a single-owner async event loop (`HeadlessServer::run`,
`src/server/headless.rs`, **10,689 lines**) that `tokio::select!`s over the App
event channel, API request channel, client transport events, a coalesced render
notify, and timer deadlines, draining phases in a fixed order each tick. Around
it live plain OS threads: JSON-API listener + thread per connection, two threads
per binary client (reader + writer), and one `herdr-pty-<id>` poll(2) thread per
pane. Agent detection runs as an abortable tokio task per pane.

**Rendering is server-side**: each frame the server renders the *entire TUI*
into a virtual ratatui buffer (TestBackend) per client size, then diffs against
a per-client baseline and streams either `SemanticFrame` (structured cell grid)
or `TerminalAnsi` (server-side ANSI diff — used over SSH). Clients are dumb
blitters with zero app logic.

Notable mechanisms verified in code:

- **Dual-priority client writer queue**: unbounded reliable control lane +
  single-slot droppable render lane, so a slow client can never accumulate frame
  backlog (`src/server/client_transport.rs`).
- **Foreground-client concept**: the most-recently-active client owns shared
  host-coupled state (pane size, theme, keybindings); others get read-mostly
  renders — herdr's answer to tmux's "smallest client wins".
- **Live update handoff**: old server passes every PTY master fd to a freshly
  spawned successor over SCM_RIGHTS with a restored/ready/committed/owned
  handshake; agents survive the binary swap (`src/server/handoff.rs`).
- **Legacy escape hatch**: `--no-session` runs a pre-server monolithic TUI
  reusing the same `App` type — a second, drifting implementation of the same
  drain-phase loop.

## 2. PTY & terminal emulation

- Every pane has a real kernel PTY (vendored portable-pty; ConPTY on Windows)
  owned by a **dedicated per-pane I/O actor thread** running a poll(2) loop over
  the master fd + a self-pipe wake fd. Reads, writes, resize, and handoff all
  serialize on that one thread — the quiesce→dup→release handoff state machine
  is race-free by construction.
- VT emulation is **vendored libghostty-vt** (Ghostty's Zig terminal core) via
  bindgen FFI: kitty keyboard/graphics, mode 2027 graphemes, paged byte-limited
  scrollback (default 10 MB/pane), dirty tracking. Vendoring discipline is
  exemplary (`vendor/*.patches.md` + reverse-apply CI checks).
- A `response_order` mutex spans the read callback and out-of-band writes so
  terminal query replies (DA/DSR/OSC color/XTGETTCAP) are emitted in exactly the
  order a real terminal would produce them; herdr can substitute theme-aware
  OSC 10/11 replies for libghostty's.
- Resize is latest-wins coalesced through shared state, not the command queue.
- Windows is a parallel 4-thread ConPTY implementation sharing shape, not code,
  with workaround caches and **no handoff support** (`#[cfg(unix)]` throughout).

## 3. UI, rendering, input

- ratatui 0.30 + crossterm, but never against a real terminal in the normal
  path — see server-side rendering above.
- Strict **compute/render split**: `compute_view()` mutates all geometry and
  snapshots every clickable rect into `AppState.view`; `render()` draws from
  `&AppState` only. Mouse hit-testing runs against the previous frame's rects.
- Layout is a serializable **BSP split tree** per tab; workspaces hold tabs.
- Render triggers flow through a coalescing `RenderSignal` (origin-tracked per
  pane, so invisible-pane output skips rendering entirely) throttled to ~60 fps.
- A modal `Mode` state machine (Terminal/Prefix/Navigate/Copy/Resize/overlays)
  drives both key dispatch and overlay rendering. Prefix key → tmux-style
  bindings; every non-terminal mode has a dedicated key handler.
- Substantial mouse chrome: clickable tabs, workspace cards, drag-resize on
  split borders, scrollbars, selection — all hit-tested against ViewState rects.
  Desktop and mobile layouts fork into parallel code paths.
- Two theme domains: a ratatui `Palette` for chrome (preset gallery + live
  settings editor) and the probed host terminal theme (OSC 10/11 + mode 2031),
  forwarded into each pane's emulator so nested TUIs see real colors.

## 4. Agent-native layer (the differentiator)

Four pillars:

1. **Detection** — identity from the OS process tree (foreground process group,
   unwrapping interpreter/shell wrappers, 21 known agent CLIs); state
   (Idle/Working/Blocked) from **TOML manifest rules screen-scraping a dedicated
   bottom-of-screen snapshot** (priority + region + AND/OR/NOT gates of
   contains/regex matchers), never the user-scrollable viewport. Manifests are
   three-source (bundled < remote catalog from herdr.dev < local override) and
   debuggable via `herdr agent explain --json`.
2. **Hook integrations** — herdr installs hook scripts into each agent's own
   hook system; they report lifecycle state and session identity back over the
   socket. Three-tier authority: full-lifecycle hooks own state for 6 agents;
   for Claude Code/Codex/etc. hooks are **identity-only and screen-scraping owns
   state**; some agents are identity-only with no state channel.
3. **Agent resume** — hook-reported session refs are validated (argv built as
   data, never shell text; hardcoded source/agent allowlists) and persisted;
   after restart herdr replays `claude --resume <id>`, `pi --session <path>`,
   etc., with dedupe reservations so duplicated panes can't both resume one
   conversation.
4. **Agent drivability** — panes get `HERDR_ENV=1`, pane/tab/workspace IDs, and
   the socket path in env; a bundled SKILL.md teaches agents the CLI
   (`herdr agent prompt --wait`, `wait --until blocked`, `pane read`, safety
   rules). Waits are implemented server-side as 100 ms polling loops.

## 5. API & protocol surface

- **JSON API socket**: one request line → response line(s), serde-tagged Method
  enum, thread per connection. Long-running verbs (`agent.wait`,
  `pane.wait_for_output`, `events.subscribe`) are polling compositions inside
  the connection thread — every tick re-dispatches reads into the single App
  channel. Events come from a 512-entry seq-numbered ring behind a Mutex.
- **Binary client socket**: bincode frames, strict version equality both
  directions (older AND newer clients rejected); wire types decoupled from
  ratatui (packed u32 colors, u16 modifiers).
- **A third surface** exposes observe/control terminal streams through the
  binary protocol, with a CLI stdio adapter (`herdr terminal session observe`)
  that translates the binary frames to/from newline-JSON envelopes for
  programmatic drivers — same socket framing and version, adapter on stdio.
- **Remote**: the binary protocol verbatim over `ssh … exec herdr
  remote-client-bridge` stdio — no ports, no remote daemon protocol; the local
  side auto-installs a matching remote binary and forces the ANSI-diff encoding.
- Machine-consumable contract: every schema type derives JsonSchema; the
  generated `herdr-api.schema.json` is committed and embedded in the binary.

## 6. Persistence & session state

- `session.json` (versioned, pretty JSON): workspaces → tabs → BSP layout →
  per-pane cwd/labels/agent identity/**agent conversation refs** + sidebar UI
  state. Scrollback lives in a separate `session-history.json`, opt-in
  (secrets concern), aligned to the session file positionally at the
  workspace/tab levels, with panes keyed by pane ID within each
  positionally-matched tab.
- Saves are debounced 5 s, captured on the main thread, serialized/written on a
  background thread; atomic tmp+rename with manual symlink-chain resolution —
  but **no fsync anywhere**.
- Restore allocates fresh pane IDs (old→new remap), respawns shells in saved
  cwds (fallback HOME), executes agent resume plans once geometry/theme are
  known, prunes panes that fail to spawn (log-only), and reconciles the
  user-visible pane/tab numbering with max+1 counters.
- Worktree membership is validated on restore (checkout still exists, git key
  matches) and degrades to a plain workspace otherwise.

## 7. Plugins, config, integrations

- Plugins are directories with a TOML manifest declaring argv commands (never
  shell) for build/startup/actions/event-hooks/panes/link-handlers, installed
  from GitHub (`topic:herdr-plugin` marketplace crawled by a Cloudflare Worker)
  with an install-time consent prompt and a manifest-immutability check after
  build. **No sandboxing** — full-privilege child processes auto-run on server
  events; they talk back through the socket API via injected env.
- Config is TOML with two-tier parsing: strict on startup, per-section lenient
  on live reload (a typo never nukes a session). Reload is pull-only (CLI/
  keybind), no file watcher.
- 16 per-agent integration asset blocks (hook scripts in sh+python, powershell,
  typescript) with version markers so `herdr integration status` can flag stale
  hooks after updates.
- Self-update: channel manifests, sha256 verification, package-manager
  detection (brew/mise/nix redirect), live handoff negotiation, product
  announcements piggybacked on the update manifest. `src/update.rs` is 3,493
  lines.
