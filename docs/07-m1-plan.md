# M1 execution plan

The build plan for **M1 — Durability** ([05-roadmap.md](05-roadmap.md)). Binding
design is [04-architecture.md](04-architecture.md) §6 (persistence) and §2 (the
Persist actor); this document does not change it. Where research contradicted or
complicated a decision in 04/05, it is recorded in [§6 Risks](#6-risks--findings)
rather than silently redesigned. The T19 lesson from the M0 plan is applied from
day one: every cross-crate seam this milestone creates has a named owner (U10).

Everything below that states a fact about the amx tree, herdr, a man page, or a
crate was read from source during planning. The load-bearing code facts:
the server today runs three actors (`Core`, `PaneHost`, `Gateway` —
`amx-server/src/lib.rs` says so explicitly; `Persist` does not exist), the
method table holds sixteen rows ending at `client.viewport`
(`amx-proto/src/control/mod.rs:191-351`), `Event` holds sixteen variants and is
`#[non_exhaustive]` (`amx-core/src/event/mod.rs:41-159`), `Ctx.state_dir`
exists but nothing ever writes it (`amx-core/src/ctx.rs:183`; the only non-test
reference is `session/registry.rs:237`), and the state model a snapshot must
capture is `Pane { id, label, cwd }` — no argv — plus
`Workspace { id, label, layout, focus, area }` with a `Layout` that already
derives serde because `session.state` ships the tree verbatim
(`amx-core/src/layout/tree.rs:43-50`).

---

## 1. Decisions taken during research

### D-M1-1 — The durable write sequence: tmp fsync → rename → directory fsync

**Decision:** one helper, `persist::io::write_atomic`, used for every state
file, doing exactly:

1. create `<name>.<pid>.<counter>.tmp` in the destination directory;
2. write the full payload;
3. `File::sync_all` on the tmp file;
4. `rename(tmp, dest)`;
5. open the destination directory and `File::sync_all` on it.

**Evidence, step by step.** fsync(2): "Calling fsync() does not necessarily
ensure that the entry in the directory containing the file has also reached
disk. For that an explicit fsync() on a file descriptor for the directory is
also needed" — that is step 5, and skipping it means the rename itself can
vanish at power loss. Step 3 must precede step 4 because rename orders nothing:
fsync(2) is the only call that "transfers all modified in-core data … to the
disk device", so a rename that becomes durable before the tmp file's blocks do
leaves the committed name pointing at incomplete data (ext4's `auto_da_alloc`
heuristic papers over the rename-over-existing case, but it is a mount option,
not a contract). The tmp name carries pid + a process counter because herdr's
fixed `session.json.tmp` (`herdr/src/persist/io.rs:44-76`) lets two writers to
the same directory clobber each other's staging file; herdr itself uses the
collision-proof form in the one place it hardened
(`.{name}.{pid}.{nanos}.tmp`, `herdr/src/detect/manifest_update.rs:406-441`) —
that file is also the only place in herdr that calls fsync at all, which is W8
in one sentence.

**Darwin.** On macOS a plain `fsync` does not flush the drive cache; full
durability needs `fcntl(F_FULLFSYNC)`. Rust's standard library already does
this: `File::sync_all` and `sync_data` compile to `fcntl(fd, F_FULLFSYNC)` on
`target_vendor = "apple"` (`library/std/src/sys/fs/unix.rs:1391-1412` in the
pinned 1.97.1 sources). So the helper uses `std::fs::File` sync calls, not
`rustix::fs::fsync` — rustix's `fsync` is the raw syscall and would silently
lose the darwin guarantee. This is the one place in the server where std file
I/O is deliberately preferred over rustix.

**Failure handling.** A failed `sync_all` on the tmp file aborts the write
before the rename — the previous snapshot stays intact and untouched, the tmp
file is unlinked, the error is returned. After an fsync EIO the page cache
state is unknown (fsync(2) ERRORS), so the helper never retries the same fd; a
later save opens fresh files. A failed *directory* sync after a successful
rename is reported but the write is not rolled back — the content is correct,
only its durability against power loss within the next few seconds is weaker.

### D-M1-2 — File watching: rustix inotify + kqueue, no `notify`

**Decision:** implement config watching directly on `rustix`, which is already
a workspace dependency: `rustix::fs::inotify` (`init`/`add_watch`/`Reader`,
present in the pinned 1.1.4 sources) on Linux, `rustix::event::kqueue` with
`EventFilter::Vnode` + `NOTE_WRITE | NOTE_RENAME | NOTE_DELETE` on darwin
(also verified in 1.1.4). Two small cfg modules under
`amx-server/src/platform/watch/`, one shared test suite.

**Rejected — `notify`:** measured with `cargo tree` on notify 8.2.0: the Linux
build pulls `inotify`, `inotify-sys`, `libc`, `mio`, `log`, `walkdir`,
`same-file`, `bitflags`, `notify-types` — nine crates, two of which (`libc`,
`mio`) D-M0-3 deliberately evicted from the tree; the darwin build swaps in
`fsevent-sys` + `libc`. Taking a nine-crate dependency to watch one file in
one directory fails HACKING.md's lean-tree rule on its face, and amx needs
none of notify's value (recursive walks, cross-platform event normalization,
debouncers) — the watched set is a single well-known path.

**Mechanism.** Watch the config *directory*, not the file: editors and this
plan's own atomic writer replace files by rename, which kills a file-scoped
watch. Linux: one inotify watch on `$XDG_CONFIG_HOME/amx` filtered to events
naming `config.toml`. Darwin: kqueue vnode filters attach to fds, and a
directory fd reports `NOTE_WRITE` when its entry set changes — watch the
directory fd, and on any event re-open the file by path (an in-place edit is
additionally caught by a second vnode filter on the file fd itself, refreshed
after every rename). Both paths coalesce bursts with a short quiet window
(editors emit several ops per save) and then hand off to the shared reload
logic, so the platform modules contain file-descriptor plumbing only.

### D-M1-3 — `toml` is the one new dependency

`toml` 1.1 with `default-features = false, features = ["parse", "serde"]`,
in `amx-core`. Measured transitive cost: `toml_parser`, `toml_datetime`,
`serde_spanned`, `winnow` — four small parsing crates, no I/O, no unsafe
syscall surface. Config *types and parsing* live in `amx-core/src/config/`
(pure functions over `&str`, like the rest of amx-core); file reading and
watching are the server's. The client will read the same types when
client-side config lands (keys/theme, M4), which is why the types do not live
in amx-server.

### D-M1-4 — Where state lives

Confirmed in source: `Ctx.state_dir` is
`$XDG_STATE_HOME/amx/<session>` with a `$HOME/.local/state` fallback
(`amx-core/src/ctx.rs:183, 203, 287-301`), and today the server never creates
it. M1 gives it this layout:

```
$XDG_STATE_HOME/amx/<session>/
  session.json            the snapshot (D7): layout, labels, cwds, shorts
  history/
    <pane-uuid>.rows      opt-in scrollback sidecar, one per pane (D-M1-6)
```

Config is configuration, not state, so it lives under
`$XDG_CONFIG_HOME/amx/config.toml` — per user, shared by every session, hot
reloaded by each running server independently. `Env` grows a `config_home`
field (`$XDG_CONFIG_HOME`, empty-counts-as-unset like its siblings, fallback
`$HOME/.config`) and `Ctx` grows `config_path`. Nothing else about `Ctx`
changes; tests keep constructing an `Env` pointing at a tempdir and get an
isolated config path for free.

