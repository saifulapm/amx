# amx — a minimal agent multiplexer

**amx** is a ground-up redesign of the "runtime your coding agents live on" idea
pioneered by [herdr](https://herdr.dev): a background server that owns your agents'
terminals so they survive disconnects, reboots, and upgrades — rebuilt with a
minimal, keyboard-only philosophy and an event-driven architecture that fixes
herdr's structural weaknesses.

## Documents

| Doc | Contents |
|---|---|
| [01-herdr-architecture.md](01-herdr-architecture.md) | How herdr actually works — full subsystem map from source exploration |
| [02-herdr-critique.md](02-herdr-critique.md) | Verified weaknesses, what herdr gets right, what must be kept |
| [03-vision.md](03-vision.md) | What amx is, design principles, the name, the competitive landscape (tmux/zellij, herdr, Superlogical) |
| [04-architecture.md](04-architecture.md) | The amx design: process model, event bus, protocol, agent layer, persistence |
| [05-roadmap.md](05-roadmap.md) | Milestones and build order |
| [06-m0-plan.md](06-m0-plan.md) | M0 execution plan: crate/module map, shared contracts, task DAG, waves, risks |
| [07-m1-plan.md](07-m1-plan.md) | M1 execution plan: durability — fsynced snapshot, restore + loss report, Persist actor, config hot reload, crash suite |
| [08-m2-plan.md](08-m2-plan.md) | M2 execution plan: agents — hook-coverage spike, registry, fusion state machine, AgentHub, attention queue, waits, resume |
| [09-m3-plan.md](09-m3-plan.md) | M3 execution plan: continuity & reach — wedge spike, SCM_RIGHTS handoff, reconnect-resync, self-update, SSH bridge, worktrees, layouts |

## TL;DR

- **Keep herdr's bones**: server owns PTYs + terminal state (libghostty-vt), thin
  attach/detach, agent status, session resume, live binary handoff.
- **Fix its structure**: one event-driven runtime (no polling anywhere, no dual
  monolithic/headless modes), one IPC surface with version skew tolerance, one
  declarative agent registry, fsynced UUID-keyed persistence.
- **Smart-client rendering** (the Superlogical insight): the server streams pane
  *grid deltas*, not pre-rendered UI frames; the client owns chrome and
  scrolling locally.
- **Minimal by construction**: keyboard-only chrome — no mouse UI, no sidebar, no
  right-click menus, no settings dialogs, no sounds, no announcements. Config is a
  file. The UI is panes + a status line. Mouse events are only *forwarded* to
  apps that ask for them.
- **Agent-first features herdr lacks**: zero-latency waits (awaits, not 100ms
  polls), an attention queue you cycle with one key, and hook-emitted status for
  Claude Code/Codex *fused* with screen detection — hooks assert the edges they
  can see reliably; the screen confirms the transitions hooks structurally miss.
