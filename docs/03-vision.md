# Vision — what amx is

## One sentence

**amx is the minimal, keyboard-only runtime your coding agents live on**: a
background server that owns agent terminals so they survive disconnects,
reboots, and upgrades — and tells you, instantly, which agent needs you.

## The landscape

**tmux / screen / zellij** — general-purpose multiplexers. Their model is the
dumb terminal: the multiplexer re-renders everything into the client terminal
and constantly reconciles local and remote state. No agent awareness at all.

**herdr** — proved the category: agent-aware panes (working/blocked/idle),
socket API agents can drive, session resume (`claude --resume`), live binary
handoff. But it grew mouse-first chrome, a sidebar, settings UIs, sounds,
announcements, a marketplace — and its internals accumulated structural debt
(see [02-herdr-critique.md](02-herdr-critique.md)): 100 ms polling everywhere,
a 10k-line server loop, three IPC surfaces with zero skew tolerance,
screen-scraping as the primary status source for the most popular agents.

**Superlogical** (Mitchell Hashimoto's new company, July 2026) — validates the
server-side-session direction from the other end. Their thesis: tmux-class
multiplexers are slow because they duplicate state and reconcile constantly;
the fix is server-owned sessions with **smart clients** (built on libghostty)
that render and scroll locally, sending input up like SSH. Sessions become "a
stateful session for the work itself," with human and agent work converging.
Closed development, VC-backed, multiplexer as the wedge for a much larger
platform.

**amx's position**: herdr's agent-native depth + Superlogical's smart-client
architecture insight + a minimalism neither of them has. Open, one Rust binary,
local-first, no platform ambitions — a sharp tool.

## Design principles

1. **Keyboard is the interface.** Every capability is reachable from the home
   row: prefix keys for commands, a modal navigate layer for movement (vi-like,
   kakoune-inspired). There is *no chrome mouse UI* — no clickable tabs, no
   drag-resize, no right-click menus, no scrollbars to grab, no hover states.
   Mouse events are forwarded to applications that request mouse reporting
   (vim, htop) and otherwise ignored. This is not a downgrade; it deletes
   herdr's hit-rect bookkeeping, ViewState coupling, drag state machines, and
   the entire mobile layout fork.
2. **The UI is panes + one status line.** No sidebar, no cards, no dialogs
   beyond a single command palette / picker primitive (fuzzy list, like a
   minimal fzf) that serves every "choose one of N" interaction: switch
   workspace, switch pane, pick an agent, run a command.
3. **Config is a file.** One TOML file, per-section lenient reload (herdr's
   best config idea, kept) — but amx adds a file watcher, where herdr's reload
   is pull-only (CLI/keybind). No in-TUI settings editor, no theme gallery — a
   theme is six colors in the config.
4. **Everything is the API.** The CLI, the status line, agents, and "plugins"
   all speak the same socket API. There is no plugin manifest system, no
   marketplace, no auto-installed event hooks: an extension is any program that
   subscribes to events and calls methods — run it yourself, supervise it
   yourself. This deletes herdr's biggest attack surface (W7) by construction.
5. **Push, never poll.** Status changes, waits, and subscriptions are channel
   awaits end to end. An agent's block→you-notified latency is microseconds,
   not 100 ms.
6. **Boring durability.** Fsynced atomic snapshots, UUID-keyed, restore reports
   its losses in the UI. Agents survive amx upgrades via fd handoff.
7. **Small enough to audit.** Hard module-size budgets (soft 500 / hard 1000
   lines, generated code exempt), integration tests in `tests/` driving the
   public API, no file like headless.rs can exist.
8. **Unix-first.** Linux and macOS are tier 1. Windows is out of scope until
   the design has a trait seam it can slot into cleanly (herdr showed what
   inline cfg-forking costs).

## What we deliberately do NOT build

- Mouse chrome, sidebar, settings UI, theme gallery, mobile layout
- **Tabs** — herdr's tree is workspace → tabs → layout; amx flattens to
  workspace → one BSP layout. Want another layout? Make another workspace —
  they're cheap, and the picker + prefix keys switch either way. (Migration
  note for herdr users: `prefix+c` new-tab and the `tab.*` API have no
  equivalent; workspaces take that role.)
- Sounds, product announcements
- In-app toasts and OS-notification spawning — out-of-terminal notification is
  an *extension* consuming the attention-queue event stream (a reference
  ~20-line notifier ships with the agent milestone). amx itself keeps exactly
  one built-in notify path: OSC 9/99 escapes emitted by the client to the host
  terminal (SSH-safe, chrome-free, a few dozen lines) on blocked-agent events.
- Plugin manifests, marketplace, auto-run event hooks
- A monolithic no-server mode
- Windows (initially)

## What we build that herdr doesn't have (or got stuck on)

- **The attention queue**: a server-maintained ordered queue of agents needing
  input. One key jumps to the next blocked agent; the status line shows the
  count. Herdr marks status per pane; amx makes "handle the next one" a single
  keystroke — the core loop of running many agents becomes muscle memory.
- **Zero-latency waits**: `amx wait --until blocked` returns the moment the
  status transition lands. Agent-to-agent orchestration gets fast enough to
  compose.
- **Fused agent status**: herdr shipped hook-driven state for Claude Code and
  Codex in 0.3.0 and later reverted them to identity-only, because their hook
  systems miss transitions — Esc interrupts and permission-dialog cancels emit
  no hook event, and subagent stop events falsely idled the parent pane (see
  herdr's CHANGELOG and the `HOOK_REMOVALS` list in `claude_settings.rs`). amx
  doesn't repeat either extreme: hooks assert the high-confidence edges they do
  see (turn start/stop, tool use, permission request) the instant they fire;
  screen detection confirms the transitions hooks can't see, with defined
  per-transition precedence. Faster than herdr's screen-only state, honest
  about what hooks can't observe.
- **Declarative sessions as the paved road**: herdr has `layout.export/apply`
  as API calls; amx makes the file the workflow — `amx layout export` captures
  the live session, `amx apply layout.toml` reproduces it. Check your agent
  topology into the repo.
- **Worktree-native agent spawning**: one command creates a git worktree,
  a workspace bound to it, and an agent running in it. Cleanup collapses all
  three. (herdr has the pieces; amx makes it the paved road.)
- **Smart-client rendering** (see [04-architecture.md](04-architecture.md)):
  local scrolling of a locally-cached scrollback, instant chrome feedback,
  bandwidth proportional to visible pane damage — not full-screen UI frames.

## The name

Requirements: short, typeable as a command a hundred times a day, pronounceable,
meaningful, not squatted by a major tool.

| Candidate | For | Against |
|---|---|---|
| **amx** | **a**gent **m**ultiple**x**er; 3 chars; reads like tmux's successor; the repo already bears it | opaque until explained (so was tmux) |
| herd | the flock-of-agents image, verb and noun | one letter from herdr — reads as a fork |
| flok | flock of agents, 4 chars | cutesy spelling; existing flok live-coding tool |
| pax | pane multiplexer, "peace" | common Unix name (tar's pax) — collision |
| qorral | corral — where you keep the herd | 6 chars, spelling ambiguity |

**Decision: amx.** Three letters in the tmux lineage, says exactly what it is
(agent multiplexer), and the muscle-memory test wins: `amx`, `amx attach`,
`amx wait --until blocked` all feel right.