`state_dir` and `history/` are created on first write (with a one-time
`sync_all` on the parents they create, so the directories themselves survive
power loss); `session delete` must switch from `remove_dir` to a scoped
recursive delete now that the directory has contents — see R-M1-5.

### D-M1-5 — Snapshot schema: version 1, N/N−1 read window, unknown fields ignored

One file, `session.json`, serde through `serde_json`, shaped as flat tables
keyed by UUID exactly like the wire's `session.state` (04 §6: "keyed by UUID"
— herdr's positional workspace/tab zipping is the failure mode being
designed out):

```json
{
  "version": 1,
  "focused_workspace": "6f4a…",
  "workspaces": [
    { "id": "6f4a…", "short": 1, "label": "build",
      "layout": { "root": …, "zoomed": null }, "focus": "9c2e…" }
  ],
  "panes": [
    { "id": "9c2e…", "short": 1, "label": "editor", "cwd": "/home/s/amx" }
  ]
}
```

- `layout` is `amx_core::Layout` verbatim — it already derives serde for the
  wire, and reusing the wire form means one algebra, not a projection.
- `short` numbers are persisted because 04 §6 requires them "stable across
  restarts"; `Core` owns the maps today (`workspace_shorts`/`pane_shorts`,
  a documented stand-in while `ShortNumbers` is unimplemented — see R-M1-4).
- No argv: M1 restore respawns *shells* in saved cwds (05 M1); agent resume
  and argv-as-data are M2, and the version window means adding fields later
  is not a schema break.
- **Read window:** the reader accepts `version` in `{N−1, N}` (at v1, just
  `{1}`), refuses newer versions with a distinct error that restore reports
  as a loss ("snapshot from a newer amx") rather than logging, and ignores
  unknown fields — the same tolerance rule as the wire (04 §4), asserted by a
  test, so v2 can add fields without stranding v1 readers within the window.
- Client presentation state stays client-side (04 §6). Nothing in the client
  today asks to be persisted; the schema deliberately has nowhere to put it.

A checked-in golden freezes the v1 bytes
(`tests/goldens/persist/session-v1.json`) with the same
`AMX_UPDATE_GOLDENS=1` regeneration path as the protocol goldens, and the
version-window test is structured like `tests/skew.rs` — a row table where
supporting a new version means adding a row.

### D-M1-6 — Sidecar content: unstyled logical lines, honestly

`docs/notes/scrollback-identity.md` is explicit: "History is served as text.
… history rows carry their characters and no styling" — the packed `Cells`
layout that would make styled history possible exists only for the live grid
stream (`amx-proto/src/stream/cell.rs`), and `history/pack.rs` labels the
text-only row layout *provisional*. So M1 sidecars store what history
actually is today:

- **Format:** a small binary header (magic + format version + pane UUID +
  the `RowRange` covered), then rows in the existing packed layout
  (`u32 len, text bytes, u8 flags` — `amx-proto/src/stream/history.rs`),
  written through the same `put_row` the wire uses so the file format and the
  wire format cannot drift. Named `history/<pane-uuid>.rows`, **not**
  `.ansi` as 04 §6 writes — the content is not an ANSI replay stream and
  naming it one would be a small lie; flagged as R-M1-1 for a doc PR rather
  than silently absorbed.
- **Restore:** soft-wrapped rows are joined into logical lines (the wrap flag
  exists for exactly this) and replayed into the fresh pane's VT on the
  parser thread before PTY output flows, herdr's `initial_history_ansi`
  mechanism with text instead of ANSI. Replayed lines re-wrap at the current
  width and receive fresh RowIds through the normal tracker — no attempt is
  made to preserve old ids across a restart; the eviction-floor contract
  makes that unnecessary.
- **Opt-in** (04 §6: scrollback holds secrets): `[persist] history = true`.
  Turning it off wipes `history/` immediately, herdr's behavior for the same
  reason. Styled sidecars are deliberately deferred until the `Cells`
  packing serves history; when that lands the sidecar format version bumps
  inside the same file header.

### D-M1-7 — Debounce: quiet window plus a staleness cap

herdr's save debounce is a 5 s *trailing* window re-armed by every dirty
event (`SESSION_SAVE_DEBOUNCE`, `herdr/src/app/mod.rs:42`), so a continuously
busy session never saves until it goes quiet — up to unbounded staleness under
sustained change, and its own docs admit ~5 s of loss on hard crash. amx uses
two constants: a save fires after **500 ms of quiet** or **5 s after the first
unsaved change**, whichever comes first. Dirtiness is structural (pane and
workspace lifecycle, layout, labels, focus, cwds), never per-frame damage, so
the quiet path is the common one and the cap only bounds pathological loops.
Both constants live in one place, are named, and the actor's unit tests drive
them with tokio's paused virtual time (`tokio::time::pause` — the test-util
feature is already a dev-dependency), not wall-clock naps.

### D-M1-8 — Config reload semantics

Following herdr's two-tier lenient model (`load_live_config_from_str`,
`herdr/src/config/io.rs:238-349`), which is the shape 05 M1 names
"per-section lenient fallback", with two deliberate simplifications:

- **Whole-file failure** (unreadable, TOML parse error, top level not a
  table): keep the entire running config, report the failure. A typo while
  editing must never yank live settings.
- **Per-section failure:** a section that fails to deserialize keeps its
  current running values; every valid section still applies. Diagnostics name
  the section and the error.
- **Absent sections reset to defaults** — the file is declarative, and reload
  is idempotent with file content (herdr's behavior, kept on purpose; "keep
  the old value for a section the user deleted" makes the running config
  unreconstructable from the file).
- **Unknown keys are ignored silently.** herdr warns through `serde_ignored`;
  amx's wire contract is "unknown fields are ignored" and the config follows
  the same rule rather than taking a dependency to warn. Recorded as a
  divergence in R-M1-10; if the typo-UX cost proves real, `serde_ignored` is
  a one-crate, one-task addition later.

Sections in M1 are exactly the ones with consumers:

```toml
[persist]
history = false        # scrollback sidecars (D-M1-6); hot: off wipes, on dumps

[terminal]
shell = "/bin/zsh"     # pane spawn override; default $SHELL then /bin/sh; hot for new panes
```

The loaded config travels as a `tokio::sync::watch` channel (already a
dependency), so any actor can hold a receiver and observe changes without a
new subscription mechanism; a `ConfigReloaded` event on the bus makes reloads
visible to tests and, later, to `amx events` consumers.

### D-M1-9 — Restore ordering, and how it meets the first-attach seed

`session/serve.rs` assembles the server as: bind the gateway socket, spawn
`Core`, spawn the gateway accept loop. Restore slots between bind and spawn:
after the bind claims the session (losing the race is still a clean error),
the snapshot is loaded and applied through `Core` *before* the accept loop
starts, so the earliest possible client already sees the restored session.
The first-attach seed (`Core::seed_first_workspace`,
`actor/core/workspace.rs:81-121`) returns early when any workspace exists —
restore-then-attach therefore composes with zero changes to the seed: a
restored session is never re-seeded, an empty or failed restore falls through
to the normal seed, and both facts get tests.

**Prune-and-report semantics** (05 M1: "prune-and-report losses"; 04 §6:
"never log-only"):

