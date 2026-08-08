# Attention surfaces & small screens — D14/D15

Status: designed, not scheduled (targets M4 alongside the fix register in
[notes/design-review.md](notes/design-review.md)). This doc amends the letter
of two earlier decisions — D9's "mouse forwarded, never interpreted" gains one
wheel exception, and the status line grows a per-workspace breakdown — so per
HACKING.md it exists as a doc before any implementation PR.

**The scenario that forces it:** 5 workspaces, 5 agents each — 25 agents. The
shipped surfaces answer "how many need me" (`⚑N`) and "take me to the next
one" (`next-attention`, global, block-time order). They do not answer *"which
agents, of which project, are blocked on what?"* without jumping. At 25 agents
that missing survey-before-dive moment turns `next-attention` into a treadmill
that yanks focus across projects in block-time order. Separately: attaching
from a phone's SSH terminal (a real daily workflow) meets a client that
assumes a wide viewport and a keyboard with easy modifiers.

Prior art audited before this design: Claude Code's agents view (research
preview, v2.1.139+; `claude agents`, peek panel, `ctrl+s` grouping,
`ctrl+x` stop, Haiku-generated row summaries every ~15 s). What amx adopts,
and what it deliberately rejects, is called out inline below.

---

## D14 — small screens, and the wheel

### Narrow-viewport policy

Below `client.narrow_cols` (config, default 60 columns), the client switches
its *own projection*, and declares what that projection is showing (04 §3
already gives every client an independent view — see the amendment below):

- **Single-pane projection**: the focused pane fills the viewport (the
  degenerate one-pane layout `amx attach --pane` already renders). Pane
  switching goes through the picker; split navigation keys still work, they
  just change which single pane is shown.
- **Compact status line**: global `⚑N` + active workspace name + queue head
  only. The full per-workspace breakdown (D15) needs width it doesn't have.
- **Picker and agents view render full-screen** instead of as an overlay
  region; peek (D15) replaces the list rather than sharing the screen.

**Amendment (M4 research, D-M4-7 / R-M4-2): this is not client-side only, and
"the server and pane grids are untouched" cannot hold.** As first written this
section said the server, other clients and pane grids were untouched. Read
against the tree, that produces the opposite of the phone case it exists for:

- `Core` holds one viewport, `Option<(u16, u16)>`
  (`amx-server/src/actor/core/mod.rs:140`), set from the declaring client's rows
  and cols alone — `handle_viewport` (`actor/core/view.rs:144-157`) reads
  `params.rows` and `params.cols` and nothing else.
- `reconcile_pane_sizes` (`actor/core/view.rs:184-221`) projects the **whole
  layout** into that viewport and resizes every pane's PTY to its slot's
  interior.
- `client::Viewport.panes` — documented as "Panes visible in this client's
  projection … The server sends grid traffic for these panes and no others"
  (`amx-proto/src/control/client.rs:22-24`) — has no server-side reader
  anywhere.

So a 60-column client showing one pane full-screen would still declare a
60-column viewport against a two-pane layout, and the server would size that
pane to roughly 28 columns. The client would letterbox a 28-column grid inside
60 columns of terminal: correct by 04 §3's letterbox rule, and useless.

**The amended policy:** the narrow client declares what it is actually
showing — `Viewport{rows, cols, panes: [the one pane]}` — and the server's
projection gains one rule: *a viewport declaring a single pane sizes that pane
to the whole content area*, rather than to its slot in a layout the declaring
client is not drawing. That is also how `Viewport.panes` finally gets the reader
it was frozen with (R-M4-14).

