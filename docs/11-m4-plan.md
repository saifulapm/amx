# M4 execution plan

The build plan for the half of **M4** that this milestone runs: the D14/D15
attention surfaces ([10-attention-surfaces.md](10-attention-surfaces.md)) and
the paydown of the design-review register
([notes/design-review.md](notes/design-review.md)). Binding design is
[04-architecture.md](04-architecture.md) — §3 (the client's own projection and
the published snapshot), §5 (the agent layer and the attention queue), §7 (the
input model and the picker) and §4 (the wire's additive-field contract). Where
research contradicted 04 or 10, it is recorded in [§8](#8-risks--findings) as a
proposed amendment rather than silently redesigned.

**What this plan does not cover.** [05-roadmap.md](05-roadmap.md)'s M4 section
lists more than these two bodies of work: `amx api schema`, extension examples,
the registry long tail, theme-as-six-colours, the performance pass and the
kitty-graphics decision. None of them is in this plan or its task DAG. They are
not cancelled; they need their own plan, and R-M4-11 says so out loud rather
than letting a green M4 imply they shipped.

Everything below that states a fact about the amx tree, herdr, a crate or a
terminal is cited to `path:line` and was read during planning. The largest
unknown — whether any mouse byte can reach amx at all — is deliberately **not**
resolved here: that is X01, the mouse-path spike, and [§2](#2-the-mouse-path-spike-x01)
states why the wheel exception cannot merge over it and what happens under each
outcome.

Process lessons inherited and applied throughout: the milestone has a **standing
integration owner** from wave 1 with a **live smoke that runs from wave 1**, not
an integration task bolted on at the end (DR-4, [§6](#6-waves-file-ownership-and-the-standing-integration-owner));
the contracts task splits budget-edge files before the waves press them (07
R-M1-3, 08 R-M2-5, 09 R-M3-7); no field is frozen without its reader landing
inside the same milestone (R-M3-12, D-M4-10); and the milestone does not exit on
green tests — four times now a green suite has hidden a non-working feature, so
the exit is green tests **plus** the recorded live smoke of [§7](#7-the-m4-exit).

This plan states each task's scope, dependencies, owned files and what must be
true for it to be accepted. It deliberately does **not** pre-write acceptance
test names and carries no agent prompt drafts (DR-5): a name written before the
code invites satisfying the name over the intent, and the implementer picks the
tests.

---

## 1. Decisions taken during research

### D-M4-1 — The mouse path has never run end to end, so D14's wheel exception is spike-gated

04 §7 and D9 promise that "`mouse_forward = true` (default) forwards SGR events
to applications that enabled mouse reporting". The mechanism is designed, typed,
unit-tested — and unreachable in a running amx. Four facts, each read from
source:

- The client never asks its host terminal for mouse reports. Entering the
  terminal writes `ALT_SCREEN_ENTER` = `\x1b[?1049h\x1b[?25l`
  (`amx-client/src/term.rs:21`) and nothing else; no `?1000`, `?1002`, `?1003`
  or `?1006` appears anywhere in `crates/amx-client/src`. A terminal that was
  never asked sends no SGR report, so `mouse::scan`
  (`amx-client/src/input/mouse.rs:40`) has nothing to recognise in production.
- The per-pane gate is never fed. `Input::set_mouse_reporting`
  (`amx-client/src/input/mod.rs:180`) is what decides whether a report is
  forwarded or dropped (`app/actions.rs:73,81`), and its only callers in the
  whole tree are three lines of `amx-client/tests/input.rs:255,266,283`. Every
  pane therefore reads as "mouse disabled" to a live client.
- The server knows the answer and tells nobody. `Terminal::mouse_tracking`
  (`amx-vt/src/terminal.rs:380`) has **no caller at all**, and the handoff
  manifest reads the same modes generically (`?1000`, `?1002`, `?1003`, `?1006`
  are all in `CARRIED_MODES`, `handoff/manifest/modes.rs:111-116`) purely to
  carry them across an upgrade.
- No wire field carries it. `PaneState`
  (`amx-proto/src/control/session.rs:64-111`) has label, cwd, size, history and
  agent, and nothing about mouse reporting.

So D14 is not "one more interpreted event class" on a working path; it is the
first consumer of a path that has never carried a byte. That is exactly the
"spike first anything unverified" case, and X01 gates X13. [§2](#2-the-mouse-path-spike-x01)
has the protocol and the outcome tree.

### D-M4-2 — `agent.list` is answered by `Core`, out of state it already holds plus the pane feeds

Everything D15's response needs is already in one actor, except two fields
D-M4-3 and D-M4-4 add:

| Field | Where it already is |
|---|---|
| `workspace{id,name}` | `Core`'s state tree — `core/view.rs:52-62` walks workspaces with their labels |
| `pane`, `name` | `core/view.rs:75-82` (the pane's label) |
| `kind`, `status` | the hub's mirror into `Core` (`core/view.rs:100`, filled by `agent_hub/commit.rs:170-188`) |
| `attention` order | `Core.attention`, same mirror (`core/view.rs:117`) |
| `last_line` | the pane's published frame — `Core` owns the `PaneHost`s, and `SnapshotFeed::latest` (`actor/pane_host/mod.rs:406`) is an `Arc` clone out of a lock-free slot |

`last_line` is the literal last non-empty visible row, and reading it costs
nothing anyone has to think about: `Row::line()`
(`amx-vt/src/snapshot/mod.rs:141`) is documented as "a borrow of the row's own
arena: calling this allocates nothing and copies nothing", rows are stored
padded so `trim_end` is the whole of the trimming, and the scan runs from the
bottom (`Snapshot::grid`, `snapshot/mod.rs:197`). `pane.read` already serves the
visible grid this way, off the connection task, with "no parser round trip"
(`dispatch/pane.rs:199-219`).

**Decision:** `agent.list` is one `Core` mailbox round trip answered in a new
`actor/core/agents.rs`, not a second actor conversation and not a per-pane
`StreamCall::Wiring` fan-out (`dispatch/pane.rs:284` — that is one round trip
*per pane*, which at 25 agents is 25). Nothing new crosses an actor boundary
that does not already cross one.

### D-M4-3 — `reason` is the detector's own name, not a new taxonomy

10 §D15 writes `"reason": "permission"` with `// blocked only: permission |
idle_prompt | …`. An enum spelled that way is a second vocabulary that has to be
maintained per agent alongside the manifests. It is unnecessary, because the
detectors already name themselves:

- Tier 2 hands the winning rule's name to the hub on every evaluation —
  `ScreenVerdict { asserts, rule: Some(rule.name().to_owned()), .. }`
  (`actor/agent_hub/detect.rs:80-93`) — and the shipped names are already the
  words D15 wants: `permission_dialog`, `prompt_box_idle`,
  `footer_interrupt_hint_working` (`crates/amx-server/assets/manifests/claude.toml:47,99,63`).
- Tier 1 names itself too: `HookEvent::PermissionRequest`
  (`amx-proto/src/control/agent.rs:69`).

**Decision:** `reason` is an optional short string carrying the name of whatever
last moved the pane into its current state — the winning manifest rule for a
screen-owned state, the hook event for a hook-asserted entry, and absent for
tier-3 `busy`/`quiet`. The tracker does not retain it today (nothing in
`agent/fusion/tracker.rs` holds a rule name); retaining it is the change. This
keeps the "no generated summaries" fence of D15 intact — the string is the
detector's own identifier, not an interpretation — and it means a new manifest
rule is self-describing on the wire the day it is written, with no second table
to update.

### D-M4-4 — `since` is epoch milliseconds, and the reply carries the server's `now`

Nothing in the tree keeps a wall clock for an agent's status. `AgentSnapshot`
carries `transition_seq`, "the *transition*'s sequence, held until the next
transition replaces it" (`amx-core/src/agent/mod.rs:220-224`), and the hub's
timers are `tokio::time::Instant` (`actor/agent_hub/mod.rs:71,211-213`) — a
monotonic clock a test can pause, and one that means nothing to a client.

A bus sequence cannot be rendered as `4m`. So a wall-clock instant has to be
carried, and the obvious spelling — epoch milliseconds, as 10 §D15 writes it —
has a defect this milestone can see coming: m3-live-smoke §5 attaches from one
machine to another over SSH, and two machines' clocks are not the same clock. A
renderer computing `local_now − since` would show an age wrong by the skew.

**Decision:** `since` is epoch milliseconds (absolute, friendly to
`amx agents --json` and to any external consumer), **and** the `agent.list`
reply carries the server's own `now` in the same units. Every age a surface
renders is `now − since` from inside one reply, advanced locally by monotonic
elapsed time between refreshes. Nothing renders an age against its own wall
clock, so clock skew is unobservable. Two fields, one of them free, and the
alternative — carrying a derived `age_ms` — would have thrown away the absolute
instant every external consumer wants.

`since` is additive on `AgentSnapshot`, which means it rides the handoff
manifest for free (`Manifest.agents` is a list of `AgentSnapshot`s, W07's wave
outcome) — but the cold-restore path is not free, and R-M4-4 names it.

### D-M4-5 — `last_line` rides `agent.list` only, and never `session.state`

`session.state` already carries a per-pane `agent` block, and adding
`last_line` there would be less code than a new method. It is refused on two
grounds. First, cost of the wrong shape: every attached client folds the whole
of `session.state` on every resync (`amx-client/src/app/events.rs:129-192`) and
the reply already carries every workspace's layout tree — putting a line of
every pane's screen contents on it means every mirror, every golden and every
`amx session state` dump grows a copy of what is on screen. Second, cadence:
D15 refreshes the detail lines at up to 4 Hz, and `session.state` is the
expensive reply.

**Decision:** `agent.list` is the narrow, agent-only projection and is the only
place `last_line` appears. `session.state`'s `agent` block gains `reason` and
`since` (both small, both wanted by the status line, which must not need a
second call to render the breakdown) and nothing else.

### D-M4-6 — The identity block on attention events is folded from the bus, not asked for

D15 requires `attention_enqueued`/`attention_dequeued` to carry
`workspace{id,name}`, `pane`, `name`, `reason`, `since` so a notifier needs no
follow-up query. The hub publishes those events and is their only publisher
(`actor/agent_hub/mod.rs:15-17`, and D-M3-2's one-publisher-per-kind rule), but
the hub holds no labels: `Tracked` (`agent_hub/mod.rs:183-214`) has a title and
an argv, and `AgentHub.workspaces` (`mod.rs:278`) maps a pane to a workspace id
and no name.

The hub must not ask `Core` for them — parking on a sibling is what its shutdown
discipline forbids (`agent_hub/mod.rs:36-43`). It does not have to: **the hub
already reads the bus** and already folds pane facts off it —
`Event::PaneCreated{pane, workspace}` is where `workspaces` comes from
(`agent_hub/run.rs:196`) and `Event::PaneTitle` is folded beside it
(`run.rs:190`).

**Decision:** the hub keeps a small names mirror — pane label, workspace label —
seeded when a pane is handed over (`AgentCommand::PaneStarted`, and
`AgentHub::inherit`/`announce_inherited` on the import path) and moved by
`Event::PaneRenamed` and `Event::WorkspaceRenamed` folded in the same match arm
that already folds `PaneTitle`. Two new arms and one map; no new cross-actor
call, no ask, no change to the shutdown rules. R-M4-4 names the seeding paths
that have to be checked, because a label that only arrives by rename event would
be absent for every restored pane.

### D-M4-7 — The narrow-viewport projection is not client-side only

10 §D14 says the narrow policy switches "its *own* projection — the server,
other clients, and pane grids are untouched". Read against the tree, that is not
achievable as written:

- `Core` holds one viewport, `Option<(u16, u16)>` (`actor/core/mod.rs:140`), set
  from the declaring client's rows and cols (`core/view.rs:144-157`).
- `reconcile_pane_sizes` (`core/view.rs:184-221`) projects **the whole layout**
  into that viewport and resizes every pane's PTY to its slot's interior.
- `client::Viewport.panes` — "Panes visible in this client's projection … The
  server sends grid traffic for these panes and no others"
  (`amx-proto/src/control/client.rs:10-25`) — **has no server-side reader
  anywhere**. `handle_viewport` takes `rows` and `cols` and drops the rest.

So a 60-column client that shows one pane full-screen still declares a
60-column viewport against a two-pane layout, and the server sizes that pane to
about 28 columns. The client would letterbox a 28-column grid inside 60 columns
of terminal: correct, and useless — the phone case D14 exists for.

**Decision:** the narrow projection declares what it is actually showing. The
client sends `Viewport{rows, cols, panes: [the one pane]}`, and the server's
projection learns its first rule about that field: **a viewport declaring a
single pane sizes that pane to the whole content area**, rather than to its slot
in a layout the declaring client is not drawing. This is one amendment to 10
§D14's letter (R-M4-2) and it is also how `Viewport.panes` finally gets the
reader it was frozen with (R-M4-14).

What does *not* change: the pane grid still follows the most-recently-active
client (04 §3), so a phone attaching to a session a desktop is also watching
does resize its panes. That trade is 04's, already accepted, and is not made
worse here.

### D-M4-8 — Keybindings are not config data yet, so the phone profile is code before it is documentation

10 §D14 says of modifier-hostile keyboards: "No new mechanism: keybindings are
already config data and clients may bring local bindings at handshake. Ship a
documented `[keys]` phone profile example … Documentation work, not code."
Every clause of that is false in the tree:

- The prefix key is a constant: `pub const PREFIX: u8 = 0x01;` with the comment
  "configurable once config lands in M1" (`amx-client/src/input/mod.rs:57-58`).
  M1, M2 and M3 landed and it did not.
- The whole prefix table is a `match` on literal bytes
  (`input/mod.rs:306-337`), not data.
- There is no `[keys]` section. `config::SECTIONS`
  (`amx-core/src/config/mod.rs:44-49`) is `persist`, `terminal`, `update`,
  `work`, and the module's own rule is that "a section nobody reads is a promise
  nobody keeps" (`config/mod.rs:41-43`).
- `amx-client` reads no configuration at all — the crate contains no reference
  to `Config`.
- `client::Keybindings::{Server, Local}` (`amx-proto/src/control/client.rs:40-46`)
  has no reader anywhere in the tree.

**Decision:** X07 builds the minimum the profile needs and no more — a `[client]`
section (the narrow threshold) and a `[keys]` section carrying the prefix key and
the prefix table's bindings, read client-side through the `Ctx.config_path` the
CLI already builds (`amx-core/src/ctx.rs:199`, `crates/amx/src/cmd/mod.rs:46`),
plus `amx keys` to print the resolved table (04 §7 promises it). `amx-core`
already depends on `toml` (`crates/amx-core/Cargo.toml:19`) and `amx-client`
already depends on `amx-core`, so no dependency enters the tree. The docs
(X20) then describe a mechanism that exists. R-M4-3 records the amendment to 10.

### D-M4-9 — The client adopts `Effect` before the new surfaces land, not after

DR-10 is right that `amx-client` never consumes the structural dirtiness type:
it uses `dirty: bool` (`app/mod.rs:131`) and `layout_dirty: bool`
(`app/mod.rs:139`), the exact failure mode D2 exists to prevent, while the
server side consumes `amx_core::Effect` properly.

M4 adds four more client surfaces that each want to say "something changed" —
the status-line breakdown, the narrow projection, the agents view and the peek
region. Adopting the type *after* they land means four more call sites to
convert and four more chances for the silent-freeze bug the status-line cache
already documents in its own module doc ("a field added to the rendered text and
not to the equality guard renders once and then freezes, silently",
`app/status.rs:19-24`).

**Decision:** the adoption is wave 2, alone in `amx-client/src/app/`, and every
wave-3/4 client task is written against `Effect` from the start. This is R-M1-3's
"no split waits for the hard limit" applied to a type instead of a file. The two
name shadows DR-10 also names (`agent/fusion`'s `Effect` and
`amx-vt::callbacks`'s) are renamed by whichever task owns their file — X06 and
X09 respectively — because a rename is not worth a file-ownership exception.

### D-M4-10 — No field ships in M4 without its reader inside M4

R-M3-12 recorded the qualified version of this: "freezing a field ahead of its
reader costs nothing, and it does not make the reader's design right", and M3
found three fields whose readers were wrong or absent (`workspace.create`'s
`focus`, `Hello.resume`'s `generations`, and now `Viewport.panes` and
`Keybindings` in this plan's own research). M4 freezes six additive fields
across three surfaces.

**Decision:** the seam ledger this milestone opens counts **fields as well as
handlers**. A field added by X02 names the task that reads it, and the ledger
does not empty until every named reader has landed and the integration owner has
seen it work over a socket rather than in a type. Nothing is frozen "for later".

### D-M4-11 — `amx agents` is the renderer; `agent.list` is the method

The method table generates a CLI path from each row (`control/table.rs:92-99`,
`cli.rs:102`), so an `agent.list` row gives `amx agent list` for free and
`--json` for free through the generic call path. D15 additionally asks for
`amx agents` — a human table, `--watch`, `--workspace`.

**Decision:** both, and they are not the same thing. `agent.list`'s table row
carries CLI path `["agent", "list"]` and is the machine surface;
`amx agents` is a hand-written top-level subcommand (the shape `attach`,
`work` and `update` already use, `cli.rs:91-101`) that renders the same reply
for a person, with `--json` printing it verbatim so nobody has to know there are
two spellings. `--watch` is `amx events --json`'s loop packaged: subscribe,
redial on EOF, re-query on `gap` — the contract `crates/amx/src/cmd/events.rs`
already documents and implements (`events.rs:9-56`), reused rather than
reinvented.

### D-M4-12 — DR-6 is paid in wave 1, before anything else reads a short number

`ShortNumbers::assign` and `::resolve` are `todo!()`
(`amx-core/src/id.rs:230-233,247-250`), `Core` runs a monotonic stand-in whose
own comment says "swap it for `ShortNumbers` once that lands"
(`actor/core/mod.rs:95-109`), and the stand-in's output is **persisted**: restore
reads `saved.short` back off disk (`actor/core/restore.rs:201,360`) and the
import path does the same (`core/import.rs:201,218`). Every day the milestone
runs adds snapshots shaped by a mapping nobody implemented.

**Decision:** X05 lands in wave 1, before `agent.list` and the agents view give
short numbers a third and fourth consumer, and it owns the `Core` fields, the
restore/import reads and the `--pane` parse that routes around it today
(`crates/amx/src/cmd/attach.rs:36-40`).

---

## 2. The mouse-path spike (X01)

**What is known**, and it is all of D-M4-1: no mouse-tracking mode is ever
requested of the host terminal, the per-pane forwarding gate has no production
writer, the server-side accessor that would answer it has no caller, and no wire
field carries the answer. What is *not* known is everything that decides whether
D14's wheel exception can be built as designed.

**The questions, in order.**

1. **Does anything arrive?** Request SGR mouse reporting from a real host
   terminal and observe the bytes. `amx-client/src/bin/raw_mode_probe.rs` is the
   precedent for a probe binary that takes a real terminal and prints what it
   sees; this is its sibling.
2. **Which modes, and what do they cost?** `?1000` (button events) reports the
   wheel as button 64/65; `?1002` adds motion-while-pressed; `?1006` selects the
   SGR encoding `mouse::scan` already recognises
   (`amx-client/src/input/mouse.rs:24`). Requesting tracking takes the *host
   terminal's own* selection and copy away from the user unless they hold a
   modifier — which is a real cost for a keyboard-only tool and has to be
   measured on at least two terminal emulators rather than assumed, and priced
   against a config switch that leaves it off.
3. **How does a pane's own mouse mode reach the client?** The server can read it
   per pane (`Terminal::mode`, `amx-vt/src/terminal.rs:280`, over the same
   `Mode::dec(1000)`/`(1002)`/`(1003)` the handoff already reads,
   `handoff/manifest/modes.rs:49-57`) — the spike establishes *when* it can read
   it without a parser round trip on the query path, and therefore whether the
   fact belongs on `PaneState` as state, on the bus as an event, or both.
4. **Does the wheel decode without a coordinate?** `mouse::scan` deliberately
   "never extracts a button or a coordinate" (`input/mouse.rs:5-8`). The wheel
   exception needs exactly one number — the button — and the spike writes down
   the parse and the fence: the column and row are not parsed, at all, ever,
   because that is the boundary 03 §1 and D14 both draw.

**Protocol.** Observe before deciding: build the probe, run it under at least
two terminal emulators and one phone SSH client if one is reachable, record the
byte sequences verbatim in `docs/notes/m4-mouse-path.md`, and only then write
the recommendation. A hypothesis nobody watched arrive is labelled a hypothesis
in the note.

**Outcome tree.**

- **(a) Reports arrive and the cost is acceptable.** X13 builds the whole chain:
  request tracking on entry (behind a config switch defaulting to on), report the
  pane's mode to `Core`, carry it on `PaneState`, feed
  `Input::set_mouse_reporting` from it, forward to panes that asked and
  interpret wheel-up/down for panes that did not.
- **(b) Reports arrive but tracking costs the host terminal's selection in a way
  the note judges unacceptable by default.** Same chain, switch defaults off,
  and the phone profile (X20) is where it is turned on — the users who need
  touch-scroll are exactly the users who are not selecting text with a mouse.
  The wheel exception ships, reachable, and 10 §D14's "the concession every
  trial user reaches for in the first minute" becomes a documented opt-in;
  that is a change to the *default*, recorded here as a finding rather than
  taken silently.
- **(c) Reports do not arrive, or the mechanism is materially different from
  what D14 assumes.** X13 shrinks to the honest half — the per-pane mouse mode
  reaches the client and D9's forwarding works for the first time — and the
  wheel exception is deferred with the note as its written revisit condition.
  In that outcome D14 loses its mouse clause and keeps its narrow-viewport
  clause, and [§7](#7-the-m4-exit)'s exit drops the wheel row rather than
  claiming it.

In no outcome does X13 merge on the assumption that a byte arrives that nobody
has watched arrive.

---

## 3. Wire and event surface

Small, additive, and every field names its reader (D-M4-10). Nothing here is a
protocol version bump: additive optional fields ride v1 under the both-directions
unknown-field tolerance, the R-M1-8 precedent M2 and M3 both used.

**One new method row.**

| Row | Params | Reply | CLI |
|---|---|---|---|
| `agent.list` | optional `workspace` filter | `seq`, `now`, `attention` (queue order), `agents[]` per D15's shape | `amx agent list` |

Each `agents[]` entry carries `workspace{id,name}`, `pane`, `name`, `kind`,
`status`, `reason`, `since`, `last_line`. Read by X10 (server) and X14/X16
(the view and the CLI).

**Additive fields, no version bump.**

| Field | On | Reader |
|---|---|---|
| `reason` | `AgentSnapshot` (`amx-core/src/agent/mod.rs:206`) | X10, X11, X14, X16 |
| `since` | `AgentSnapshot` | X10, X11, X14, X16 |
| `now` | the `agent.list` reply | X14, X16 (D-M4-4) |
| `mouse` | `PaneState` (`control/session.rs:64`) | X13 |
| `workspace` | `agent::NextParams` (`control/agent.rs:443`, an empty struct today) | X17 |
| identity block | `Event::AttentionEnqueued` / `AttentionDequeued` (`amx-core/src/event/mod.rs:237,245`) | X16's `--watch`, `examples/notify.sh`, and any external consumer |

**One new error code.** `RpcError` already reserves amx's own range and has one
occupant — `WAIT_ABANDONED = -32000` in −32000..=−32099
(`amx-proto/src/rpc.rs:162-172`), added for exactly this reason: "a client that
recognises this code redials and asks again; one that does not sees an ordinary
error". DR-16's retriable code is its second occupant, and
`DriveError::NotAccepting` stops mapping to `INVALID_PARAMS`
(`dispatch/pane.rs:276`). Read by X09.

**One field that finally gets a reader.** `client::Viewport.panes`
(`control/client.rs:24`) — see D-M4-7. No wire change; the change is that the
server reads it.

**One wire surface decided.** `GridMessage::Scrolled`
(`amx-proto/src/stream/grid.rs:171`) is defined, encoded, golden-frozen
(`tests/protocol.rs:440-448`) and client-decoded
(`amx-client/src/stream.rs:201-203`), and no server code emits it. DR-7: emit it
or delete it. X08 decides and does one of them.

**No new event kinds.** The attention events grow fields; nothing new is
published. The bus keeps one publisher per event kind (D-M3-2): the hub owns
the agent and attention kinds, and the names it puts in them are folded off the
bus rather than asked for (D-M4-6).

**Goldens law** (the R-M2-7 pattern, restated so no wave task discovers it
mid-flight): one method golden and one skew arm for `agent.list`; regenerated
`session.state` and event-envelope goldens for the additive fields; a golden for
whichever way DR-7 is decided. All of it lands in X02.

**No new dependencies.** Client-side config needs `toml`, which `amx-core`
already carries (`crates/amx-core/Cargo.toml:19`).

---

## 4. The design-review register: what M4 takes, and what it declines

Every row of [notes/design-review.md](notes/design-review.md) still marked
`open` or `watch`, with where it goes. **The register is verified row by row
before it is scheduled** — one row was already stale when it was written (R-M4-6),
and a plan that scheduled it would have paid for a fix that exists.

| Row | Disposition |
|---|---|
| **DR-4** unowned integration seam | **Plan requirement**, not a task — [§6](#6-waves-file-ownership-and-the-standing-integration-owner) |
| **DR-5** over-planning at test-name granularity | **Plan requirement** — this document carries no acceptance-test names and no prompt drafts |
| **DR-6** ShortNumbers | **X05**, wave 1 (D-M4-12) |
| **DR-7** `GridMessage::Scrolled` is dead wire | **X08**, wave 2 |
| **DR-9** owed doc corrections | **X03**, wave 1 |
| **DR-10** three `Effect` shadows; client dirtiness | **X09** (client adoption + the vt rename) and **X06** (the fusion rename), wave 2 — D-M4-9 |
| **DR-11** the second shutdown wedge | **Watch, kept.** Not schedulable: the field mechanism has never been caught in the act, and W01's diagnosis fixed one path and narrowed a second to two `await`s in `conn/`. M4 keeps the drain census and adds one thing the register asks for — a milestone of field time. The live smoke ([§7](#7-the-m4-exit)) records the exporter's exit status and the drain census on every run, so "a milestone of field time passed clean" becomes a recorded fact rather than an impression. R-M4-10. |
| **DR-12** `frame on unbound channel` under flood | **X08**, wave 2 — the *decision* (refusal or silence, `amx-client/src/stream.rs:156` returns `Applied::Nothing` today) plus the named test the register says it lacks |
| **DR-13** remote latency honesty | **X03**, wave 1 — one sentence in 03 |
| **DR-14** fusion does not eliminate screen scraping | **No action**, as the register says. Restated here so it is not quietly dropped: both shipped agents measured `edges`, tier 2 owns every user-initiated exit, and D-M4-3 makes that visible rather than hiding it — `reason` naming `permission_dialog` says out loud which tier answered. |
| **DR-15** stale in-tree prose | **X03**, wave 1 — and it goes first inside that task, because in a repo whose first rule is "never guess", stale prose misleads every later task in this plan |
| **DR-16** reconnect coverage | **X09**, wave 2 — the retriable error code (wire-adjacent, decided before more callers bake in the ambiguity) and `attach --pane`'s missing reconnect (`crates/amx/src/cmd/viewport.rs`). The bridged-client redial is **declined**: it needs a respawned ssh child, which is `remote/`'s business and a different mechanism from the one this row is about; revisit when a remote attach is a daily path rather than a smoke step. |
| **DR-17** remote UX edges | Split. The `--help` clause is **already fixed** and was fixed before the register was written (R-M4-6): `cli.rs:78-90` declares `--remote` documentary, in commit `8e508c1`, an ancestor of the tree the register re-verified against. The `$MISE_INSTALLS_DIR` field and the newline-in-session-name encoding decision go to **X19**, wave 5. |
| **DR-18** no release channel exists | **Declined, with a revisit condition.** Release engineering is not multiplexer work, no task in this plan depends on a published binary, and the machinery is proven end to end against a `file://` channel (m3-live-smoke §2). `amx update check` already reports the 404 plainly and says there is no release pipeline (`crates/amx/src/update/manifest.rs:40`, W10's wave outcome). **Revisit when** the first tagged release is cut, or the first cross-platform seeding request arrives — whichever comes first; both make the stub a service that has to exist. |
| **DR-19** recorded flakes, unowned | **X04**, wave 1 — all four get an owner |
| **DR-20** SSH exit-clause residuals | **X19** for what code can do (the skew table's honest label), and the **live smoke** for the two things only a second machine proves: an independently versioned far side, and a handoff or `update apply` over the remote link. [§7](#7-the-m4-exit) names them as steps rather than letting "SSH works" round up. |
| **DR-21** resume optimization, recorded not built | **Declined, with a revisit condition and a measurement.** The route is sound and already half-present — `KeyframeReason::Resumed` exists (`amx-server/src/damage/keyframe.rs:65`) — but it is client-side sequencing work whose value is proportional to reconnect traffic. M4 *adds* per-client binds (peek binds a non-visible pane), so the condition becomes checkable: the live smoke records keyframe count and bytes on reconnect with 25 agents and a peek open. **Revisit when** that number is a cost somebody can name. |

Rows already resolved need no task, with one exception worth stating: **DR-1's
residual is real work and is scheduled.** The codec landed, but the client's
`Attrs` (`amx-client/src/model/grid.rs:38-51`) carries fg, bg, bold, italic, a
boolean underline and reverse, while the wire's `CellStyle`
(`amx-proto/src/stream/cell.rs:123-144`) carries ten attributes including faint,
blink, invisible, strikethrough, overline, the underline *style* and its colour
— and the frame writer emits four of them (`amx-client/src/render/mod.rs:89-98`).
Until that is closed, "the client renders the server's cells" is a claim about
six of ten attributes. It is **X18**, wave 5, and it is what makes D15's peek
region show a real screen rather than a reduced one.

---

## 5. Task DAG

Difficulty is `hard` when the task carries syscall, concurrency,
wire-compatibility, cross-crate-seam or restore-correctness risk; `normal`
otherwise. Every task lands with tests that fail without the change and finishes
with `cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt
--check` green. File scopes are exclusive within a wave; sequential fills of an
earlier task's file are declared here, never discovered.

Per DR-5, each entry states scope, dependencies, owned files and what must be
true for it to be accepted. It does not name the tests that prove it — that is
the implementer's call, and a name written here would be a name to satisfy.

---

### X00 — Integration owner (standing, waves 1–6)

- **Difficulty:** hard · **Wave:** 1 through 6 · **Depends on:** —
- **Goal:** DR-4, structurally: one owner holds every cross-crate seam for the
  whole milestone, and the live end-to-end smoke runs from wave 1 rather than at
  the exit.
- **Scope (owns exclusively):** `docs/notes/m4-live-smoke.md`,
  `docs/notes/m4-wave-outcomes.md`, `docs/notes/design-review.md` (the register's
  status column — every task reports its row and the owner strikes it, so the
  register never has five concurrent editors), `tests/integration.rs`,
  `tests/agents/attention.rs`, `tests/hygiene.rs` from wave 2 onward (X02 opens
  the seam ledger in wave 1; the owner closes it), and by exception any join no
  wave task could be granted.
- **The seams it owns**, named in advance rather than discovered:
  1. proto ↔ server ↔ client for every additive field in [§3](#3-wire-and-event-surface)
     — the D-M4-10 ledger;
  2. hub → `Core` mirror → `agent.list` → the two renderers (client view, CLI);
  3. client input ↔ server pane modes — the mouse chain X01 and X13 build;
  4. client viewport ↔ server pane sizing — the narrow projection's two halves
     (D-M4-7);
  5. `crates/amx` ↔ `amx-client` shared rendering: `amx agents --watch` and the
     agents view must not become two implementations of one table.
- **Acceptance:** the wave-1 smoke is recorded before wave 2 opens and a delta
  is recorded at every later wave boundary; the seam ledger empties with no stub
  answering retired code; the wave-outcomes note is written from what happened;
  and [§7](#7-the-m4-exit) is met.
- **The protocol it runs** is [§6](#6-waves-file-ownership-and-the-standing-integration-owner).

---

### X01 — The mouse-path spike

- **Difficulty:** hard · **Wave:** 0 · **Depends on:** —
- **Goal:** [§2](#2-the-mouse-path-spike-x01)'s four questions answered from
  observation, and one of its three outcomes chosen in writing.
- **Scope:** `docs/notes/m4-mouse-path.md`, `scripts/spike/**`,
  `crates/amx-client/src/bin/mouse_probe.rs` (new, beside
  `raw_mode_probe.rs`).
- **Acceptance:** the note records, verbatim, the bytes each terminal emitted
  under each requested mode; names the modes amx should request and the default
  it should request them at, with the selection cost measured rather than
  assumed; states where the per-pane mouse mode is read and how it reaches a
  client without a parser round trip on the query path; writes down the
  button-only parse and the fence that no column or row is ever parsed; and
  chooses outcome (a), (b) or (c) with the evidence that forced it. A conclusion
  nobody observed is labelled a hypothesis.
- **Gates:** X13.

---

### X02 — M4 contracts: rows, fields, config sections, splits, stubs

- **Difficulty:** hard · **Wave:** 1 · **Depends on:** —
- **Goal:** every shared surface later waves implement against, frozen; every
  budget-edge file split before the waves press it.
- **Scope:** `amx-proto/src/control/mod.rs` (the `agent.list` row),
  `amx-proto/src/control/agent.rs` **split into a directory** (463 lines today
  and the new payloads cross the budget), `amx-proto/src/control/session.rs`
  (`PaneState.mouse`), `amx-proto/src/rpc.rs` (the retriable code constant),
  `amx-proto/tests/**`, `tests/goldens/**`, `tests/protocol.rs`, `tests/skew.rs`
  (the `agent.list` arm); `amx-core/src/agent/mod.rs` (`reason`, `since` on
  `AgentSnapshot`), `amx-core/src/event/mod.rs` (the attention identity block),
  `amx-core/src/config/mod.rs` (`[client]` and `[keys]` sections and their
  `SECTIONS` rows), `amx-core/tests/**`; `amx-server/src/actor/calls.rs` (the
  `agent.list` call variant), `amx-server/src/actor/core/agents.rs` (new, empty),
  `amx-server/src/dispatch/agent.rs` seam stub; `crates/amx/src/cli.rs` +
  `crates/amx/src/cmd/mod.rs` (routing arms for `agents` and `keys`, planted here
  so no two wave tasks touch `cli.rs` — the U01/V02/W03 precedent);
  `tests/hygiene.rs` (opening M4's seam ledger); **budget splits landed before
  anything grows them:** `amx-proto/src/control/agent.rs` (463),
  `crates/amx/src/cli.rs` (471), `amx-server/src/agent/fusion/tracker.rs` (511,
  already over), `amx-server/src/actor/pane_host/parser.rs` (532, already over),
  `amx-server/src/actor/pane_host/mod.rs` (501, already over),
  `amx-server/src/dispatch/agent.rs` (450).
- **Declared hand-offs:** the one-line `pub mod agents;` in
  `amx-server/src/actor/core/mod.rs` and the empty module beside it ride X05's
  commit, because X05 owns that file whole this wave (the W02→W03 pattern). One
  arm in `amx-client/src/app/wired.rs`'s exhaustive `Method` match is the
  minimum the new row costs a file X09 owns in wave 2; the reader is X09's.
- **Acceptance:** the new row answers by name from the real binary and refuses
  with the seam's stub reply; every additive field parses in both directions
  with and without it present; every golden the [§3](#3-wire-and-event-surface)
  law demands is regenerated and readable in a diff; the seam ledger names each
  new field beside the task that will read it (D-M4-10); the module-size check
  is green with every named split landed, and no file this task touched is over
  the soft budget.

---

### X03 — Doc truth: DR-15, DR-9, DR-13, and the two D14 amendments

- **Difficulty:** normal · **Wave:** 1 · **Depends on:** —
- **Goal:** the tree stops asserting things that are false, and 04/10 stop
  promising things this plan's research contradicts.
- **Scope:** `crates/amx-client/src/model/grid.rs:33` (the "both bodies are
  still `todo!()`" comment — the codec exists),
  `crates/amx-client/tests/scrollback.rs:5` (the "no wire path delivers any of
  it yet" header — it does), `docs/09-m3-plan.md` §7 clause 3 and
  `docs/notes/m3-wave-outcomes.md`'s matching passage (both predate
  m3-live-smoke §5, which verified the SSH criterion on a second machine),
  `docs/04-architecture.md` §2 and §10 ("broadcast event bus" — the
  implementation is a cursor-over-replay-ring bus with typed gaps and resumable
  cursors, which is better than the promised primitive; the word is what is
  wrong) and the R3 correction from the M0 plan (herdr's bindings *are*
  bindgen-generated; its defect is the missing regeneration check),
  `docs/03-vision.md` (D14's wheel amendment to design principle 1, and DR-13's
  one sentence owning the remote-latency trade), `docs/10-attention-surfaces.md`
  (the two amendments D-M4-7 and D-M4-8 force).
- **Acceptance:** no comment or doc line in the named files asserts something
  the tree contradicts; each 04 change is the minimum wording that makes the
  sentence true, not a redesign; 10's two amendments name this plan's decision
  and its evidence; and every changed claim was checked against source rather
  than against this plan. The register rows go to X00, not to
  `design-review.md` directly.

---

### X04 — DR-19: the four recorded flakes get an owner

- **Difficulty:** normal · **Wave:** 1 · **Depends on:** —
- **Goal:** four failures with a written mechanism and no owner become four
  tests that either pass under load or say precisely why they cannot.
- **Scope:** `tests/adversarial.rs` (the `FLOOD_INGEST` 8 MiB threshold,
  `adversarial.rs:188`, and the `PATIENCE`-bounded observation loop around
  `adversarial.rs:245-300`: the register asks for rate-over-observed-time
  instead of a fixed quantity, and the loop's own comment already argues for
  the same thing — "a flood given a third of a core delivers a third of the
  bytes in the same three seconds"),
  `crates/amx-server/tests/flow_control.rs` + `flow_control/harness.rs`
  (`two_clients_at_different_speeds_each_stay_consistent`, one observed failure
  where 58 of 60 publications carried no damage at all — W08's wave outcome
  records the evidence), `crates/amx-server/tests/agent_verbs.rs` +
  `agent_verbs/harness.rs` (two failures in ~12 runs),
  `crates/amx/tests/hook.rs` (the `_hook` BrokenPipe self-race — the register's
  fix is to tolerate BrokenPipe on the payload write).
- **Acceptance:** each of the four is either fixed with the mechanism named, or
  documented in place with the reason it cannot be and what would change that;
  none is made to pass by widening a bound until it stops failing; and the
  suites run green under `nproc`-wide load for a repetition count the task
  states.

---

### X05 — DR-6: ShortNumbers, implemented and adopted

- **Difficulty:** hard · **Wave:** 1 · **Depends on:** —
- **Goal:** the lowest-free-number, reuse-after-release mapping 04 §6 specifies,
  implemented, adopted by `Core`, and reachable from the CLI.
- **Scope:** `amx-core/src/id.rs` (`assign`, `resolve`), `amx-core/tests/**`,
  `amx-server/src/actor/core/mod.rs` (the two stand-in counters and two maps at
  `core/mod.rs:95-109` become two `ShortNumbers`; the file also carries X02's
  declared one-line module declaration), `amx-server/src/actor/core/view.rs`
  (`short_of_workspace`/`short_of_pane`, `view.rs:125-137`),
  `amx-server/src/actor/core/restore.rs` (`restore.rs:201,220,360`),
  `amx-server/src/actor/core/import.rs` (`import.rs:201,218,346`),
  `amx-server/src/agent/address.rs` (short numbers join the UUID-then-label
  resolution order), `crates/amx/src/cmd/attach.rs` (the `--pane` parse that
  routes around `resolve` today, `attach.rs:36-40`), server tests.
- **Acceptance:** a released number is reused by the next assignment and not
  before; a number whose object is gone resolves to `None` and never to whatever
  took the slot next (the doc comment at `id.rs:241-246` is the specification);
  numbers survive a restart and a live handoff unchanged, including a session
  whose snapshot was written by the stand-in; the resolution order is UUID
  first, so a pane labelled with a number cannot shadow the number; and no
  `todo!()` remains in `crates/*/src`.

---

### X06 — The hub's new facts: `reason`, `since`, and identity-bearing attention events

- **Difficulty:** hard · **Wave:** 2 · **Depends on:** X02
- **Goal:** D-M4-3, D-M4-4 and D-M4-6 in the one actor that owns them.
- **Scope:** `amx-server/src/agent/fusion/**` (retain the name of whatever last
  moved the pane; and DR-10's rename of this module's `Effect` shadow),
  `amx-server/src/actor/agent_hub/{mod.rs,commit.rs,detect.rs,inherit.rs,run.rs}`
  (the names mirror, its seeding at `PaneStarted` and at `inherit`, the two new
  bus arms, the identity block on the two attention events, `since` stamped at
  the transition), `crates/amx-server/tests/{fusion.rs,agent_hub.rs,agent_hub/**}`.
- **Acceptance:** a pane's `reason` names the manifest rule that asserted its
  state (the shipped names, not a translation of them) or the hook event that
  did, and is absent for tier-3 `busy`/`quiet`; `since` moves only on a real
  transition and not on a re-evaluation that changed nothing; a subscriber to
  `attention_enqueued` can render `workspace/name blocked (reason)` with no
  follow-up call, including for a pane whose label was set before this hub
  started and for one inherited across a handoff; the hub still arms no timer on
  an idle session and still never sends to a sibling on the way down; and the
  write-then-publish ordering of `StatusView::commit` is untouched.

---

### X07 — Client configuration, a configurable prefix, and `amx keys`

- **Difficulty:** normal · **Wave:** 2 · **Depends on:** X02
- **Goal:** D-M4-8's minimum: the client reads configuration, the prefix table
  is data, and `amx keys` prints it.
- **Scope:** `amx-client/src/config.rs` (new — resolving `[client]` and
  `[keys]` into the bindings the input machine runs on),
  `amx-client/src/input/{mod.rs,mouse.rs}` (the byte-literal `match` at
  `input/mod.rs:306-337` becomes a lookup over a table; `PREFIX`
  (`input/mod.rs:57`) becomes a field), `crates/amx/src/cmd/keys.rs` (new),
  `crates/amx/src/cmd/attach.rs` (loading the config from the `Ctx` the CLI
  already builds and handing the bindings to the app through
  `App::input`, `app/mod.rs:329` — this task does not edit `app/`, which is
  X09's this wave), `crates/amx/tests/**`, `crates/amx-client/tests/input.rs`.
- **Acceptance:** an unset or malformed `[keys]` section leaves the shipped
  bindings exactly as they are, per the config module's lenient per-section rule
  (`amx-core/src/config/mod.rs:8-23`); a rebound prefix takes effect on the next
  attach and the old one goes to the pane like any other byte; the literal-prefix
  escape (pressing the prefix twice) still forwards it verbatim; `amx keys`
  prints the resolved table, including which bindings came from the file; and no
  dependency is added.

---

### X08 — DR-7 and DR-12: dead wire surface, and the unbound-channel decision

- **Difficulty:** normal · **Wave:** 2 · **Depends on:** X02 (goldens)
- **Goal:** the wire stops carrying a message nobody sends, and a client that
  receives a frame on a channel it does not know does something deliberate.
- **Scope:** `amx-proto/src/stream/grid.rs`, `amx-client/src/stream.rs`,
  `amx-server/src/damage/**`, `tests/protocol.rs` (a declared sequential fill of
  X02's goldens), the affected suites.
- **Acceptance:** `GridMessage::Scrolled` is either emitted by the server on the
  path 04 §3 describes — rows leaving the live grid, announced with id and hash
  — with a test that fails without the emission, or deleted from the enum, the
  codec, the goldens and the client, with the revisit condition written where it
  used to be; either way no golden protects a message nothing exercises. The
  unknown-channel behaviour (`amx-client/src/stream.rs:156`) is chosen with the
  reason written down — the old refusal was the check that caught a
  desynchronised peer (`docs/notes/frame-read-cancellation.md`) — and gets the
  named test the register says it lacks, including under the flood that produced
  the 3-in-30 failures.

---

### X09 — DR-10 and DR-16: one `Effect` client-side, and a retriable error

- **Difficulty:** hard · **Wave:** 2 · **Depends on:** X02
- **Goal:** the client stops tracking dirtiness with booleans before four new
  surfaces start setting them (D-M4-9), and a caller can tell "retry me" from
  "you asked wrong".
- **Scope:** `amx-client/src/app/**` (`dirty` at `app/mod.rs:131` and
  `layout_dirty` at `app/mod.rs:139` become `amx_core::Effect`, folded the way
  the server folds it; every call site that sets one today converts with it,
  including the arm X02 handed this file), `amx-vt/src/callbacks.rs` (DR-10's
  second name shadow) and its callers, `amx-proto/src/rpc.rs` reader side,
  `amx-server/src/dispatch/pane.rs:276` (the `NotAccepting` mapping),
  `crates/amx/src/cmd/viewport.rs` (`attach --pane`, which never reconnects —
  DR-16), client and CLI tests.
- **Acceptance:** no boolean dirtiness flag remains in `amx-client`, and a
  handler that changes what is on screen without reporting it does not compile;
  the repaint-allocation property the client already holds is unchanged; a
  mutating verb refused because the session is mid-handoff answers with the
  retriable code and every CLI path that can retry does; a single-pane attach
  survives a server swap the way a full attach already does, or the task states
  precisely which part of that it could not reach and why.

---

### X10 — `agent.list` on the server

- **Difficulty:** hard · **Wave:** 3 · **Depends on:** X02, X06
- **Goal:** D-M4-2's method, answered from `Core` in one round trip, at
  25 agents.
- **Scope:** `amx-server/src/actor/core/agents.rs` (the module X02 planted),
  `amx-server/src/dispatch/agent.rs` (the seam fill), server tests.
- **Acceptance:** the reply carries every field of [§3](#3-wire-and-event-surface)'s
  table with `attention` in the same queue order `session.state` reports, and
  the `seq` it was captured at; `last_line` is the literal last non-empty
  visible row, SGR-stripped and trimmed, never scrollback and never an
  interpretation, and is the empty string for a blank pane rather than absent;
  a `--workspace` filter narrows `agents` without changing the global queue
  order; the whole answer costs one mailbox round trip regardless of how many
  panes there are, and answering it does not block the actor loop measurably at
  25 panes; `now` and `since` agree with D-M4-4; and a pane with no tracked
  agent is absent rather than listed as unknown.

---

### X11 — The status line: per-workspace breakdown and the compact form

- **Difficulty:** normal · **Wave:** 3 · **Depends on:** X02, X07
- **Goal:** D15 surface 1 — `[api ⚑2] [web ⚑1] [infra] … ⚑5 api/backend 4m` —
  and D14's compact form beneath the narrow threshold.
- **Scope:** `amx-client/src/app/status.rs`, `amx-client/src/render/chrome.rs`,
  client tests.
- **Note on dependencies:** this needs no new wire. The client already mirrors
  **every** workspace and **every** pane — `fold_state` walks `state.workspaces`
  and `state.panes` whole (`amx-client/src/app/events.rs:137-186`) and holds
  labels, agent snapshots and the queue (`model/mod.rs:52-81`) — so the
  breakdown is a projection of state the client has had since M2. `since` comes
  from X02's field, folded through the existing `set_pane_agent` path.
- **Acceptance:** a workspace with no blocked agent shows its name without a
  count and one with blocked agents shows the count; the active workspace is
  distinguishable; the global count and the per-workspace counts are derived
  from one source and cannot disagree; the queue head's age advances between
  refreshes without a new call and is right across a remote link (D-M4-4);
  the line degrades to the compact form under the configured narrow threshold
  and back; and the module's own cache trap is respected — every new input is in
  the equality guard, and the test that catches a seventh input added without
  one still catches an eighth.

---

### X12 — D14: the narrow-viewport projection, and the reader `Viewport.panes` was frozen with

- **Difficulty:** hard · **Wave:** 3 · **Depends on:** X07
- **Goal:** below `client.narrow_cols` the client shows one pane full-screen and
  the server sizes that pane to the whole viewport (D-M4-7).
- **Scope:** `amx-client/src/app/narrow.rs` (new — the projection policy),
  `amx-client/src/app/{mod.rs,binds.rs}` (the repaint path and the viewport
  declaration at `binds.rs:132-148`), `amx-server/src/actor/core/view.rs` (the
  `Viewport.panes` reader and the single-pane sizing rule; X05 owned this file
  in wave 1, so this is a declared sequential edit), server and client tests.
- **Declared hand-off:** X13 owes this file the one line that fills
  `PaneState.mouse` in `session_state`; it rides X12's commit.
- **Acceptance:** a client narrower than the threshold renders exactly one pane
  and no borders it cannot afford, and the pane's grid is the size of the
  viewport rather than the size of its slot in a layout nobody is drawing;
  split-navigation keys change which pane is shown rather than splitting the
  screen; crossing the threshold in both directions is a projection change and
  never a layout mutation — no `pane.close`, no `pane.split`, and the server's
  layout tree is byte-identical either side; the picker and the agents view
  render full-screen under the policy; and a wide client attached to the same
  session keeps drawing its own projection, with the pane-size churn bounded by
  the debounce 04 §3 already requires.

---

### X13 — D14: the mouse path end to end, and wheel → copy mode

- **Difficulty:** hard · **Wave:** 3 · **Depends on:** X01 (gate), X02
- **Goal:** whichever of [§2](#2-the-mouse-path-spike-x01)'s outcomes X01
  reached, built: at minimum D9's forwarding works for the first time; at best
  D14's wheel exception with it.
- **Scope:** `amx-client/src/input/**`, `amx-client/src/term.rs` (requesting and
  releasing mouse tracking on the host terminal, restored on every path the
  guard already restores — normal exit, panic, `SIGTERM`,
  `term.rs:139-155`), `amx-client/src/copy.rs` (the wheel's entry into and exit
  from copy mode over the cached scrollback),
  `amx-server/src/actor/pane_host/**` (observing the pane's mouse-tracking mode
  on the parser thread, over `Terminal::mode`, `amx-vt/src/terminal.rs:280`),
  `amx-server/src/actor/panes.rs` and `amx-server/src/actor/core/report.rs` (the
  report and its fold), client and server tests.
- **Declared hand-off:** the one-line `mouse:` fill in `core/view.rs`'s
  `session_state` rides X12's commit.
- **Acceptance:** a pane that enables mouse reporting receives SGR reports
  byte-identical, and a pane that does not receives none — proven against a real
  program on a real pty, not against a hand-set flag; the pane's mode reaches an
  attached client without a parser round trip on any query path; wheel-up in a
  pane without reporting enters copy mode and scrolls the local cache, and
  wheel-down at the live edge leaves it; no column or row is parsed anywhere in
  the client, and a test asserts the chrome path sees no positional value at
  all; leaving amx leaves the host terminal's mouse state exactly as it was
  found; and the config switch X01 recommends defaults the way X01's note says,
  with the reason quoted where the default is set.

---

### X14 — The agents view

- **Difficulty:** normal · **Wave:** 4 · **Depends on:** X10, X09
- **Goal:** D15 surface 2 — the picker with one extension (a live detail line),
  grouping, idle collapse, and `ctrl+b/p/x/r`.
- **Scope:** `amx-client/src/app/agents.rs` (new — the view's state, its
  ordering and its keymap), `amx-client/src/app/overlay.rs` (the view joins the
  picker and copy mode as a drawn surface),
  `amx-client/src/picker.rs` (the detail-line extension — entries may carry a
  second column; that sentence is the whole licence),
  `amx-client/src/input/mod.rs` (the view's prefix key and the scoped-attention
  key X17 owes a binding), client tests.
- **Acceptance:** ordering within a group is blocked-oldest-first, then working,
  then idle, so the top row is always who needs the user most; more than three
  idle agents in a group collapse to one row that expands; `ctrl+s` toggles
  grouping between workspace and state; `ctrl+b` filters to blocked and is the
  same surface the attention picker would have been; `ctrl+p` prompts without
  attaching; `ctrl+x` kills only on a second press and no confirmation dialog is
  introduced; `ctrl+r` renames; filter, grouping and selection persist per
  client across closing and reopening and are never sent to the server; the
  detail lines refresh at up to 4 Hz from one `agent.list` per window and never
  one per pane (R-M4-7); and no filter syntax and no launcher exist.

---

### X15 — The peek region

- **Difficulty:** hard · **Wave:** 4 · **Depends on:** X14, X09
- **Goal:** `Space` on a selected agent shows that pane live, read-only, in a
  reserved region.
- **Scope:** `amx-client/src/app/peek.rs` (new — binding, lifetime and
  teardown), `amx-client/src/app/binds.rs` (binding a grid stream for a pane
  outside the focused workspace; `bind_visible` binds only the focused
  workspace's panes today, `binds.rs:66-96`),
  `amx-client/src/render/grid.rs`, client tests.
- **Note on dependencies:** no server change is needed. `stream.bind` resolves a
  pane through `StreamCall::Wiring` with no visibility check, and
  `Viewport.panes` has no server reader (D-M4-7), so a bind for a non-focused
  pane is served today. After X12 that field *does* have a reader, which is
  exactly why this task and X12 must agree: a peek bind is legitimate and the
  narrow projection's declaration must not make it a lie. That agreement is
  X00's seam 4.
- **Acceptance:** peek renders the selected pane's real cells with its real
  attributes, updating as the pane paints; no keystroke reaches the peeked pane
  — `ctrl+p` is the only reply path; closing the peek releases the stream and
  the pane stops costing this client traffic; switching selection moves the
  stream rather than accumulating them; a peeked pane that dies leaves the view
  usable and says so; and under D14's narrow policy the peek replaces the list
  rather than sharing the width.

---

### X16 — `amx agents`: one-shot, `--watch`, `--json`, `--workspace`

- **Difficulty:** normal · **Wave:** 4 · **Depends on:** X10
- **Goal:** D15 surface 3, sharing its rendering with nothing and duplicating
  nothing.
- **Scope:** `crates/amx/src/cmd/agents.rs`, `crates/amx/src/agents/**` (the
  table renderer both forms use), `crates/amx/tests/**`.
- **Acceptance:** the one-shot form prints workspace, name, status, reason, age
  and last line and exits; `--json` prints the `agent.list` reply verbatim, so a
  consumer never has to know a human form exists; `--watch` is a live full-screen
  view that `q` quits, built on the subscribe-redial-requery contract
  `amx events --json` already implements and documents
  (`crates/amx/src/cmd/events.rs:9-56`) — a `gap` re-queries and is never
  swallowed, and a server swap is invisible rather than an end of stream;
  `--workspace` scopes either form; and the whole command works with no client
  attached, from a plain SSH window, at 45 columns.

---

### X17 — Workspace-scoped `next-attention`

- **Difficulty:** normal · **Wave:** 3 · **Depends on:** X02
- **Goal:** D15's scoped cycling: clearing one project's queue never yanks focus
  across projects.
- **Scope:** `amx-server/src/actor/agent_hub/verbs.rs` (`next_attention`,
  `verbs.rs:29-46`), `crates/amx/src/cmd/**` for the CLI flag, server tests.
- **Declared hand-off:** the client keybinding is X14's, in wave 4, on a key
  neighbouring the existing `prefix+a` (`amx-client/src/input/mod.rs:333`).
- **Acceptance:** the scoped call focuses the oldest blocked agent in the named
  workspace and reports how many remain *in that workspace*, while the queue
  itself stays global and block-time ordered — a scoped call never reorders it;
  an empty scope is an honest empty and not an error, the way the unscoped call
  already is; and a request with no workspace behaves exactly as it does today,
  goldens unchanged.

---

### X18 — DR-1's residual: the client renders the cells the server sends

- **Difficulty:** normal · **Wave:** 5 · **Depends on:** X15
- **Goal:** close the gap between the ten attributes the wire carries and the
  six the client models.
- **Scope:** `amx-client/src/model/grid.rs` (`Attrs`, `grid.rs:38-51`),
  `amx-client/src/render/{mod.rs,grid.rs}` (the SGR differ, which emits four of
  them, `render/mod.rs:89-98`), `amx-client/src/stream.rs` (`cell_of`,
  `stream.rs:237-253`), client tests.
- **Acceptance:** every attribute `CellStyle` carries
  (`amx-proto/src/stream/cell.rs:123-144`) either reaches the terminal or is
  named in the module doc as deliberately dropped with the reason; the underline
  *style* and its colour are distinguished from a boolean underline; the
  differ's no-allocation-after-the-first-frame property survives; and a peeked
  pane and a focused pane render the same cells identically.

---

### X19 — DR-17 and DR-20 residuals

- **Difficulty:** normal · **Wave:** 5 · **Depends on:** —
- **Goal:** three named residuals closed or decided, and the SSH evidence
  labelled honestly.
- **Scope:** `crates/amx/src/update/pm.rs` (`$MISE_INSTALLS_DIR`, whose absence
  is documented at `pm.rs:24-29` and whose fix W10 wrote down as "one `Env`
  field plus one parameter on `pm::classify`"), `amx-core/src/ctx.rs` (that
  field), `crates/amx/src/remote/ssh.rs` (the newline-in-session-name case, which
  `ssh.rs:318-322` records as the one character with no answer against csh —
  decide the encoding or refuse the name at the boundary, and say which),
  `tests/skew.rs`, `tests/remote_ssh.rs`, `crates/amx/tests/update.rs`.
- **Acceptance:** a relocated mise installs root classifies as mise and is
  redirected rather than written into; a session name that cannot cross to a
  csh login shell is handled by a decision written in the module rather than by
  drift; and the skew suite's label says what it proves — current-vs-current
  until a second protocol version exists, and same-tree-cross-built until a
  second machine builds its own binary (DR-20's first residual, which no code
  change can close and which [§7](#7-the-m4-exit) names as a smoke step).

---

### X20 — The configuration reference and the phone profile

- **Difficulty:** normal · **Wave:** 5 · **Depends on:** X07, X12, X13
- **Goal:** D14's last deliverable: a documented `[keys]` phone profile, resting
  on a mechanism that exists (D-M4-8).
- **Scope:** `docs/12-config.md` (new — the configuration reference amx does not
  have; every section, every key, the per-section lenient reload rule, and the
  narrow threshold), `examples/keys-phone.toml`, `docs/README.md`'s table row.
- **Acceptance:** every section and key documented exists in
  `amx-core::config::SECTIONS` and is read by something named in the doc; the
  phone profile is a file a user can copy that binds a prefix a phone keyboard
  can actually emit, and it was tried on a real phone SSH client or the doc says
  it was not; and the wheel-scroll default matches whatever X13 shipped, quoted
  from X01's note rather than restated.

---

## 6. Waves, file ownership, and the standing integration owner

Merge in wave order; within a wave, any order — no two tasks in a wave touch the
same file. X00 runs alongside every wave and owns only its own files.

| Wave | Tasks | Concurrency | Unblocks |
|---|---|---|---|
| 0 | **X01** mouse-path spike | 1 | X13's gate |
| 1 | X02 contracts · X03 doc truth · X04 flakes · X05 ShortNumbers · *(X00 opens)* | 4 + owner | wave 2 |
| 2 | X06 hub facts · X07 client config + keys · X08 dead wire · X09 client `Effect` + retriable error | 4 + owner | wave 3 |
| 3 | X10 `agent.list` · X11 status line · X12 narrow projection · X13 mouse path · X17 scoped attention | 5 + owner | wave 4 |
| 4 | X14 agents view · X15 peek · X16 `amx agents` | 3 + owner | wave 5 |
| 5 | X18 styling · X19 remote residuals · X20 config docs | 3 + owner | X00's exit |
| 6 | **X00** exit | 1 | M4 exit |

**File-ownership check for concurrent waves** (no overlaps):

- **Wave 1** — X02: `amx-proto/**`, `amx-core/src/{agent/mod.rs,event/mod.rs,config/mod.rs}`,
  `amx-server/src/actor/calls.rs`, `actor/core/agents.rs` (new),
  `dispatch/agent.rs`, `crates/amx/src/{cli.rs,cmd/mod.rs}`, `tests/goldens/**`,
  `tests/protocol.rs`, `tests/skew.rs`, `tests/hygiene.rs`, and the six budget
  splits. X03: docs plus two comment lines in `amx-client`. X04:
  `tests/adversarial.rs` and four test files nobody else lists. X05:
  `amx-core/src/id.rs`, `amx-server/src/actor/core/{mod,view,restore,import}.rs`,
  `agent/address.rs`, `crates/amx/src/cmd/attach.rs`. X02 and X05 both need
  `actor/core/mod.rs`: **X05 owns the file whole and carries X02's one-line
  module declaration** as a declared hand-off, the W02→W03 shape. Disjoint after
  that.
- **Wave 2** — X06: `amx-server/src/agent/fusion/**` and
  `actor/agent_hub/{mod,commit,detect,inherit,run}.rs` plus its suites. X07:
  `amx-client/src/{config.rs,input/**}`, `crates/amx/src/cmd/{keys.rs,attach.rs}`.
  X08: `amx-proto/src/stream/grid.rs`, `amx-client/src/stream.rs`,
  `amx-server/src/damage/**`, `tests/protocol.rs` (sequential fill of X02's).
  X09: `amx-client/src/app/**`, `amx-vt/src/callbacks.rs`,
  `amx-server/src/dispatch/pane.rs`, `crates/amx/src/cmd/viewport.rs`. X07 and
  X09 are both in `amx-client` and disjoint by directory — X07 stays out of
  `app/` by handing bindings through the public `App::input`
  (`app/mod.rs:329`), which is a declared constraint on X07 and not an accident.
  DR-10's two renames are split by file ownership: X06 renames the fusion
  shadow, X09 the vt one.
- **Wave 3** — X10: `actor/core/agents.rs`, `dispatch/agent.rs`. X11:
  `amx-client/src/app/status.rs`, `render/chrome.rs`. X12:
  `amx-client/src/app/{narrow.rs,mod.rs,binds.rs}`, `actor/core/view.rs`. X13:
  `amx-client/src/{input/**,term.rs,copy.rs}`, `actor/pane_host/**`,
  `actor/panes.rs`, `actor/core/report.rs`. X17:
  `actor/agent_hub/verbs.rs`, `crates/amx/src/cmd/**`. X13 owes `core/view.rs`
  one line and hands it to X12; X17 owes the client a key and hands it to X14.
  Disjoint.
- **Wave 4** — X14: `amx-client/src/app/agents.rs`, `app/overlay.rs`,
  `picker.rs`, `input/mod.rs`. X15: `amx-client/src/app/peek.rs`,
  `app/binds.rs`, `render/grid.rs`. X16: `crates/amx/src/cmd/agents.rs`,
  `crates/amx/src/agents/**`. X14 and X15 share a surface and not a file: X14
  reserves the region, X15 fills it. That seam is X00's, named in advance
  (seam 5).
- **Wave 5** — X18: `amx-client/src/{model/grid.rs,render/**,stream.rs}`. X19:
  `crates/amx/src/{update/pm.rs,remote/ssh.rs}`, `amx-core/src/ctx.rs`,
  `tests/{skew,remote_ssh}.rs`. X20: docs and `examples/`. Disjoint.

**Cross-wave sequential edits, declared:** X10 fills X02's `dispatch/agent.rs`
stub and `core/agents.rs` module; X08 fills goldens X02 regenerated; X12 and X13
edit files X05 owned in wave 1; X14 edits `input/mod.rs` after X07 and X13;
X15 edits `app/binds.rs` after X12; X18 edits `stream.rs` after X08 and
`render/grid.rs` after X15; X03's two comment fixes precede X18's edits to the
same files. All sequential, never concurrent.

### The standing integration owner (DR-4)

M2's V17, M3's W14 and the two before them were four retrofits of the same hole:
the wave scheme that makes parallel execution safe leaves cross-crate joins owned
by nobody. M4 does not repeat it.

**One agent holds X00 for the whole milestone.** It is not a wave-6 task with a
wave-6 start; it opens with wave 1 and closes the milestone. It owns the five
seams named in X00's entry, it owns the seam ledger from wave 2, and it owns the
register's status column so twenty tasks do not edit one file.

**The wave-1 smoke.** Before wave 2 opens, the owner runs the real binary and
records `docs/notes/m4-live-smoke.md` §1 — the baseline every later run is a
delta against. It exercises what wave 1 could not have broken *and what waves
2–5 will*, so a regression has something to be a regression from:

1. Twenty-five agents across five workspaces, using the spike-anchored fake
   agents (`tests/agents/fixtures.rs` — real agent binaries do not exist on
   runners, R-M2-8's standing constraint), several of them blocked.
2. A real `amx attach` on a real pty at 200×50, and a second at 45×20 — the
   narrow case, before any narrow policy exists, so the "correct and useless"
   letterbox D-M4-7 predicts is *observed and recorded* rather than argued.
3. `amx session state` and `amx agent next` against that session, with wall-clock
   timings, so wave 3's `agent.list` has a cost to be compared against.
4. `amx events --json` running throughout, its stream checked gapless-or-gap-marked.
5. Short numbers before and after a restart, so X05's claim about snapshots
   written by the stand-in is measured on a real one.
6. The old server's exit status and drain census on a `session stop` (DR-11's
   watch — [§4](#4-the-design-review-register-what-m4-takes-and-what-it-declines)).

**Every wave boundary after that** appends a dated section: what the wave added,
run against the real binary from outside the process, plus the previous
baseline's checks re-run. A wave is not closed until the owner has smoked it. The
record lives in one file for the milestone, which is what makes "when did this
stop working" answerable.

**Where wave outcomes go.** `docs/notes/m4-wave-outcomes.md`, written by each
task as it lands — divergences and hand-offs only, the M3 format. A task that
landed exactly as its §5 entry describes writes nothing.

---

## 7. The M4 exit

Green tests **plus** a recorded live smoke of the real binary. Four milestones
have now needed the second half — M2's smoke found two non-working features, M3's
found a client that came back from a swap wrong forever — and this milestone's
own research found a third class before a line was written: a mouse path that is
green in a unit test and dead in a running amx (D-M4-1).

**In CI, over the real binary:**

1. Twenty-five agents across five workspaces. `agent.list` answers with every
   field, `attention` in queue order, and `last_line` matching what
   `pane.read` says the bottom of each pane is.
2. The status line shows a per-workspace breakdown whose counts sum to the
   global count, and the queue head with an age that advances.
3. The agents view opens, filters, groups, collapses idle rows, jumps, prompts,
   renames, and kills only on the second press; peek shows a live pane from
   another workspace with its real attributes and forwards no keystroke.
4. `amx agents`, `--json` and `--watch` answer with no client attached, and
   `--watch` survives a server swap without ending its stream.
5. `next-attention --workspace` clears one project's queue without leaving it.
6. A 45-column client renders one pane at 45 columns — not a 28-column grid
   letterboxed inside 45 — and crossing the threshold mutates no layout.
7. Whichever mouse row X01's outcome licenses: a pane that asked for reports
   gets them byte-identical and one that did not gets none; and, under outcomes
   (a)/(b), wheel-up scrolls copy mode and wheel-down at the live edge leaves it,
   with no positional value parsed anywhere.
8. The register's own greens: no `todo!()` in `crates/*/src`; short numbers
   stable across restart and handoff; the four DR-19 flakes green under
   `nproc`-wide load for the repetition count X04 states; no golden protecting
   an unexercised message; the client carrying no boolean dirtiness flag; and
   the seam ledger empty with every additive field of [§3](#3-wire-and-event-surface)
   read by the task that promised to read it.

**By hand, before the milestone closes** — recorded in
`docs/notes/m4-live-smoke.md` with date, versions and outcomes, the format M2 and
M3 used:

1. Five real Claude Code sessions across at least two workspaces, several
   blocked on real permission dialogs, with the agents view open beside them:
   every `reason` names the rule or hook that actually fired, and every
   `last_line` is the line on the screen.
2. An attach from a **phone SSH client** — the workflow D14 exists for — using
   the X20 phone profile: reach a pane, scroll its scrollback by touch (or record
   that outcome (c) means you cannot), open `amx agents --watch`, and answer a
   blocked agent with `ctrl+p`.
3. DR-20's two unproven clauses, which only a second machine can close: a far
   side running a binary it built itself rather than one cross-built here, and a
   handoff or `amx update apply` exercised over the remote link. Whatever is not
   reached is named as not reached.
4. DR-11's watch: the exporter's exit status and drain census across the
   milestone's runs, so "a milestone of field time passed clean" is a record.
5. DR-21's measurement: keyframe count and bytes on a reconnect with 25 agents
   and a peek open, so the deferred optimization has a number attached to its
   revisit condition.

Green CI plus this checklist is the exit. Green CI alone is not — four times
proven, and once more by this plan's own research.

---

## 8. Risks & findings

Flagged for the orchestrator, not silently resolved.

**R-M4-1 — The mouse spike may cost the host terminal's selection, or may find
nothing arrives at all.** [§2](#2-the-mouse-path-spike-x01)'s outcome tree is
the contract: X13 does not merge on an assumption about a byte nobody watched
arrive. Under (b) the wheel exception ships opt-in, which changes a *default* 10
§D14 states; under (c) D14 keeps its narrow-viewport clause and loses its mouse
clause, and [§7](#7-the-m4-exit) drops the row rather than claiming it. The
milestone's other tracks are independent of the outcome and do not wait.

**R-M4-2 — Proposed amendment to 10 §D14: the narrow projection is not
client-side only.** D-M4-7 has the evidence: `Core` holds one viewport as
`(rows, cols)` (`actor/core/mod.rs:140`), projects the whole layout into it
(`core/view.rs:184-221`), and never reads `client::Viewport.panes`
(`control/client.rs:24`). 10 §D14's "the server, other clients, and pane grids
are untouched" cannot hold for a client that shows one pane full-screen. The
amendment is one server-side rule — a viewport declaring a single pane sizes
that pane to the whole content area — and X03 writes it into 10. Nothing in 04
is contradicted: 04 §3 already gives the server the client's projection as the
definition of visibility.

**R-M4-3 — Proposed amendment to 10 §D14: the phone profile is code before it
is documentation.** D-M4-8 has the evidence: a `const PREFIX`
(`amx-client/src/input/mod.rs:57`), a `match` on byte literals
(`input/mod.rs:306-337`), four config sections and no `[keys]`
(`amx-core/src/config/mod.rs:44`), a client that reads no config, and a
`Keybindings` enum with no reader (`control/client.rs:40`). 10's "No new
mechanism … Documentation work, not code" is wrong; X07 builds the minimum and
X03 amends the sentence. 04 §7 is *not* contradicted — it promised exactly this
mechanism and it was never built.

**R-M4-4 — `since` must survive a handoff and a cold restore, or every age lies
after an upgrade.** The handoff half is free: the manifest carries
`AgentSnapshot`s (W07's outcome), so an additive field rides. The cold-restore
half is not: a restored pane's agent status is re-derived, and a `since` that
restarted at the restore would tell a user an agent has been blocked for four
seconds when it has been blocked all night. X06 states which it does and the
honest fallback is "since this server started tracking it", said out loud rather
than implied. The same seeding question applies to D-M4-6's names mirror: a
label that only arrives by rename event is absent for every restored and every
inherited pane, so the seed at `PaneStarted`/`inherit` is load-bearing, not a
convenience.

**R-M4-5 — Module budgets bind before M4 writes a line.** Four `src/` files are
over the soft budget today and every one of them is on M4's path:
`actor/pane_host/parser.rs` 532 (X13 grows it), `agent/fusion/tracker.rs` 511
(X06 grows it), `crates/amx/src/remote/ssh.rs` 516 (X19 touches it),
`actor/pane_host/mod.rs` 501 (X13). Warning-adjacent and certain to grow:
`crates/amx/src/cli.rs` 471 (two new verbs), `amx-proto/src/control/agent.rs`
463 (the `agent.list` payloads), `amx-server/src/dispatch/agent.rs` 450 (the
handler), `amx-core/src/state/session.rs` 499 and
`amx-server/src/actor/core/restore.rs` 499 (both at the line, and X05 edits the
second). X02 front-loads six splits; the R-M1-3 rule stands — no split waits for
the hard limit, and W05's discovery stands with it: a file becoming a directory
needs no parent edit, so "I cannot split this, it is not my file" costs nothing.

**R-M4-6 — The register is partly stale, and every row is verified before it is
scheduled.** DR-17's first clause says "`amx --help` never mentions `--remote`".
It does: `cli.rs:78-90` declares the flag documentary with the reason, landed in
commit `8e508c1`, which is an ancestor of `b727786` — the tree the register says
it re-verified against. One row in twenty-one was already paid for. The cost of
not checking is a task that produces a diff of zero and a milestone that thinks
it fixed something.

**R-M4-7 — `agent.list` is cheap per call and quadratic per pane.** One call
walks every tracked pane, which is what makes D-M4-2 a single round trip. A
surface that refreshed *per pane* at 4 Hz with 25 agents would issue 100 calls a
second each walking 25 panes. X14 and X16 refresh once per debounce window for
all rows; the constraint is in both their acceptances, and X00's seam 2 is where
a violation would be caught.

**R-M4-8 — DR-18 is declined and the decline is conditional.** No task in this
plan needs a published binary, the machinery is proven against a `file://`
channel, and `update check` reports the missing manifest plainly. It stops being
declinable at the first tagged release or the first cross-platform seeding
request. Treating this plan as having shipped a hosted channel would be reading
a stub as a service — R-M3-4's sentence, still true.

**R-M4-9 — DR-21 is declined with a measurement rather than an opinion.** The
route is sound and `KeyframeReason::Resumed` already exists
(`damage/keyframe.rs:65`); the value is proportional to reconnect traffic, and
M4 raises that traffic slightly by adding peek binds. [§7](#7-the-m4-exit)
records the number so the revisit condition is checkable instead of atmospheric.

**R-M4-10 — DR-11 stays a watch, and M4 is the milestone of field time it asked
for.** The census stays; the live smoke records the exporter's exit and the
drain census on every run. If a census fires, it becomes a finding in
`m4-wave-outcomes.md` with the backtrace, not a retry.

**R-M4-11 — This plan is half of the roadmap's M4.** `amx api schema`, extension
examples, the registry long tail, theme-as-six-colours, the performance pass and
the kitty-graphics decision are all in 05's M4 section and none is in this task
DAG. They are not cancelled and they are not covered; a green M4 by this plan's
exit criteria does not meet 05's M4 exit sentence ("a stranger can read the docs,
extend amx with a shell script, and add their agent with one registry stanza"),
and 05's M4 section should say which half this plan is.

**R-M4-12 — Adopting `Effect` client-side touches every file in
`amx-client/src/app/`.** That is why it is wave 2 and alone there (D-M4-9). If
X09 slips past its wave, waves 3 and 4 inherit a conflict with four tasks at
once — X11, X12, X14 and X15 all live in `app/`. The mitigation is ordering, and
the fallback is that X09 lands the fold and the type and leaves the four new
surfaces to adopt it as they are written, which is worse but not a merge
disaster.

**R-M4-13 — No new dependency is expected.** Client-side config uses `toml`,
already in `amx-core` (`crates/amx-core/Cargo.toml:19`). If any task finds it
needs one, HACKING.md's rule applies: a one-line justification in the commit
body, and a note here.

**R-M4-14 — Two more frozen fields with no readers, found while planning.**
`client::Viewport.panes` (`control/client.rs:24`) and
`client::Keybindings` (`control/client.rs:40`) join `workspace.create`'s `focus`
and `Hello.resume`'s `generations` as fields frozen ahead of a reader that never
came. M4 gives the first one a reader (D-M4-7) and the second one either a
reader or an exemption written into 04 — X07 decides which, since local
keybindings resolved entirely client-side may make the handshake declaration
redundant. D-M4-10 is the rule that stops the list growing.