| Failure | Action | Reported as |
|---|---|---|
| saved cwd no longer exists | respawn in `$HOME`, keep the pane | degraded (pane, old cwd) |
| shell spawn fails | prune the pane | lost (pane, label, cwd) |
| workspace loses every pane | prune the workspace | lost (workspace, label) |
| sidecar unreadable/torn | skip replay, keep the pane | degraded (pane) |
| snapshot unreadable or newer than the window | start fresh | lost (whole session, reason) |

The `RestoreReport` — full entries, not counts — lives in `Core` for the
server's lifetime, is served whole by `session.report`, and its summary
(counts) rides `session.state` so every attaching client can render the
status-line indicator without a second call.

---

## 2. The Persist actor

The fourth actor of 04 §2's table, filling the `Persist` row: "debounced
snapshot capture, fsynced writes, restore". Its contract, precisely, because
the shutdown-wedge flake (R-M1-2) makes sloppiness here expensive:

**Inputs.**

- A bus `Subscription` — dirtiness is *observed*, not pushed: Persist filters
  events to the structural set (`PaneCreated`, `PaneExited`, `PaneRenamed`,
  `WorkspaceCreated`, `WorkspaceRenamed`, `WorkspaceClosed`, `FocusChanged`,
  `LayoutChanged`, `PaneResized`) and treats a `Gap` as dirty without
  re-reading anything — a gap can only hide transitions, and the capture that
  follows reads current state anyway. This keeps `Core` free of any
  "remember to tell Persist" call sites; the event bus is the spine (04 §2)
  and forgetting to publish is already impossible.
- A typed mailbox, `PersistCommand`:
  - `Snapshot(Box<Snapshot>)` — an unsolicited capture push. Sent exactly
    once in normal operation: by `Core` on its shutdown path (below).
  - `Flush { reply: oneshot }` — force a save now; the deterministic handle
    the crash suite and the actor's own tests use instead of waiting out the
    debounce.
- A `watch::Receiver<Config>` for the `[persist]` section.

**The capture split.** Capture happens on the Core actor; writing happens on
Persist; fsync happens on the blocking pool. When the debounce fires, Persist
sends `SessionCall::Capture { reply }` through the ordinary `CoreHandle` —
no back door, same rule as the connection path. `Core` assembles the
`Snapshot` synchronously from `SessionState` + its shorts maps, refreshes
each pane's cwd through the existing `PaneCommand::ForegroundCwd` query with
a short per-pane budget (falling back to the stored cwd — a stale cwd is a
degraded restore, not a wrong one), and replies with the snapshot plus, when
sidecars are enabled, a clone of each pane's command handle. Persist then
serializes and hands the bytes to `spawn_blocking(write_atomic)` — fsync
(milliseconds on darwin under `F_FULLFSYNC`) never blocks the async
runtime. Persist is sequential by construction: a capture that arrives while
a write is in flight simply waits its turn in the actor loop, which replaces
herdr's re-arm-and-hope threading (`start_background_session_save`'s +250 ms
retry) with backpressure.

**Sidecars.** Persist tracks per-pane history heads from the same bus events
(`HistoryCommitted` raises, `HistoryInvalidated`/`HistoryEvicted` adjust) and
dumps only panes whose history moved since their last dump. Rows are read
through the pane handles from the capture reply — `PaneCommand::HistoryRange`
already executes on the parser thread and chunks (04 §3); at ~3.3 µs a row a
full 5,000-row pane costs ~17 ms of parser time per dump, which is why dumps
ride the debounced save and never any hotter path.

**Effects and events.** Persist returns no `Effect` (it renders nothing) and
publishes nothing in steady state. `SessionRestored` is published once by the
restore path at startup; `ConfigReloaded` belongs to the watcher task. Both
are new `Event` variants — the enum is `#[non_exhaustive]` for exactly this
(M0 plan R10).

**Shutdown, in order.** The runtime's `JoinSet` is flat — cancellation is
broadcast and join order is completion order (`runtime.rs:90-100`) — so
Persist's rules are chosen to need no ordering:

1. `Core::run`'s break path gains one call *before* it drops its mailbox:
   `persist.try_send(PersistCommand::Snapshot(capture_cheap()))` — a
   non-blocking push of a final capture built from stored state only (no
   pane queries; the panes are about to be killed). `try_send` so a full
   mailbox can never wedge Core's own drain.
2. Persist, on cancellation, stops arming timers and keeps draining its
   mailbox until it closes (Core's final push arrives, then the senders
   drop), then writes once if anything is dirty, then returns. It never
   sends a request to another actor after cancellation — the capture
   request/reply pattern is live-only — so it can never deadlock against a
   sibling that is also draining, and it adds no new edge to the wedge
   flake's graph (R-M1-2).
3. The blocking write it may be waiting on is bounded, local-disk fsync; if
   that hangs, `Runtime::shutdown` hangs visibly, which is the runtime's
   stated policy ("hang rather than leak", `runtime.rs:74-76`).

Kill -9 needs none of this to work: the debounced save plus atomic writes
mean the on-disk snapshot is always a complete, parseable recent state, which
is what the crash suite proves.

**Restore** is not the actor: it runs once at startup on the serve path
(D-M1-9), before the actor loop starts. `persist::load` (pure read +
validate + version window) feeds `Core::restore` (spawn + prune + report).
Persist's actor loop begins with a clean "nothing dirty" slate afterward.

---

## 3. Wire and event surface