What does **not** change: the pane grid still follows the most-recently-active
client (04 §3, and `core/mod.rs:138-139` says so in the field's own comment), so
a phone attaching to a session a desktop is also watching does resize its panes.
That trade is 04's and is not made worse here. Nothing in 04 is contradicted:
§3 already gives the server the client's projection as the definition of
visibility.

Above the projection change, it is a rendering policy keyed on width.

### The wheel exception

Mouse handling gains exactly one interpreted event class. For a pane whose
application requested mouse reporting, SGR events are forwarded unchanged
(D9). For a pane that has not: **wheel-up enters copy mode and scrolls the
local cache; wheel-down at the live edge exits copy mode.** Nothing else —
no clicks, no taps, no drag, no hit-rects, ever. On a phone terminal,
touch-scroll arrives as wheel events, so this one concession is what makes
scrollback reachable by touch.

**Amendment (the M4 spike, outcome (b)): it ships opt-in, off by default.** As
first written this section added "on a desktop it is the concession every trial
user reaches for in the first minute", which assumed the wheel was free. It is
not. Receiving a wheel event means asking the host terminal for mouse tracking,
and both emulators the spike measured document what that costs in their own
manuals: with an application holding the mouse, ordinary drag-select becomes
shift-drag (foot's `selection-override-modifiers`, alacritty's `Shift`
suppression). The cost is paid all the time rather than where it buys
something, because the exception exists precisely for panes that did *not* ask
for the mouse — so the request cannot be scoped to the panes that did, which is
what a multiplexer mirroring a pane's own request can do and this cannot.
`mouse.enabled` therefore defaults **off**, and the phone profile is where it is
turned on. Evidence, measured rather than assumed:
[notes/m4-mouse-path.md](notes/m4-mouse-path.md) §2.2, §2.3 and §5, including
the one hypothesis that note labels as still unobserved.

**And what the exception buys is smaller than this section assumed.** DEC mode
`1007` (alternate scroll) is *set by default* in both emulators the spike
measured, observed over DECRQM before amx asked for anything, and amx runs on
the alternate screen (`ALT_SCREEN_ENTER`, `amx-client/src/term.rs:21`). So a
wheel turn over a running `amx attach` already produces cursor-up/cursor-down
keys that the client forwards to the focused pane. "The wheel does nothing" was
never true. The exception buys an **unambiguous** wheel — scroll that reaches
scrollback rather than whatever the focused application does with arrow keys —
not a wheel that works at all. It is a smaller prize, bought at the cost above,
which is the whole reason the default moved.

This amends 03 §Design principles 1 ("otherwise ignored" → "otherwise only
wheel events are interpreted, as copy-mode scroll, and only when the user turns
mouse tracking on") and the letter of D9. The
fence stays: any *positional* mouse interpretation (anything requiring a
hit-rect) remains out, permanently.

### Modifier-hostile keyboards

Phone keyboards make `ctrl+a` awkward. The deliverable is unchanged: a
documented `[keys]` phone profile example (alternative prefix, e.g. a function
key or double-tap sequence the user's terminal app can emit) in the config
docs.

**Amendment (M4 research, D-M4-8 / R-M4-3): the profile is code before it is
documentation.** As first written this section said "No new mechanism:
keybindings are already config data and clients may bring local bindings at
handshake … Documentation work, not code." Every clause of that is false in the
tree:

- The prefix key is a constant, not config: `pub const PREFIX: u8 = 0x01;`, and
  its own comment says "configurable once config lands in M1"
  (`amx-client/src/input/mod.rs:57-58`). M1, M2 and M3 landed and it did not.
- The prefix table is a `match` on byte literals
  (`amx-client/src/input/mod.rs:306-337`), not data.
- There is no `[keys]` section. `config::SECTIONS`
  (`amx-core/src/config/mod.rs:44-49`) is `persist`, `terminal`, `update`,
  `work` — and that module's own rule is that "a section nobody reads is a
  promise nobody keeps" (`config/mod.rs:41-43`).
- `amx-client` reads no configuration at all: the crate contains no reference to
  `Config`.
- `client::Keybindings::{Server, Local}`
  (`amx-proto/src/control/client.rs:40-46`) has no reader anywhere in the tree —
  it is one of the frozen-ahead-of-a-reader fields R-M4-14 collects.

**The amendment:** M4 builds the minimum the profile needs and no more — a
`[client]` section (the narrow threshold this doc already assumes) and a
`[keys]` section carrying the prefix key and the prefix table's bindings, read
client-side through the `Ctx.config_path` the CLI already builds
(`amx-core/src/ctx.rs:199`), plus `amx keys` to print the resolved table, which
04 §7 promises. No dependency enters the tree: `amx-core` already carries `toml`
and `amx-client` already depends on `amx-core`. 04 §7 is not contradicted — it
specified exactly this mechanism, and it was never built. The documentation then
describes something that exists.

### The tier-3 fence

A real touch client — tap to switch, swipe, native rendering — is **not TUI
chrome and never will be**: it is a separate client speaking the protocol
(the door 05 §Non-goals already leaves open). herdr's parallel mobile layout
codepath is the documented counterexample (02 §Bloat). Nothing in D14 may
grow toward it inside `amx-client`.

---

## D15 — attention surfaces

One principle: **the attention queue stays global and block-time-ordered**
(the longest-blocked agent anywhere is the head — fairness is the server's
job); **every display surface groups by workspace** (orientation is the
eye's job). Three surfaces, one data source.

### The data source: `agent.list`

One new control method. Response (shape, not schema — the derive pipeline
owns wire names):

```jsonc
{
  "seq": 41023,              // bus seq at capture — the standard resync rule
  "attention": ["<pane-uuid>", …],   // queue order, head first
  "agents": [{
    "workspace": { "id": "<uuid>", "name": "api" },
    "pane": "<uuid>",
    "name": "backend",       // agent name = pane label (D-M2-9)
    "kind": "claude",        // registry id; absent for tier-3 unknowns
    "status": "blocked",     // working | blocked | idle | busy | quiet
    "reason": "permission",  // blocked only: permission | idle_prompt | …
    "since": 1754650000000,  // ms epoch of the current status's entry edge
    "last_line": "Allow Bash(git push origin main)? (y/n)"
  }, …]
}
```

`last_line` is **the literal last non-empty visible row** of the pane's
published POD cell snapshot, SGR-stripped and trimmed — never scrollback,
never an interpretation. It is read off the snapshot (the lock-free copy that
already serves render and tier-2 detection), so it costs the parser thread
nothing. Consumers refresh it on the pane's coalesced damage events,
debounced to ≤ 4 Hz per pane, so 25 flooding agents cannot turn any surface
into a firehose. Push, never poll.

**Fence (rejected from Claude Code's design): no generated summaries in
core.** CC pays one Haiku call per working session per ~15 s for its row
summaries — a token bill and a 15 s staleness window inside a monitor. The
literal line plus the fusion `reason` covers the same question for free at
damage latency. Semantic summaries are an *extension*: subscribe to events,
`pane read`, call any model, render wherever. The server never calls out.

`attention_enqueued` / `attention_dequeued` events carry the same identity
block (`workspace{id,name}`, `pane`, `name`, `reason`, `since`) so a notifier
can say "api/backend blocked (permission request)" without a follow-up query.

### Surface 1 — the status line

```
[api ⚑2] [web ⚑1] [infra] [docs ⚑2] [exp]   ⚑5  api/backend 4m
```

Per-workspace segments (flag count only when nonzero, active workspace
highlighted), then the global count and the queue head with its age. One
glance answers *which projects need me and who has waited longest*. Client
chrome over pushed state; narrow viewports collapse to the D14 compact form.

### Surface 2 — the agents view

The picker primitive with one extension: **entries may carry a live detail
line, and the view may reserve a live peek region.** It is still a fuzzy
list — Enter jumps, Esc closes, typing filters — not a second dialog type.
That sentence is the whole license; anything further (scrolling panes inside
the picker, tabs, forms) is out.

```
agents — 25 · ⚑5
api/backend     ⚑ blocked   permission  4m │ Allow Bash(git push origin main)? (y/n)
docs/writer     ⚑ blocked   permission  7m │ Allow Write(chapter-3.md)? (y/n)
web/frontend    ⚑ blocked   permission  2m │ Allow Bash(npm install)? (y/n)
api/tests       ● working               2m │ Running cargo test --workspace…
infra/deploy    ● working              11m │ Applying terraform plan (3 of 7)…
12 idle
```

Keymap (defaults; all rebindable, introspectable via `amx keys`):

| Key | Action |
|---|---|
| type… | fuzzy filter (matches `workspace/name`) |
| `↑`/`↓` | move selection |
| `Enter` | jump to the agent's pane |
| `Space` | live peek of the selected pane (read-only) |
| `Esc` | close peek, else close view |
| `ctrl+s` | grouping: workspace ↔ state (CC's chord, kept for muscle memory) |
| `ctrl+b` | blocked-only filter on/off |
| `ctrl+p` | prompt the selected agent inline (`agent.prompt`) without attaching |
| `ctrl+x` | kill the selected pane — press twice to confirm (no confirm dialog exists; CC's twice-pattern, adopted) |
| `ctrl+r` | rename the selected agent |

Semantics:

- **Ordering** within groups: blocked (oldest first) → working → idle. The
  top row is always "who needs me most".
- **Idle collapse** (adopted from CC): more than 3 idle agents in a group
  collapse to an `N idle` row; Enter on it expands. Blocked and working rows
  are always individually visible.
- **Peek** binds a read-only grid stream for the selected pane (the
  viewport-visibility subscription already covers non-focused panes) and
  renders it in a reserved region beside/below the list — full-screen under
  D14's narrow policy. No input is forwarded; `ctrl+p` is the reply path.
- **View state persists per client** (filter, grouping, selection) —
  client presentation state stays client-side (04 §6). Reopening the view
  restores it; that is the "back to the board" motion. CC's `←`-to-return is
  deliberately **not** copied: CC's rows are transcripts, amx's are real
  terminals, and arrow keys belong to the applications in them.
- The **attention picker** from earlier drafts is this view with `ctrl+b`
  on — one surface, not two.

**Fence (rejected): the view is a monitor, not a launcher.** No inline
dispatch of new agents (CC has it; `amx agent start` and the worktree flow
own creation). No filter syntax (`s:working`, `a:name`) — fuzzy typing plus
`ctrl+b` is the whole grammar.

### Surface 3 — the CLI

```
amx agents                      # one-shot table (workspace, name, status, reason, age, last line)
amx agents --watch              # live full-screen view; q quits
amx agents --workspace api      # scope either form to one workspace
amx agents --json               # the agent.list response, verbatim
```

Same data, no client attach needed: a spare pane, a plain SSH window, a phone
(`amx agents --watch` is the read-only mission-control screen — D14's
cheapest deliverable). Live consumers that want a stream use
`amx events --json` plus re-query on `gap`, the standard contract; `--watch`
is that loop, packaged.

### Scoped cycling

`next-attention` stays global (head of queue). One new binding:
**`next-attention --workspace`** (workspace-scoped variant, default on a
neighboring key) cycles only the current workspace's blocked agents, so
clearing one project's queue never yanks focus across projects.

---

## What this deliberately leaves out

- LLM summaries, token/cost meters, model badges in core — extension
  territory (the API carries everything needed).
- PR/CI status in rows (CC shows PR numbers) — an extension may render its
  own column via a custom feed; the multiplexer does not know what a PR is.
- Any second dialog primitive, any positional mouse, any launcher surface.

## Decision table entries (for 04 §10)

| # | Decision | Replaces / amends |
|---|---|---|
| D14 | Narrow-viewport single-pane projection + compact status line; wheel-only mouse concession (wheel → copy-mode scroll in panes without mouse reporting); touch clients are separate protocol consumers, never TUI chrome | amends D9's letter; herdr's parallel mobile layout fork stays deleted |
| D15 | Attention surfaces: `agent.list` + identity-bearing attention events; per-workspace status-line breakdown; agents view as the picker's one extension (live detail line + peek region); `amx agents` CLI; workspace-scoped `next-attention`. Queue ordering stays global; grouping is display-only. No generated summaries, no launcher, no filter syntax in core | extends D2's status line and the picker primitive (03 §2) |

## Effort

| Piece | Estimate |
|---|---|
| `last_line` extraction + `agent.list` + event identity fields | ~2 days |
| Status-line breakdown (+ D14 compact form) | ~1 day |
| Agents view: detail column, grouping, idle collapse, `ctrl+b/p/x/r` | ~3 days |
| Peek region (non-focused pane stream + reserved-region render) | ~3–4 days |
| `amx agents` CLI (one-shot / `--watch` / `--json` share rendering) | ~1–2 days |
| D14 narrow policy + wheel → copy mode + phone profile docs | ~3–5 days |
| Scoped `next-attention` | ~0.5 day |

Roughly two weeks total. As of the M3 merge this depends on nothing open in
the fix register: the `Cells` codec (DR-1) and history delivery (DR-2) both
landed, so peek renders real styled grids and copy-mode scrolling over
fetched history works from day one.