Decided here so every task below can cite it (the "which need method-table
rows" question):

| Change | Kind | Goldens |
|---|---|---|
| `pane.rename { pane, label } → { seq }` | new method row | forced: coverage law derives owed names from `Method::ALL`, skew's `sample_params` match is exhaustive |
| `session.report {} → { seq, report }` | new method row | forced, same two mechanisms |
| `PaneState.label: Option<String>` in `session.state` | field addition | regenerate `session.state` goldens |
| `StateReply.restore: Option<RestoreSummary>` | field addition | same regeneration |
| `Event::SessionRestored { workspaces, panes, lost, degraded }` | new event variant | event envelope goldens added by U10 (the M2 `events --json` surface will freeze these; better frozen now) |
| `Event::ConfigReloaded { rejected_sections }` | new event variant | same |

Field additions stay inside protocol v1: unknown-field tolerance is the wire
contract both directions, `skip_serializing_if` keeps old goldens' shape for
absent values, and the N/N−1 window is untouched. `workspace.rename` already
exists (`control/mod.rs:213-220`) — M1 adds no second workspace verb, just its
persistence. The pane-rename plumbing below the wire already exists unwired:
`SessionState::rename_pane`, `Pane::set_label`, `Event::PaneRenamed`.

`amx session report` merges into the generated `session` subcommand group the
same way the lifecycle verbs do (`crates/amx/src/cli.rs:41-46`), with a small
hand-written formatter so losses read as a table rather than raw JSON — the
one verb where human output is the point.

---

## 4. Task DAG

Difficulty is `hard` when the task carries syscall, concurrency,
wire-compatibility or crash-semantics risk, `normal` otherwise. Every task
lands with tests that fail without the change, and finishes with
`cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
green (HACKING.md). File scopes are exclusive within a wave (§5).

---

### U01 — M1 contracts: types, rows, routing stubs

- **Difficulty:** hard · **Wave:** 0 · **Depends on:** —
- **Goal:** every type and seam the milestone needs, compiling, so waves 1–3
  never collide: later tasks fill bodies, never signatures.
- **Scope (owns exclusively):**
  `crates/amx-core/src/{ctx.rs,event/mod.rs,config/mod.rs (new)}`,
  `crates/amx-core/Cargo.toml`,
  `crates/amx-proto/src/control/{mod.rs,pane.rs,session.rs}`,
  `crates/amx-server/src/actor/mod.rs`,
  `crates/amx-server/src/actor/core/mod.rs` (routing arms + the split it
  forces), `crates/amx-server/src/dispatch/mod.rs` (delegating stubs),
  `crates/amx-server/src/persist/mod.rs` (new: schema types only),
  `tests/skew.rs` (`sample_params` arms), `tests/goldens/proto/**`
  (regeneration), `crates/amx-proto/tests/goldens.rs` (new rows).
- **Work:** `Env.config_home` + `Ctx.config_path` (D-M1-4). New `Event`
  variants `SessionRestored`/`ConfigReloaded` (§3). Method rows
  `pane.rename` + `session.report` with payload types; `PaneState.label` and
  `StateReply.restore` (+`RestoreSummary`, `RestoreReport`, `RestoreLoss`
  types); dispatch stubs answer through the existing `seam()`/
  `NOT_IMPLEMENTED` path (`dispatch/mod.rs:39-45`, dead code today, live
  now). `PersistCommand`, `SessionCall::Capture`, the `Snapshot` schema
  types with serde + the version constant. Core routing arms for
  `Capture`/`Rename` delegate to named submodule functions with stub bodies
  so U05–U07 never touch `core/mod.rs` — and since `core/mod.rs` sits at 493
  lines, the arms land together with the planned split (move the pane-report
  folding into `core/report.rs`) rather than pushing past the soft budget.
  Config skeleton: `Config`, `PersistConfig`, `TerminalConfig`, section
  names, `toml` dependency with its one-line justification. Regenerate
  protocol goldens; add the two skew `sample_params` arms (the exhaustive
  match refuses to compile without them — that is the point).
- **Acceptance:**
  - `pane_rename_and_session_report_answer_not_implemented_not_404`
  - `state_reply_with_restore_summary_round_trips_and_omits_when_none`
  - `snapshot_schema_round_trips_serde_and_ignores_unknown_fields`
  - `config_path_derives_from_passed_env_not_process_env`
  - `protocol_goldens_cover_every_control_method_and_stream_message` (the
    existing law, now passing with 18 rows)
  - `skew_harness_runs_current_against_current_and_fails_on_an_unhandled_variant`
    (existing, now exercising both new rows)
- **Prompt draft:** Land the M1 contracts for amx exactly as
  `docs/07-m1-plan.md` §1–§3 specifies: `config_home` on `Env` and
  `config_path` on `Ctx`; the `SessionRestored` and `ConfigReloaded` event
  variants; the `pane.rename` and `session.report` method-table rows plus the
  `label` and `restore` additions to `session.state`'s payloads; the
  `Snapshot`/`RestoreReport` schema types with their version constant; the
  `PersistCommand` mailbox and `SessionCall::Capture`; and the `[persist]`/
  `[terminal]` config type skeleton with the `toml` dependency justified in
  the commit body. New dispatch handlers answer through the existing `seam()`
  path — behavior comes later; signatures, serde shapes and routing are the
  deliverable, because three waves of tasks build against them in parallel.
  Adding rows breaks the goldens coverage law and the skew harness by design:
  regenerate the goldens, add the `sample_params` arms, and leave both suites
  green. `actor/core/mod.rs` has seven lines of budget left — land the new
  routing arms together with a split that moves the pane-report folding into
  its own module, and keep every new file well under 500 lines.

---

### U02 — Durable I/O and the snapshot file

- **Difficulty:** hard · **Wave:** 1 · **Depends on:** U01
- **Goal:** `write_atomic` with the D-M1-1 sequence, snapshot save/load with
  the version window, sidecar file codec, and the `session delete` fix.
- **Scope:** `crates/amx-server/src/persist/{io.rs,snapshot.rs,sidecar.rs}`
  (new), `crates/amx-server/src/session/registry.rs`,
  `crates/amx-server/tests/persist_io.rs`,
  `tests/goldens/persist/**` (new tree; no rig `.rs` files — the rig suite
  is U09's).
- **Work:** `write_atomic(dir, name, bytes, &impl Syncs)` where `Syncs` is a
  two-method seam (`sync_file`, `sync_dir`) defaulting to `File::sync_all` —
  the seam exists so a recording test can assert *ordering* (write → file
  sync → rename → dir sync), which is the only honest way to verify fsync
  discipline in CI; the real power-loss claim rests on the man-page-backed
  sequence, and the test pins the sequence. First-write directory creation
  syncs the parents it creates. `persist::load` applies D-M1-5's window:
  newer-version and corrupt files return distinct errors the restore path
  turns into report entries. Sidecar codec per D-M1-6 (header + `put_row`
  rows), torn-file detection (the packed reader already stops cleanly on a
  torn run). Switch `registry::delete` from `remove_dir` to a recursive
  delete scoped to the session's own two directories — it fails outright
  once `session.json` exists (R-M1-5).
- **Acceptance:**
  - `write_atomic_syncs_file_before_rename_and_directory_after`
  - `failed_file_sync_leaves_the_previous_snapshot_untouched`
  - `stray_tmp_files_are_ignored_by_load_and_removed_by_the_next_save`
  - `snapshot_v1_golden_matches` / `newer_version_is_refused_with_a_reportable_error`
  - `corrupt_snapshot_is_refused_not_partially_applied`
  - `sidecar_round_trips_rows_and_detects_truncation`
  - `session_delete_removes_populated_state_dir`
- **Prompt draft:** Implement amx's durable persistence I/O in
  `amx-server/src/persist/` per `docs/07-m1-plan.md` D-M1-1: a single
  `write_atomic` helper doing tmp-write → `File::sync_all` on the tmp file →
  rename → `sync_all` on an fd of the containing directory, with
  pid+counter tmp names, std (not rustix) sync calls because std's
  `sync_all` is `F_FULLFSYNC` on darwin, and a two-method sync seam so a
  test can record and assert the exact ordering. On top of it: snapshot
  save/load with the version-1 schema U01 froze, an N/N−1 read window that
  refuses newer versions with a reportable error, unknown-field tolerance
  matching the wire rules, and a checked-in v1 golden regenerable under
  `AMX_UPDATE_GOLDENS=1`; and the sidecar codec — header plus the wire's own
  `put_row` layout so file and wire can never disagree. Read herdr's
  `src/persist/io.rs` and `src/detect/manifest_update.rs` for contrast
  (the latter is the only fsynced write herdr has), and write amx's own
  code — never copy lines. Also fix `session delete`: `remove_dir` on the
  state directory fails the moment it has contents.

---

### U03 — Config model and lenient parsing

- **Difficulty:** normal · **Wave:** 1 · **Depends on:** U01
- **Goal:** `Config::from_str`-style pure parsing with D-M1-8's exact
  semantics and diagnostics.
- **Scope:** `crates/amx-core/src/config/**` (bodies; U01 made the
  skeleton), `crates/amx-core/tests/config.rs`.
- **Work:** whole-file failure keeps everything (the parse function takes
  the *current* config and returns next-config + diagnostics, so "keep" is
  structural, not a caller convention); per-section failure keeps that
  section's current values; absent sections are defaults; unknown keys
  ignored. Diagnostics are typed (`ConfigDiagnostic { section, message }`),
  not strings assembled at the call site, because the reload event carries a
  count and `session report`-style surfaces may want them later.
- **Acceptance:**
  - `parse_error_keeps_the_entire_running_config`
  - `invalid_section_keeps_current_values_while_valid_sections_apply`
  - `absent_section_resets_to_defaults`
  - `unknown_keys_and_sections_are_ignored_like_the_wire`
  - `diagnostics_name_the_failing_section`
- **Prompt draft:** Implement amx's config parsing in
  `amx-core/src/config/` as pure functions over `&str`: no file I/O, no
  process env — the server owns reading and watching. The semantics are
  `docs/07-m1-plan.md` D-M1-8 exactly: a file-level TOML error keeps the
  entire running config; a section that fails to deserialize keeps that
  section's running values while every valid section applies; a section
  absent from the file resets to its defaults so reload is idempotent with
  file content; unknown keys are silently ignored, the same tolerance rule
  as the wire. Model the operation as
  `(current: &Config, text: &str) -> (Config, Vec<ConfigDiagnostic>)` so
  "keep current" is enforced by the signature. herdr's
  `load_live_config_from_str` (`herdr/src/config/io.rs`) is the reference
  mechanism for the two-tier fallback — study it, then write your own; the
  differences (absent-section behavior is the same, unknown-key warnings are
  deliberately dropped) are listed in the plan.

---

### U04 — File watcher platform layer

- **Difficulty:** hard · **Wave:** 1 · **Depends on:** U01
- **Goal:** one `watch_config(ctx, cancel) -> impl Stream`-shaped seam with
  inotify and kqueue implementations that survive the rename-based writes
  editors and `write_atomic` both do.
- **Scope:** `crates/amx-server/src/platform/watch/{mod.rs,linux.rs,darwin.rs}`
  (new), `crates/amx-server/tests/watch.rs`.
- **Work:** D-M1-2's mechanism: watch the directory, filter to
  `config.toml`, coalesce bursts behind a short quiet window, deliver "the
  file may have changed" (never file contents — reloading is the consumer's
  job, so the platform modules stay fd-plumbing only). Create
  `$XDG_CONFIG_HOME/amx` if absent so the watch always has a directory.
  Darwin re-arms the file-fd vnode filter after every rename event. The
  shared test suite runs identically on both platforms (macOS is tier 1 and
  CI-enforced): touch, rename-over, delete-then-recreate, editor-style
  write-tmp-then-rename, burst coalescing — paced by the rig's TICK
  conventions, no wall-clock naps.
- **Acceptance:**
  - `rename_over_the_config_file_is_observed` (the atomic-writer case)
  - `in_place_write_is_observed`
  - `delete_then_recreate_resumes_watching`
  - `a_burst_of_writes_coalesces_to_one_notification`
  - `unrelated_files_in_the_directory_are_ignored`
  - `watcher_stops_promptly_on_cancellation`
- **Prompt draft:** Build amx's config file watcher in
  `amx-server/src/platform/watch/` directly on rustix — no `notify`, which
  was measured to pull nine crates including the `libc` and `mio` the tree
  deliberately evicted (see D-M1-2 in `docs/07-m1-plan.md`). Linux uses
  `rustix::fs::inotify` with one watch on the config *directory* filtered to
  `config.toml`; darwin uses `rustix::event::kqueue` vnode filters on the
  directory fd, re-opening and re-arming the file fd after every rename,
  because kqueue watches fds and a renamed-over file is a new fd. Watching
  the directory rather than the file is the load-bearing choice: editors and
  amx's own atomic writer replace the file by rename, which silently kills a
  file-scoped watch. Deliver coalesced "changed" notifications only — no
  reading, no parsing. The same test suite must pass on both platforms;
  macOS is tier 1 and its pty/timing quirks are documented in
  `tests/support/term.rs` — follow the rig's TICK pacing conventions and
  never sleep on the wall clock.

---

### U05 — The Persist actor

- **Difficulty:** hard · **Wave:** 2 · **Depends on:** U01, U02
- **Goal:** §2's actor: bus-observed dirtiness, quiet+cap debounce, capture
  request, blocking-pool writes, sidecar dumps, the shutdown drain contract.
- **Scope:** `crates/amx-server/src/actor/persist.rs` (new),
  `crates/amx-server/tests/persist_actor.rs`.
- **Work:** exactly §2. The actor is driven by `select!` over the bus
  subscription, the mailbox, the debounce timer, and cancellation; unit
  tests run under `tokio::time::pause` so the 500 ms/5 s constants are
  tested at virtual speed. Sidecar dumps skip panes whose history head has
  not moved. `Flush` exists for determinism. On cancellation: no more
  timers, no more capture requests, drain mailbox to closure, final write if
  dirty, return.
- **Acceptance:**
  - `a_structural_event_schedules_a_save_after_the_quiet_window`
  - `continuous_change_saves_at_the_staleness_cap_not_never` (herdr's
    starvation, disproven here)
  - `pane_damage_events_never_schedule_a_save`
  - `a_gap_on_the_bus_is_treated_as_dirty`
  - `capture_request_goes_through_the_core_mailbox_not_a_back_door`
  - `writes_happen_on_the_blocking_pool_and_serialize_behind_one_another`
  - `sidecar_dump_skips_panes_whose_history_did_not_move`
  - `cancellation_drains_the_mailbox_flushes_once_and_returns`
  - `no_request_is_sent_to_another_actor_after_cancellation`
- **Prompt draft:** Implement the `Persist` actor in
  `amx-server/src/actor/persist.rs` against the contract in
  `docs/07-m1-plan.md` §2. Dirtiness is observed from the event bus — filter
  to the structural variants, treat a `Gap` as dirty, never look at damage
  events — so `Core` carries no "remember to notify persistence" call sites.
  Debounce with two named constants, 500 ms of quiet or 5 s after the first
  unsaved change, tested under paused tokio time, not wall-clock sleeps.
  When the timer fires, request a capture through the ordinary `CoreHandle`
  (`SessionCall::Capture`), serialize, and write via
  `spawn_blocking(write_atomic)` so fsync — milliseconds on darwin under
  `F_FULLFSYNC` — never stalls the runtime; the actor is sequential, so
  concurrent saves are impossible by construction rather than by flag.
  Sidecars dump only panes whose history head moved, through the pane
  handles the capture reply carries. The shutdown rules exist because of a
  known rare wedge in the JoinSet drain: after cancellation, arm nothing,
  request nothing from any other actor, drain your mailbox until it closes
  (Core pushes a final capture there), write once if dirty, and return.

---

### U06 — Core: capture, restore, loss report

- **Difficulty:** hard · **Wave:** 2 · **Depends on:** U01, U02
- **Goal:** the Core side of persistence: capture assembly, restore apply
  with prune-and-report, the report served over `session.report` and
  summarized in `session.state`, and the final-capture push on shutdown.
- **Scope:** `crates/amx-server/src/actor/core/{persist.rs (new),view.rs}`,
  `crates/amx-server/src/actor/core/workspace.rs`,
  `crates/amx-server/src/dispatch/mod.rs` (fill U01's `session.report` and
  `session.state` stubs), `crates/amx-server/src/session/serve.rs`,
  `crates/amx-server/tests/restore.rs`.
- **Work:** `Capture` handler per §2 (state + shorts + `ForegroundCwd`
  refresh with fallback); `capture_cheap` for the shutdown push, wired into
  `Core::run`'s break path before the mailbox drop; `Core::restore(snapshot,
  sidecars)` applying D-M1-9's table through the existing spawn paths
  (`spawn_pane` is called, not modified — U07 owns `core/pane.rs`), keeping
  UUIDs and shorts, publishing the normal per-entity events plus one
  `SessionRestored`; sidecar replay through a `PaneCommand` seed executed on
  the parser thread before PTY output; `RestoreReport` stored on `Core`,
  served by `session.report`, counts in `StateReply.restore`. `serve.rs`
  gains the load-and-restore step between bind and spawn (D-M1-9).
- **Acceptance:**
  - `capture_reflects_labels_layout_focus_shorts_and_cwds`
  - `capture_falls_back_to_stored_cwd_when_the_probe_stalls`
  - `restore_rebuilds_workspaces_panes_and_shorts_with_the_same_uuids`
  - `restored_session_suppresses_the_first_attach_seed`
  - `empty_or_missing_snapshot_falls_through_to_the_normal_seed`
  - `missing_cwd_respawns_in_home_and_reports_degraded`
  - `spawn_failure_prunes_the_pane_and_reports_lost`
  - `workspace_losing_every_pane_is_pruned_and_reported`
  - `session_report_returns_entries_and_session_state_carries_counts`
  - `core_shutdown_pushes_a_final_capture_without_blocking`
- **Prompt draft:** Wire persistence through the `Core` actor per
  `docs/07-m1-plan.md` §2 and D-M1-9, filling the stubs U01 routed into
  `core/persist.rs`. Capture assembles the snapshot U01's schema defines
  from `SessionState` plus the shorts maps, refreshing pane cwds through
  `PaneCommand::ForegroundCwd` with a bounded per-pane budget and stored-cwd
  fallback. Restore runs on the serve path between the gateway bind and the
  accept loop: apply the snapshot through the existing spawn paths with the
  same UUIDs and shorts, replay sidecar lines into the fresh VT on the
  parser thread before PTY output flows, and implement prune-and-report
  exactly as the plan's table states — a missing cwd degrades to `$HOME`, a
  failed spawn prunes the pane, an emptied workspace prunes, and every one
  of these becomes a `RestoreReport` entry, never a log line (that
  log-only-ness is herdr's W8). The report is served whole by
  `session.report` and as counts in `session.state`. On Core's shutdown
  break, `try_send` a final cheap capture to Persist before dropping the
  mailbox — non-blocking, because nothing in the drain path may wait on a
  sibling. The first-attach seed already no-ops when workspaces exist;
  prove restore composes with it in both directions.

---

### U07 — Rename verbs, loss indicator, `session report` output

- **Difficulty:** normal · **Wave:** 2 · **Depends on:** U01
- **Goal:** the user-visible M1 surface: `pane.rename` end to end, the
  status-line loss indicator, and human-readable `amx session report`.
- **Scope:** `crates/amx-server/src/actor/core/pane.rs`,
  `crates/amx-server/src/dispatch/pane.rs`,
  `crates/amx-client/src/app/{mod.rs,status.rs (new)}`,
  `crates/amx-client/src/app/wired.rs`, `crates/amx-client/src/model.rs`,
  `crates/amx/src/cmd/session.rs`, `crates/amx-server/tests/rename.rs`.
- **Work:** fill U01's rename stub: handler → `SessionState::rename_pane`
  (already exists) → `Event::PaneRenamed` (already exists) → `Effect`.
  Client: fold `StateReply.restore` and pane labels into the model; the
  status line shows `⚠N` while the restore report is non-empty (rendered
  from state like the workspace label is today, cached the same way).
  `app/mod.rs` is already over the soft budget at 561 lines — the status
  logic moves to `app/status.rs` as part of this task, not as a favor
  later. `amx session report` gets a small formatter: one line per loss
  entry, kind, label, path; JSON stays available through the generated
  `--params` path untouched.
- **Acceptance:**
  - `pane_rename_persists_across_the_wire_and_republishes_state`
  - `workspace_and_pane_labels_survive_snapshot_and_restore` (with U06's
    restore, via the shared test server)
  - `status_line_shows_loss_count_after_a_lossy_restore`
  - `status_line_is_clean_after_a_clean_restore`
  - `session_report_formats_entries_human_readably`
  - `app_status_module_is_extracted_and_mod_rs_returns_under_soft_budget`
- **Prompt draft:** Finish amx's M1 user surface. Server: fill the
  `pane.rename` stub — the state mutation (`rename_pane`), the event
  (`PaneRenamed`) and the wire row all exist after U01; connect them and
  return the right `Effect`. Client: `session.state` now carries pane
  labels and an optional restore summary — fold both into the model, and
  render `⚠N` in the status line while the summary reports losses, built
  from mirrored state exactly the way the workspace label is today and
  cached so the repaint path stays allocation-free. `app/mod.rs` is over
  the 500-line soft budget already: extract the status-line logic into
  `app/status.rs` as part of this change. CLI: give `amx session report` a
  hand-written formatter (one loss per line: kind, label, path) merged into
  the generated `session` group the same way the lifecycle verbs are; the
  JSON form remains for scripts. Every piece lands with a test that fails
  without it.

---

### U08 — Config wired in: load, watch, hot apply

- **Difficulty:** normal · **Wave:** 3 · **Depends on:** U03, U04, U05, U06
- **Goal:** the running server loads config at startup, reloads on file
  change, applies per D-M1-8, and announces it.
- **Scope:** `crates/amx-server/src/config_rt.rs` (new: load + watch task +
  `watch::Sender`), `crates/amx-server/src/session/serve.rs` (spawn the
  watcher task; sequential edit after U06), `crates/amx-server/src/actor/persist.rs`
  (consume `[persist]`; sequential after U05),
  `crates/amx-server/src/actor/core/pane.rs` (shell override; sequential
  after U07), `crates/amx-server/tests/config_reload.rs`.
- **Work:** startup load (missing file = defaults, not an error); watcher
  task under the runtime's `JoinSet` publishing `ConfigReloaded
  { rejected_sections }` and sending on the `watch` channel; `[terminal]
  shell` consulted at spawn (config → `$SHELL` → `/bin/sh`); `[persist]
  history` hot toggle — enabling dumps on the next save, disabling wipes
  `history/` immediately (D-M1-6). Reload tests are event-driven: subscribe,
  rewrite the file, wait on `ConfigReloaded` — no polling, no naps.
- **Acceptance:**
  - `startup_without_a_config_file_uses_defaults`
  - `editing_shell_applies_to_new_panes_without_restart`
  - `enabling_history_dumps_sidecars_on_the_next_save`
  - `disabling_history_wipes_sidecars_immediately`
  - `a_broken_edit_keeps_the_running_config_and_reports_rejected_sections`
  - `reload_publishes_config_reloaded_with_the_rejection_count`
- **Prompt draft:** Wire config into the running amx server. At startup,
  read `Ctx.config_path` (a missing file is defaults, never an error) and
  put the result on a `tokio::sync::watch` channel; spawn the U04 watcher
  as a runtime task that, on change, re-reads the file, runs U03's lenient
  parse against the *current* config, sends the result, and publishes
  `Event::ConfigReloaded` with the rejected-section count. Consumers:
  pane spawn consults `[terminal] shell` before `$SHELL` before `/bin/sh`
  for new panes only; the Persist actor observes `[persist] history` —
  enabling it dumps sidecars at the next save, disabling wipes the
  `history/` directory immediately, which is the same secrets-first rule
  herdr applies. Write the reload tests event-first: subscribe to the bus,
  rewrite the file, wait for `ConfigReloaded` — the rig's hygiene rules
  forbid wall-clock naps, and the event exists precisely so nothing polls.

---

### U09 — Crash suite and the M1 exit test

- **Difficulty:** hard · **Wave:** 3 · **Depends on:** U02, U05, U06, U07
- **Goal:** the milestone's proof, in the rig, over the real binary and the
  real socket.
- **Scope:** `tests/persistence.rs` (new), `tests/Cargo.toml` (the target
  row), `tests/support/env.rs` (one addition: restart-preserving-state
  helper), `tests/goldens/persist/**` additions if the suite freezes more
  bytes.
- **Work:** four pillars.
  1. **The reboot test** (05 M1 exit): populate a session over the wire —
     two workspaces with labels, splits with distinct cwds under the temp
     root, a renamed pane, marker shells (`tests/firstrun.rs` technique) —
     wait for a save, `SIGKILL` the server, start a new server on the same
     state dir, attach, and assert: same workspace/pane UUIDs and shorts via
     `session.state`, same labels, same layout shape, respawned shells in
     the saved cwds (found by cwd through the platform helpers — the darwin
     libproc path already exists in the verbs tests), empty loss report.
  2. **Kill -9 mid-write atomicity:** a loop that mutates structure over
     the wire while killing the server at TICK-paced random points and
     restarting; after every restart the snapshot parses and restore
     reports no corruption — the file is always either the old or the new
     complete state. Plus the U02-level stray-tmp assertions at rig level.
  3. **Loss reporting end to end:** save with a pane whose cwd is then
     deleted; restart; `amx session report` lists the degraded pane, the
     attached client's status line shows `⚠1` (rasterized, the
     `wait_settled` way), and `session.state` carries the count.
  4. **Sidecar restore:** with `[persist] history = true`, fill a pane with
     marker lines, save, SIGKILL, restart, attach, scroll back (or fetch
     `pane.history` over the wire) and find the markers; with the flag off,
     assert no `history/` files exist.
  All waits condition-paced at TICK against the rig's patience budget; kill
  -9 of the *server* is new to the rig (today only clients die that way —
  `tests/adversarial.rs` kills clients exclusively) and inherits the drain
  rules of `tests/support/term.rs` on darwin.
- **Acceptance:** the four test names above, roughly:
  - `reboot_restores_workspaces_panes_labels_cwds_and_shorts`
  - `kill_dash_9_mid_write_always_leaves_a_restorable_snapshot`
  - `restore_loss_is_reported_in_status_line_and_session_report`
  - `sidecars_restore_scrollback_only_when_opted_in`
- **Prompt draft:** Build M1's crash suite in the rig (`tests/`), against
  the real binary over the real socket, following the harness conventions
  in `tests/support/` — TICK-paced condition waits, no wall-clock naps, the
  darwin drain rules, sun_path-short temp roots. Four tests carry the
  milestone's exit criteria. The reboot test populates a session over the
  wire (labeled workspaces, splits in distinct cwds, a renamed pane, marker
  shells), waits for a save, SIGKILLs the server — the rig has never killed
  a server before; clients only — restarts it on the same state directory
  and asserts UUIDs, shorts, labels, layout and cwds all came back and the
  loss report is empty. The atomicity test mutates state while killing the
  server at random TICK-paced moments in a loop, asserting every restart
  restores cleanly — the snapshot must always be the old or the new file,
  never a torn one. The loss test deletes a saved cwd before restart and
  asserts the degradation is visible in all three places: `session.report`,
  the `session.state` counts, and a `⚠1` rasterized off the attached
  client's status line. The sidecar test proves scrollback markers survive
  a reboot when `[persist] history` is on and that no sidecar files exist
  when it is off.

---

### U10 — Integration: the seams, named

- **Difficulty:** hard · **Wave:** 4 · **Depends on:** U05–U09
- **Goal:** the wired milestone, with every cross-crate/cross-actor seam M1
  created proven end to end — the T19 lesson applied on purpose instead of
  discovered late.
- **Scope:** by exception (as T19's was), the seams themselves:
  `session/serve.rs` final form (restore → persist → watcher assembly and
  their shutdown), any residual stub, `tests/integration.rs` additions, the
  event-envelope goldens for `SessionRestored`/`ConfigReloaded` (freezing
  the shapes `amx events --json` will expose in M2), and doc corrections
  discovered en route (R-M1-1's `.ansi` naming among them).
- **The seams it owns:**
  - **persist↔core:** the capture request/reply under load; the shutdown
    final-push ordering (Core pushes, then drops; Persist drains, then
    writes) exercised repeatedly under `session stop` and SIGTERM, watching
    for the wedge (R-M1-2) — this seam must leave the shutdown drain no
    worse than M0 left it.
  - **restore↔pane-spawn:** restored panes vs. the first-attach seed vs.
    sidecar replay ordering on the parser thread (replay strictly before
    first PTY output).
  - **config-reload↔running-actors:** a reload racing a save (watch channel
    vs. debounce timer), a reload racing pane spawn.
  - **wire:** the two new methods and two new fields end to end from a
    fresh client — attach to a lossy restored session and see everything
    (§3's table complete, including the event goldens).
- **Acceptance:**
  - `a_full_m1_cycle_survives_shutdown_reboot_rename_reload_and_reports_honestly`
    (the scripted end-to-end pass)
  - `repeated_clean_shutdowns_under_load_leave_no_hung_drain` (bounded
    repetitions, wedge-watch)
  - `reload_racing_a_save_corrupts_nothing`
  - `event_envelope_goldens_cover_session_restored_and_config_reloaded`
  - every stub introduced in U01 is gone (`grep`-level assertion that
    `seam()` has no callers again)
- **Prompt draft:** Integrate M1. The waves before you were kept parallel
  by exclusive file ownership, which means — as M0's T19 proved — nobody
  owned the places where they meet; you do, by exception. Take
  `session/serve.rs` to its final shape: bind, load-and-restore, spawn
  core/persist/watcher/gateway, and shutdown that provably drains — then
  exercise the three seams the plan names: capture and final-push between
  Core and Persist under repeated `session stop`/SIGTERM (the repo has a
  rare SIGTERM-immune JoinSet-drain wedge; your job is to demonstrate M1
  made it no more likely, with a bounded repetition test), restore against
  the first-attach seed and the sidecar replay ordering, and config reload
  racing saves and spawns. Close the wire surface: end-to-end tests through
  a fresh client for `pane.rename`, `session.report`, and the
  `session.state` additions against a genuinely lossy restore, plus event
  envelope goldens for the two new event variants so their shape is frozen
  before M2's `amx events --json` exposes them. Remove every `seam()` stub;
  if anything you find contradicts `docs/04-architecture.md`, flag it in
  the PR — the `.ansi` sidecar naming in 04 §6 is already known to need a
  doc correction.

---

## 5. Waves and merge order

Merge in wave order; within a wave, any order — no two tasks in a wave touch
the same file.

| Wave | Tasks | Concurrency | Unblocks |
|---|---|---|---|
| 0 | **U01** contracts | 1 | everything |
| 1 | U02 durable I/O · U03 config parse · U04 watcher | 3 | wave 2 |
| 2 | U05 Persist actor · U06 Core capture/restore · U07 renames + UI | 3 | wave 3 |
| 3 | U08 config wire-in · U09 crash suite | 2 | U10 |
| 4 | **U10** integration | 1 | M1 exit |

**File-ownership check for concurrent waves** (no overlaps):

- Wave 1 — U02: `amx-server/src/persist/{io,snapshot,sidecar}.rs`,
  `session/registry.rs`, `amx-server/tests/persist_io.rs`,
  `tests/goldens/persist/**`. U03: `amx-core/src/config/**`,
  `amx-core/tests/config.rs`. U04: `amx-server/src/platform/watch/**`,
  `amx-server/tests/watch.rs`. Disjoint.
- Wave 2 — U05: `amx-server/src/actor/persist.rs`,
  `amx-server/tests/persist_actor.rs`. U06:
  `actor/core/{persist.rs,view.rs,workspace.rs}`, `dispatch/mod.rs`,
  `session/serve.rs`, `amx-server/tests/restore.rs`. U07:
  `actor/core/pane.rs`, `dispatch/pane.rs`, client files,
  `cmd/session.rs`, `amx-server/tests/rename.rs`. Disjoint — U06 *calls*
  `spawn_pane`, only U07 *edits* its file; U01 already planted the routing
  arms both fill, so neither touches `core/mod.rs`.
- Wave 3 — U08: `config_rt.rs` plus sequential edits to `serve.rs` (U06's,
  prior wave), `actor/persist.rs` (U05's), `actor/core/pane.rs` (U07's),
  and `amx-server/tests/config_reload.rs`. U09: `tests/**` (rig only).
  Disjoint.
- Wave 4 — U10 runs alone and owns the seams by exception.

Cross-wave sequential edits are declared, not discovered: U08 extends three
prior-wave files; U09 adds one helper to `tests/support/env.rs`; U10 may
touch anything the integration requires.

---

## 6. Risks & findings

Flagged for the orchestrator, not silently resolved.

**R-M1-1 — 04 §6's sidecar filename implies content M1 does not produce.**
04 writes `history/<pane-uuid>.ansi`; the note in
`docs/notes/scrollback-identity.md` and `history/pack.rs` are explicit that
history rows are unstyled text, so the sidecar stores packed text rows and is
named `.rows` (D-M1-6). Styled sidecars are gated on the `Cells` packing
being applied to history, a known future function. 04 §6 needs a one-word
doc PR; the mechanism (per-pane files keyed by UUID, opt-in, independently
wiped) is implemented exactly as designed.

**R-M1-2 — The shutdown-wedge flake bounds the Persist design.** A rare
SIGTERM-immune hang in the JoinSet drain has been observed under load and is
not yet diagnosed. The runtime joins tasks in completion order with no
inter-actor ordering (`runtime.rs:90-100`), so M1 adds no design that
*requires* ordering: Persist's shutdown is receive-only (drain own mailbox to
closure, one local write, return), Core's contribution is a `try_send` that
cannot block, and no actor sends a request to a sibling after cancellation.
U10 carries a bounded-repetition shutdown test as a tripwire. What M1 must
not do — and does not — is add a shutdown path where one draining task awaits
another.

**R-M1-3 — Two files sit at the module-budget edge before M1 starts.**
`actor/core/mod.rs` is 493/500 and U01 must add routing arms — the split
(pane-report folding out to `core/report.rs`) is part of U01's scope, not an
emergency later. `amx-client/src/app/mod.rs` is already 561 and warning;
U07's status work moves to `app/status.rs` in the same change. Neither
split waits for the hard limit.

**R-M1-4 — Short numbers are persisted from a documented stand-in.**
`amx_core::ShortNumbers::assign/resolve` are still `todo!()`; `Core` tracks
shorts in two local maps (`core/mod.rs:79-86`) that the snapshot now
persists. That is correct for M1 (the maps *are* the display mapping, and
restoring them keeps 04 §6's "stable across restarts" promise), but when
`ShortNumbers` is finally implemented it must adopt the persisted-map
semantics or the snapshot schema's `short` fields change meaning. Recorded
so the eventual implementation knows it has a wire-adjacent constraint.

**R-M1-5 — `session delete` breaks the moment state exists.**
`registry.rs:237` removes `state_dir` with `remove_dir`, which fails on a
non-empty directory — untestable before M1 because nothing ever wrote there.
U02 scopes it to a recursive delete of the session's own directory.
Destructive-op caution: the delete stays pinned under
`$XDG_STATE_HOME/amx/<session>` derived from `Ctx`, never a user-supplied
path.

**R-M1-6 — darwin durability and watching differ, and macOS is CI-enforced
tier 1.** `F_FULLFSYNC` (what std's `sync_all` issues on Apple platforms) is
drastically slower than Linux `fsync` — fine at debounce cadence, and the
reason fsync lives on the blocking pool, but a reason the debounce constants
should not shrink casually. kqueue gives coarser directory events than
inotify (no per-name attribution), so the darwin watcher re-checks the file
by path on every directory event; the shared watcher test suite runs on both
platforms in CI. Existing darwin rig lore (pty drain rules, libproc process
probes, sun_path budget) is already encoded in `tests/support/` and U09
inherits it.

**R-M1-7 — cwd freshness is capture-time, not tracked.** amx has no OSC 7
plumbing; `Pane.cwd` is set at split time. Capture refreshes it via
`ProcessTree`'s foreground-cwd probe (the `PaneCommand::ForegroundCwd` query
that already exists), so a snapshot's cwd is as fresh as the last save, and
the shutdown push deliberately skips the probe (panes are dying) and uses
last-known values. A shell that `cd`s and is killed within the debounce
window restores to its previous directory — bounded by D-M1-7's 5 s cap,
and judged acceptable against the alternative (per-cd tracking needs OSC 7
or polling; either is out of M1 scope).

**R-M1-8 — The new `session.state` fields ride protocol v1.** Additive
optional fields under the both-directions unknown-field contract need no
version bump and stay inside the N/N−1 window; the goldens regenerate and
the skew suite gains the new rows through its exhaustive matches. If a
*reader*-visible semantic ever changes (not the case here), that is a
version bump — stated so nobody later mistakes this precedent for "fields
are always free".

**R-M1-9 — Observed en route, not M1's to fix:** `GridMessage::Scrolled` is
defined, golden-tested and client-decoded, but no server path encodes it —
the scrolled-row announcements of 04 §3 are not yet emitted. Persistence
does not depend on them (sidecars read through `HistoryRange`), so this is
recorded for the backlog rather than absorbed into M1.

**R-M1-10 — Deliberate divergences from herdr, recorded:** no symlink
chasing on state writes (herdr chases up to 16 hops in `persist/io.rs`
because dotfile-managed *config* dirs contain symlinks; amx state files are
machine-owned and a symlinked `session.json` is replaced by the rename —
config, which users do stow, is only ever *read*); no unknown-key warnings
in config (wire-consistency, D-M1-8 — revisit with `serde_ignored` if the
UX asks); quiet+cap debounce instead of trailing-only (D-M1-7); loss
reporting is queryable state instead of `warn!` lines (the entire point of
D7). Each is a design choice with its reason, not an oversight.

**R-M1-11 — The restore report does not survive its server.** The report
lives in `Core` for the run that performed the restore; a second restart
reports only its own restore. Persisting reports to disk was considered and
dropped — the losses it describes are actionable at the moment of restore,
and `session.json` reflects post-restore reality from the next save onward.
If report history is ever wanted it is one more state file behind the same
`write_atomic`.
