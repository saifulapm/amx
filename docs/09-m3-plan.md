# M3 execution plan

The build plan for **M3 — Continuity & reach** ([05-roadmap.md](05-roadmap.md)).
Binding design is [04-architecture.md](04-architecture.md) §6 (handoff and the
client-across-handoff resync), §4 (the skew window and keyframes) and §1 (the
SSH byte bridge); this document does not change them. Where research
contradicted or complicated a decision in 04/05, it is recorded in
[§8 Risks](#8-risks--findings) rather than silently redesigned.

Everything below that states a fact about the amx tree, herdr, the kernel, or
rustix was read from source or man pages during planning. The single largest
unknown — why the JoinSet drain sometimes wedges — is deliberately **not**
resolved here: that is W01, the diagnosis spike, and [§2](#2-the-shutdown-wedge-spike-w01)
states why handoff cannot merge over an undiagnosed drain and what happens
under each outcome.

Process lessons inherited from M0/M1/M2 and applied throughout: a named
integration task owns every cross-crate seam (06 §T19, and M2's W-1 — the hub
and the gateway were both correct and never met); a task that adds a capability
to *connections or the server's lifecycle* must name the `session/serve.rs`
assembly in its scope or the integration task inherits the join silently; the
contracts task splits budget-edge files before the waves press them (07
R-M1-3, 08 R-M2-5); and the milestone does not exit on green tests — three
times now a green suite has hidden a non-working feature until a live run
caught it, so the exit is green tests **plus** the recorded live smoke of §7.

---

## 1. Decisions taken during research

### D-M3-1 — The wedge spike gates the export path, not the milestone's width

`Runtime::shutdown` cancels and then joins every task in completion order with
no inter-task ordering (`runtime.rs:90-100`), and roughly once in 300 runs
under 8-way CPU load a server survives SIGTERM parked in that drain — observed
2026-08-07 on the T17 CLI suite: five open `/dev/ptmx` masters in tests that
attach once, an unreaped zombie pane child, main thread in futex wait. Suspect
area is the Core↔PaneHost interaction during cancel; it has never been
diagnosed (08 R-M2-6 designed around it, as M1 did).

M3 cannot design around it any longer. The handoff exporter's life ends in
exactly this drain — quiesce panes, retire the gateway, join everything — and
it runs on every single upgrade instead of once per `session stop`. A wedged
exporter *after* commit is a leaked process holding dead pty fds; a wedged
exporter *before* commit is a stranded upgrade with panes frozen. Either turns
the milestone's headline feature into a coin flip.

**Decision:** W01 is wave 0 and runs alone (its reproduction needs the machine
loaded on purpose, which no concurrent task should share). It gates **W06, the
export path** — nothing that quiesces-and-joins in anger merges until the
drain is understood. It does not gate the wider milestone: the manifest codec,
the fd transport, the resync, self-update's CLI half, the bridge, worktrees
and layouts are all independent of the drain and proceed in parallel. §2 has
the protocol and the outcome tree.

### D-M3-2 — The double-published pane events are fixed here, before resync ships

R-M2-3, verified still true: the pane actor publishes seven event kinds
directly on the bus (`pane_host/actor.rs:132,266,278,286,293,302,313` —
damage, title, resized, history committed/invalidated/evicted, exited) *and*
reports them to Core, which republishes six of them
(`core/report.rs:41,49,54,64,67,81`). Every such transition burns two
sequence numbers today.

M2 could decline to fix this because its consumers tolerate at-least-once. M3
cannot: the reconnect-resync makes sequence numbers a continuity artifact that
crosses process generations — `Resume.last_seq` in the frozen `Hello`
(`hello.rs:73-79`), the manifest's inherited bus head, `subscribe_after` on
the replay ring. Duplicates halve the effective replay window
(`DEFAULT_REPLAY_CAPACITY` is 1024 events, `bus.rs:12`) and they halve it
precisely when it matters most — the reconnect storm after a swap, when every
client is resuming from a seq at once and a `gap` costs each of them a full
state resync.

**Decision (W02):** the rule becomes **one publisher per event kind**, and the
publisher of a pane-thread fact is the pane actor that owns the thread. The
six republishes in `core/report.rs` are deleted; the folds they sit next to
(history window, draining list, effects) stay untouched, because reports are
mailbox messages, not bus events, and Core still needs them. Core remains the
publisher of everything it owns: created/renamed/closed, workspace, focus,
layout, restore, config. The alternative — Core as sole publisher — was
rejected on a load-bearing detail: damage reports reach Core by `try_send` and
are deliberately droppable under saturation (`pane_host/actor.rs:136-147`),
while the pane actor's own publish never drops; funneling `PaneDamage` through
Core would let a saturated Core starve `pane.wait_output`. This contradicts
one sentence — `core/report.rs:5-6` says "`Core` is the only publisher of bus
events (04 §2)" — and 04 §2 needs a one-line doc PR from "one publisher" to
"one publisher per event kind" (flagged, R-M3-9). The at-least-once contract
means removing duplicates is not a version bump; the event goldens don't
change shape, and a new test pins one-sequence-number-per-transition.

### D-M3-3 — The quiesce/release machinery M0 built is real, and one piece is missing

Verified against `pty/`: the states handoff needs exist and say what they
give. `State::Quiesced` is documented as "the state M3's descriptor handoff
lands in: the terminal is still owned and still open, but nothing is being
read from it or written to it, so its state cannot move under the process
taking it over" (`handle.rs:57-69`). `quiesce()` drains queued writes first —
reading while draining so a child blocked on output cannot deadlock the drain
(`runner.rs:175-218`) — and input queued after a quiesce stays queued rather
than being dropped (`runner.rs:220-231`, "a quiesce that discarded a keystroke
would be a data-loss bug wearing a state machine's clothes"). `resume()`
undoes it; `release()` ends the actor for good.

What is missing is any way to get the master fd *out*: `PtyActorHandle` has no
dup operation, and the actor owns its `UnixPtySession` until the thread drops
it. W03 adds `Control::DupFd(reply)` — answered on the actor thread via
`rustix::io::fcntl_dupfd_cloexec` on the session's `AsFd`, valid only in
`Quiesced` — so the descriptor leaves under the same serialization as
everything else that touches the terminal (K4's race-free-by-construction
property, kept).

Two facts about fd lifetime that the whole design leans on, verified from
unix(7): an SCM_RIGHTS transfer is "a reference to an open file description …
semantically equivalent to dup(2) into the file descriptor table of another
process". So (a) the importer's fd and the exporter's fd are the *same open
file description* — same file position, same flags — and (b) the pty is torn
down only when the last reference closes. The exporter's normal shutdown after
commit closes its copies (`pane_host/actor.rs:342-358` ends the actor thread,
dropping the session), and the child sees no hangup because the importer's
reference keeps the description open. No `preserve_for_handoff` contortion is
needed — herdr needs one because its runtime kills children on drop; amx's
does not.

### D-M3-4 — VT state crosses as a synthesized replay, not a serialized object

libghostty-vt has no serialization API — `amx-vt`'s surface is write/resize/
mode/query/snapshot (`terminal.rs`), and 06 R4/R5 already established the
grid is one mutable FFI object that cannot be copied out. herdr answers this
with `initial_history_ansi`, a bounded ANSI replay string rebuilt into a fresh
VT (`handoff_runtime.rs:29`, cap 8 KiB/pane, `handoff.rs:28`). amx does the
same mechanism with better inputs, because M0/M1 built them:

- **The styled visible grid** is synthesized from the published POD snapshot —
  cells carry resolved fg/bg and SGR attributes (`snapshot.rs:82-93`), rows
  carry wrap flags — as cursor-addressed SGR runs plus a final cursor
  position. This is what makes "no visible screen content lost" literal:
  colors, attributes and layout survive, not just text.
- **Recent scrollback** rides as packed unstyled rows (the M1 sidecar format,
  `history/pack.rs`), read on the parser thread through the same
  `read_row` path history ranges use. Unstyled history is the accepted
  fidelity bound R-M1-1 already recorded for sidecars; the manifest inherits
  it. Budget: the most recent 500 rows per pane, 256 KiB/pane serialized cap,
  truncation recorded per-pane in the manifest and surfaced through the
  restore report rather than silently.
- **Modes and flags** are read from the terminal on the parser thread: alt
  screen, bracketed paste, mouse tracking, kitty keyboard flags
  (`terminal.rs:280,380,395`), title, cursor. The importer replays scrollback
  rows first (the `replay_bytes` pattern restore already uses,
  `core/restore.rs:380-396,440-446`), then the styled grid, then applies modes.

The import side must also resume three counters or continuity is a lie:
`HistoryTracker` starts `head`/`floor` at zero (`tracker.rs:114`) and needs a
`resume(head, floor)` constructor so row ids continue (04 §6: "scrollback row
ids are continuous across handoff"); `Snapshots` generation starts at zero and
needs seeding so `Resume.generations` comparisons mean anything; and the bus
head starts at zero (`bus.rs:66-78`) and needs `Bus::new_at(seq)`. All three
are W03 contract work. Rows older than what the manifest carried are announced
through the existing eviction floor — clients that cached them keep them (row
ids are never reused), they just cannot refetch, which is the M0 invalidation
contract doing exactly its job.

### D-M3-5 — What the manifest carries, checked against what is reachable

04 §6 names VT state + recent scrollback + event-bus seq + per-pane grid
generations, "never depends on the opt-in history sidecars". Verified
reachable, and the full inventory:

| Field | Source | Why |
|---|---|---|
| manifest schema version + read window | constant | N/N−1 for the handoff surface itself |
| exporter version, proto window | build info, `version.rs` | audit trail; pre-flight already checked overlap |
| `SessionId` | `Core` (minted `new_v4()` at `core/mod.rs:188`; gains `with_session_id`) | a reconnecting client distinguishes "same session continued" (resync valid) from "different server" (drop caches) via `Welcome.session` (`hello.rs:127-129`) |
| bus head `Seq` | `ctx.bus.head()` | the successor's bus continues, `Welcome.seq` stays monotonic |
| the persist `Snapshot`, captured in memory | Core (same capture the final push uses) | layout, cwds, labels, agent identity, session refs, short numbers — everything a cold restore would read from disk (`persist/mod.rs:100-112`), without a disk round trip |
| per pane: UUID, child pid, size, title, generation, kitty flags, modes, cursor, styled-grid replay, packed recent rows, tracker `head`/`floor`, truncation flag | parser thread export command | D-M3-4 |
| per pane: fd index | transfer order | pairs descriptors with entries |
| hub: per-pane agent kind, session ref, status + cause, attention queue in block order | `AgentHub` export | statuses could be re-derived by tier 2 within ~100 ms, but carrying them avoids a visible flap and preserves queue *order*, which is not re-derivable |

Not carried, deliberately: client connections and subscriptions (they die with
the sockets, 04 §6), in-flight waits (the CLI retries, D-M3-7), damage
accumulators and per-client stream state (reconstructed by each client's
re-bind), sidecar files (already on disk, owned by the successor's Persist
once it starts).

### D-M3-6 — The staged commit, adapted where amx differs from herdr

herdr's protocol (`server/handoff.rs`, driven from `headless.rs:1140-1345` and
`4790-4860`) is: token-authenticated socket, one-line JSON manifest,
`validated` → fds → `restored` → old removes public sockets → `ready` →
`committed` → `owned`, 30 s stage timeouts, 500 ms advisory owned-ack, strict
abort everywhere. amx keeps the shape and the timeouts and changes five
things, each for a reason:

1. **Token over stdin, not argv.** herdr passes the token as a command-line
   argument (`spawn_handoff_import`, `handoff.rs:74-79`); `/proc/*/cmdline` is
   world-readable on Linux, so that leaks the secret to every local user. The
   socket's 0600 mode is the real wall, but a secret that leaks is worse than
   no secret; amx writes it to the importer's stdin pipe.
2. **A pre-flight capability probe before anything quiesces.** herdr validates
   the successor *after* pausing every pane, paying a full quiesce/rollback for
   a wrong binary. amx runs `<binary> _handoff-caps` first — one exec, JSON
   out: `{version, handoff: [min,max], proto: [min,max]}` — and refuses before
   touching a pane if the windows don't overlap. Where herdr's importer then
   demands exact expected-version equality (`handoff.rs:252-262`), amx checks
   the manifest **window**: self-update hands to any successor that reads
   manifest v1, which is what lets the handoff surface itself skew N/N−1.
3. **Per-pane fd messages, not one batch.** herdr sends all fds in a single
   `sendmsg` capped at 64 panes (`MAX_FDS_PER_HANDOFF`) under the kernel's
   SCM_MAX_FD=253 ceiling (unix(7)); a 65-pane session cannot upgrade. amx
   sends one message per pane whose 1-byte payload is the pane's manifest
   index — no session-size cliff, deterministic pairing, and a receive
   failure names the exact pane. (unix(7): a too-small ancillary buffer means
   the kernel *closes* the excess descriptors in the receiver — the
   per-message shape also makes that misconfiguration structurally
   impossible, since every message carries exactly one fd.)
4. **The session socket moves late and by unlink-then-bind.** `Gateway::bind`
   refuses a path whose file still exists and refuses harder if a live server
   answers it (`gateway.rs:154-204`) — correctly, and the handoff must not
   weaken that. Sequence: the exporter keeps its listener and socket file
   through `restored` (so an `amx` probing mid-handoff still finds a live
   server rather than auto-daemonizing a rival); on `restored` it retires the
   gateway — stop accepting, close connections, unlink the file — and the
   importer, on its side of `restored`, probe-loops the path until it is free
   (5 s cap, herdr's constant) and binds, then reports `ready`. The window in
   which a racing `amx` could daemonize a third server is the unlink-to-bind
   gap; the loser of any such race gets `AlreadyRunning` from the existing
   probe logic, and R-M3-5 records the residual.
5. **The exporter's Persist is fenced at commit.** After `committed`, the
   snapshot on disk belongs to the successor; the exporter's shutdown path
   normally pushes a final capture (`serve.rs:144-150`, 07 §2), and across a
   handoff that late write would clobber the successor's view of the session.
   The export orchestrator disarms the final push before reporting
   `committed`. herdr has no equivalent hazard only because its importer
   rewrites the snapshot immediately; amx closes the hole instead of racing
   it.

In-flight PTY reads across the transfer, stated precisely: after `quiesce()`
returns, the exporter no longer reads (`runner.rs:90` polls for readability
only in `Running`), so child output accumulates in the kernel's pty buffer;
a child that fills it blocks in `write(2)` — flow-controlled, not lost. The
importer resumes reading on the same open file description after `owned`, so
the bytes come out exactly where the exporter stopped. Input the exporter had
queued but not written was flushed *by* the quiesce (its whole contract);
input arriving during the swap fails fast at the handle (`NotAccepting`) and
is the caller's retry, which for every CLI path is D-M3-7's reconnect.

The full protocol with per-step ownership and crash consequences is
[§3](#3-the-handoff-protocol).

### D-M3-7 — Reconnect-resync: the server half exists as types, nobody sends or reads them

M2's V11 built the server→client event path and 08 R-M2-4 named it the
pattern-setter for this. The audit found the wire is *already shaped* for
resync and completely unwired:

- `Hello.resume: Option<Resume{last_seq, generations}>` is defined, golden-
  frozen, and documented as "reattach is a resume, not a fresh attach"
  (`hello.rs:66-108`) — and every `Session::attach` call site in the tree
  passes `None` (`wired.rs:74`, `cmd/{call,events,hook,viewport}.rs`; the
  attach verb goes through `App::attach`, which is `wired.rs`'s `None`), and
  no server code reads the field.
- `Welcome` already carries the capture seq and the server's `SessionId`
  (`hello.rs:121-129`) — the two facts a resuming client needs.
- The bus already has `subscribe_after(seq)` with the gap-on-underrun contract
  (`bus.rs:143-150`).
- The keyframe machinery already names the reason: `KeyframeReason::
  Generation` is documented as "what a reconnect with a stale `Resume`
  produces" (`keyframe.rs:13-15,37-38`).
- The client already has the two-reading resync split from the M2 live-smoke
  fix: `sync_state` adopts focus, `resync_state` keeps this terminal's
  presentation (`docs/notes/m2-live-smoke.md` §8.1).

So M3's work is wiring, not wire: the connection stores the hello's resume and
starts the event subscription at `subscribe_after(last_seq)` instead of
`subscribe()` (falling out of the replay ring produces the gap the client
already handles); `stream.bind` grows an additive optional `generation` param
(R-M1-8 precedent: additive fields ride v1) so a re-bound grid stream opens
with `Generation`-keyframe-or-nothing instead of unconditionally `First`; and
the client gains the reconnect loop nothing has today (no reconnect logic
exists anywhere in `amx-client` or the CLI): on transport EOF, retry connect
with backoff to a deadline, re-Hello with `Resume`, and branch on
`Welcome.session` — same id ⇒ resync (keep caches, events-since or gap), new
id ⇒ fresh attach (drop caches). `amx wait` and its cousins re-issue the call
after the reconnect; **the state-predicate contract (04 §2) is what makes the
retry exact** — a transition that fired during the swap is simply true at
re-subscribe time, so a wait can never miss it, and 04 §6's "waits retry
transparently inside the CLI" costs no server-side wait persistence at all.

### D-M3-8 — Self-update: herdr's mechanisms, amx's honesty about hosting

From `update.rs` (3,493 lines, mined not copied): channel manifests as JSON
(`latest.json`/`preview.json` with version, notes, per-platform asset URL +
`sha256`), fetch via a **curl subprocess** — "no additional Rust HTTP
dependencies" is herdr's own module doc (`update.rs:6,293-317`) — sha256
verified before install (`checksum::verify_sha256` over the `sha2` crate), and
package-manager detection by inspecting the running exe's path following
symlinks: Homebrew Cellar, mise's data dir, `/nix/store`
(`update.rs:1734-1743,1948`). Managed installs are *redirected* — print the
manager's own upgrade command, never write into its tree.

amx adopts all four mechanisms: `amx update check` / `amx update apply`; curl
subprocess with argv-as-data (no HTTP dependency); `sha2 = "0.10"` as the
milestone's one new dependency (herdr's choice too; hand-rolling crypto or
parsing `sha256sum` output across platforms are both worse — R-M3-8);
detection classes `standalone | brew | mise | nix` with redirect messages.
`apply` stages the download in the state dir, verifies, atomically renames
over the current exe (rename is legal on a running binary on unix; ETXTBSY
guards writes, not renames), then triggers `session.handoff` against the
running session — and because the caller's own connection dies at gateway
retirement, the CLI treats the disconnect as "in progress" and reconnect-polls:
a `Welcome` from the new version with the same `SessionId` is success; the old
version answering again means the abort path ran, and `session report` (whose
handoff-attempt row W03 adds) says why.

**The hosting story, honestly:** there is no amx.dev and there are no release
binaries yet. M3 ships the manifest *format*, the fetch/verify/stage/handoff
machinery, and a config-overridable channel URL whose default points at the
GitHub releases of this repository (`.../releases/latest/download/latest.json`
— a manifest asset uploaded per release). CI tests the whole path against a
`file://` URL and a scratch manifest. What M3 does **not** ship: the release
pipeline that publishes those assets, cross-platform prebuilt binaries, or a
preview channel. Until a release exists, `amx update check` against the
default URL reports "no manifest published" and says so plainly — a true
statement, not a stub pretending otherwise (R-M3-4).

### D-M3-9 — SSH remote: the byte bridge with zero client changes

K8's design, kept whole: the local `amx --remote host …` spawns
`ssh host exec amx _bridge --session <name>` and speaks the ordinary protocol
over the child's stdio; `amx _bridge` connects stdio to the session socket and
splices bytes both ways, daemonizing the remote server first when asked
(`--daemonize`, reusing `session/daemon.rs` — the auto-detect-or-daemonize
`amx` itself performs). The implementation trick that keeps the client
untouched: the local side creates a `socketpair`, hands one end to the ssh
child as stdin+stdout, and passes the other end to `Session::attach` — which
takes a `UnixStream` today (`net.rs:161`) and never learns the peer is a
subprocess. No transport trait, no client edits, the smart-client encoding is
already bandwidth-cheap by design (04 §4), and Hello/Welcome negotiates
versions across the wire exactly as locally — a remote inside the N/N−1
window works with **no** reinstall and no forced restart, which is the
roadmap's "skew window honored" clause and the thing herdr's strict equality
could never do (W3).

**Seeding**, scoped honestly: when `exec amx _bridge` fails with "command not
found", the local side probes `uname -s`/`uname -m` over ssh (herdr's probe,
`remote/attach.rs:704-718`). Platforms match ⇒ offer to stream the local
binary to `~/.local/bin/amx` (cat to a temp path, chmod, atomic mv — herdr's
three-script install shape) after an explicit confirmation; platforms differ
⇒ refuse with the reason and point at the channel manifest if one is
configured. Cross-platform seeding *requires* hosted binaries and therefore
inherits D-M3-8's hosting reality; M3 ships same-platform streaming and the
honest refusal, not a promise.

**What CI can actually test:** three tiers. (1) The bridge without ssh — spawn
`amx _bridge` directly as a child with a socketpair — exercises every byte of
amx code in the path on every platform, every run. (2) Loopback ssh: a CI-only
sshd on 127.0.0.1 with a generated key, Linux runners only — darwin runners
cannot reliably run sshd (the darwin-CI lore stands), so the job is
Linux-gated and the same test binary skips cleanly when `AMX_TEST_SSHD` is
unset. This tier is what "skew CI extended to the bridge path" means in
practice: the existing skew suite's calls run over the bridge transport —
current-vs-current until a second protocol version exists, same as M0's
harness, honestly labeled. (3) A real second machine is not a CI resource;
it is a live-smoke step (§7).

### D-M3-10 — Worktrees: git stays in the CLI, membership lives in the workspace

`amx work <branch>` composes three existing capabilities instead of teaching
the server git: the CLI runs `git worktree add` (argv-as-data, never a shell
string) rooted at the repo of the caller's cwd, placing the tree at the
configured template `work.dir` (default `{repo_parent}/{repo_name}--{branch}`
— a sibling, so repo-internal tooling never trips over a nested checkout),
creates a workspace named after the branch with the worktree as cwd, and
starts an agent in it when `--kind` is given. The one server-side addition is
a small optional `worktree {repo, branch, path}` block on workspace state and
snapshot (additive fields, no version bump), because `amx work done` and
restore both need the association: `done` resolves the workspace by branch,
kills it through the existing verb, then `git worktree remove` — refusing a
dirty tree unless `--force`, destructive-op caution as policy — and restore
validates the path still exists, degrading to a plain workspace with a
restore-report entry when it does not (herdr's restore does the same
validation; M1's report machinery is exactly where the loss belongs).

### D-M3-11 — Layouts: export what state already says, apply through public verbs

`amx layout export` renders `session.state` — which already carries the
workspace list, BSP trees, cwds, labels, and agent kinds — into a TOML file;
`amx apply layout.toml` replays it through the public surface
(`workspace.create`, `pane.split`, `agent.start`) in deterministic order. No
new server methods, no new wire types: the layout file is a *client-side*
artifact, which also means a layout can be applied to a remote session over
the bridge unchanged. Session refs are deliberately not exported (a layout is
a shape, not a conversation; refs are secrets-adjacent and machine-specific).
Apply adds workspaces to the running session, suffixing names on collision —
the conservative semantic; replacing a session is `session stop` + apply, two
explicit steps. The acceptance is a round trip: export → apply into a fresh
session → export again → equal modulo ids.

### D-M3-12 — Exit status of inherited children is `Unknown`, and that is correct

After a handoff the pane's child is not the successor's child; `waitpid` is a
parent's call, so `try_wait` yields ECHILD and the exit path reports
`ChildExit::Unknown` — a state the runner already models and tolerates
(`pty/mod.rs:64-73`, `runner.rs:330-349`). What still works, verified:
exit *detection* rides the pty EOF, not the wait (`read_once` returning
`Ok(0)`/EIO ends the actor, `runner.rs:259-267`), so `pane.exited` still
fires and `wait --until exited` still returns; `kill()` signals by pid, which
needs no parentage; `foreground_group` is an ioctl on the master. The importer
wraps the received fd in an `InheritedPtySession` whose `try_wait` probes
`kill(pid, 0)` for liveness and never invents an exit code. The one honest
degradation: a post-handoff `pane.exited` carries no numeric status. Recorded
in R-M3-3; nothing in the tree branches on the code today.

---

## 2. The shutdown-wedge spike (W01)

**What is known.** Reproduction: T17's CLI suite looped under 8-way CPU load,
~1/300 failure rate; sessions named `race`, `daemonize`, `detach` have all
hit it. Signature: server catches SIGTERM (the log line prints), never
finishes `Runtime::shutdown`; main thread in futex wait on the JoinSet drain.
Post-mortem of wedged processes: five `/dev/ptmx` masters open in tests that
attach once — the pane spawn *repeated* — plus an unreaped zombie shell.
Statistically not attributable to any recent change; mechanically consistent
with a Core↔PaneHost interaction during cancel, possibly re-seeding, possibly
an exit report that never completes the drain (a pane host's exited path moves
the host to Core's draining list, `core/report.rs:73-82`; a message loop that
re-spawns while cancelling would match the five masters).

**Protocol.** Reproduce first, then instrument, then explain:

1. A repetition harness (`tests/seams/shutdown.rs` extended, or a script under
   `scripts/spike/`) that loops the implicated suites under `nproc`-wide load
   and preserves a wedged process instead of killing it.
2. On capture: full thread backtraces (`gdb -p`/`eu-stack`), `/proc/<pid>/fd`
   inventory, the JoinSet's remaining task census — W01 adds temporary task
   *naming* to `Runtime::spawn` so a straggler is identifiable, a diagnostic
   worth keeping regardless.
3. Explain the five masters. The attach path seeds one pane; five means a
   spawn loop. Find the loop's driver (workspace seeding on attach? restore?
   a retry in a test helper?) and whether it is the cause or a symptom.
4. Fix with a test that fails without the change — a bounded-repetition
   shutdown storm in CI (the M1 tripwire pattern), plus whatever unit pins the
   actual mechanism.

**Outcome tree.** (a) *Diagnosed and fixed:* W06 unblocks; the storm test
joins the suite. (b) *Diagnosed, fix is large:* the finding names the safe
subset (e.g., "wedge requires a pane spawned during cancellation; the export
path quiesces before cancelling and cannot enter it") and W06 proceeds under
that written argument plus a watchdog: the export orchestrator bounds the
post-commit drain with a deadline, logs the census, and exits nonzero rather
than wedging silently — visible, not swallowed (the `Runtime` doc's own
standard, `runtime.rs:74-76`). (c) *Not reproduced within the budget* (two
days of machine time): the same watchdog ships, the finding documents the
attempt honestly, and the milestone's exit criterion gains a by-hand kill
check on the old server after the live-smoke upgrade. In no outcome does W06
merge with the drain both undiagnosed *and* unbounded.

---

## 3. The handoff protocol

Roles: **exporter** (the running server) and **importer** (`amx server
--handoff-import <socket>`, token on stdin, spawned detached by the exporter).
Transport: a private Unix socket `<runtime_dir>/handoff-<pid>.sock`, mode
0600, removed by the exporter once the importer has connected and
authenticated. All stage timeouts 30 s except the final advisory ack (500 ms)
and the socket-free probe loop (5 s). Every message is a single JSON line
except the fd messages; caps mirror the control-frame discipline.

The stages, with what each side owns at every step:

```
 exporter                                    importer
 ────────                                    ────────
 0  pre-flight: run `<binary> _handoff-caps`;
    refuse on window mismatch. Owns: everything.
 1  bind handoff socket; mint token;
    spawn importer (token → its stdin)
                                             2  connect; send token line
 3  verify token
 4  QUIESCE: panes quiesce (state frozen,
    input refused at the handle); per-pane
    export captured on parser threads;
    Core/hub/bus captured. Session socket
    still answers. Owns: everything.
 5  send manifest line
                                             6  validate manifest window,
                                                session dir identity, fd count
                                             7  → "validated" (or "abort:<why>")
 8  one sendmsg per pane: 1-byte index
    + SCM_RIGHTS fd (a dup of the master,
    taken on the actor thread)
                                             9  wrap fds in quiesced inherited
                                                pane hosts; seed VT replay,
                                                tracker head/floor, generation;
                                                Bus::new_at(seq); Core with
                                                inherited SessionId; hub seeded.
                                                Nothing reads or writes a pty.
                                            10  → "restored"
11  RETIRE: gateway stops accepting,
    closes connections, unlinks socket
    file. Clients begin reconnect loops.
    Owns: panes (quiesced), state.
                                            12  probe-loop session socket path
                                                free (≤5 s); Gateway::bind;
                                                → "ready". Owns: the socket.
                                                New connections see frozen
                                                grids and full state.
13  disarm final Persist push;
    → "committed". Ownership transfers.
                                            14  resume every pane; publish
                                                nothing (no state changed);
                                                → "owned" (advisory)
15  read "owned" (500 ms, best effort);
    release pane actors (dup'd fds close;
    children unaffected — importer holds
    the descriptions); cancel; drain
    (W01's territory); exit 0.
                                            16  serve. Clients reconnect with
                                                Resume; waits re-issue; agents
                                                never noticed.
```

**Crash at each step, and what it leaves:**

| Fails at | Exporter does | Importer does | The user has |
|---|---|---|---|
| 0–3 | report error to caller; nothing was touched | exits (bad/missing token, no socket) | running session, error message |
| 4 (a pane refuses quiesce) | resume the panes that did quiesce; abort | told "abort", exits | running session; report names the pane |
| 5–7 (validation refuses) | resume panes; abort; kill+reap importer | "abort:<why>", exits | running session; reason in `session report` |
| 8–10 (importer dies mid-restore) | 30 s timeout → resume panes; abort | its fds close with the process (kernel), it never read a pty | running session; nothing observed the swap |
| 11–12 (importer fails to bind; exporter crashed) | timeout → re-bind own socket (herdr's restore-sockets move), resume, abort | strict abort: unlink anything it bound, exit | running session — or, if the exporter is gone, panes orphaned-but-alive under init; snapshot on disk restores the layout and respawns what died |
| 13 lost (exporter dies before "committed" lands) | — | `wait_committed` times out → **strict abort**: never serve without commit (a wedged-not-dead exporter must not meet a live importer — split brain) | the wedge case; W01's watchdog bounds it; recovery is kill + `amx` (restore) |
| after 13 | exporter is done regardless; drain watchdog bounds a wedge | owns everything | working upgraded session; a leaked old process at worst, flagged by exit code |

The abort rule is herdr's, kept strict: **no partial import ever serves.** The
importer's only alternatives are "owned everything" and "exited having touched
nothing but dup'd fds that die with it."

---

## 4. Wire and event surface

Small on purpose; the resync rides shapes M2 froze.

- **One new method row**: `session.handoff` (params: staged binary path,
  optional timeout; reply: accepted/refused with reason — completion is
  observed by reconnecting, D-M3-8). CLI: `amx session handoff`. Golden +
  skew arm + seam stub in W03, handler in W06.
- **Additive fields, no version bump** (the R-M1-8 precedent, both-directions
  unknown-field tolerance): `generation` on `stream.bind` params; a
  `worktree` block on workspace state in `session.state` and the persist
  snapshot; a handoff-attempt row on `session.report`.
- **No new event kinds.** The swap is invisible on the bus by design — the
  successor continues the same sequence space; a client learns "server
  restarted" from `Welcome.session`, not from an event. (`SessionRestored`
  remains the cold-restore signal.)
- **No `Hello`/`Welcome` changes**: `Resume` is already on the wire and
  golden-frozen; M3 starts sending and reading it.
- **Hidden CLI surface**: `amx _bridge`, `amx _handoff-caps`, `amx server
  --handoff-import`. Public CLI: `amx update check|apply`, `amx work <branch>
  [--kind]`, `amx work done [branch]`, `amx layout export`, `amx apply
  <file>`, `amx --remote <host>` (global flag on attach/verbs), `amx session
  handoff`.
- Goldens law (R-M2-7 pattern): 1 method golden, 1 skew arm, regenerated
  `session.state`/persist/report goldens for the additive fields, a bridge
  transport case in `tests/skew.rs`. All land in W03 so no wave task discovers
  the law mid-flight.

---

## 5. Task DAG

Difficulty is `hard` when the task carries syscall, concurrency,
wire-compatibility, or restore-correctness risk; `normal` otherwise. Every
task lands with tests that fail without the change, and finishes with
`cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
green. File scopes are exclusive within a wave; sequential fills of an earlier
task's file are declared, never discovered.

---

### W01 — Shutdown-wedge diagnosis spike

- **Difficulty:** hard · **Wave:** 0 · **Depends on:** —
- **Goal:** the drain hang explained, and either fixed or bounded with a
  written safety argument (§2's outcome tree).
- **Scope (owns exclusively):** `docs/notes/m3-shutdown-wedge.md`,
  `scripts/spike/**`, `tests/seams/shutdown.rs`, `runtime.rs` (task naming +
  drain census diagnostics), plus the fix's own files if (a) — named in the
  finding before they are touched.
- **Acceptance:** the finding documents reproduction rate, thread backtraces,
  fd census, and the mechanism (or the bounded-budget failure to reproduce,
  honestly); under (a), a test that fails without the fix plus a
  bounded-repetition shutdown storm in CI; under (b)/(c), the export-drain
  watchdog design is specified for W06 and the safe-subset argument is
  written down.
- **Prompt draft:** Diagnose the amx shutdown wedge exactly as
  `docs/09-m3-plan.md` §2 lays it out. Build the repetition harness first and
  run the implicated CLI suites under full CPU load until a wedged server is
  captured alive; take thread backtraces, the fd table, and the JoinSet
  census (add task naming to `Runtime::spawn` — keep it). Explain the five
  ptmx masters before proposing anything: find what spawns panes repeatedly
  on a path that attaches once. Fix it with a test that fails without the
  change if the fix is small; otherwise write the safe-subset argument and
  the watchdog spec for the export path. Never guess — a hypothesis you did
  not observe in a backtrace is labeled a hypothesis in the note.

---

### W02 — One publisher per event kind

- **Difficulty:** normal · **Wave:** 1 · **Depends on:** —
- **Goal:** every pane transition gets exactly one sequence number (D-M3-2).
- **Scope:** `actor/core/report.rs` (delete the six republishes, keep the
  folds), `actor/mod.rs:88-90` comment, `crates/amx-server/tests/`
  (a publication-count test; touched suites re-verified), `docs/notes/` line
  in the wave outcomes for the 04 §2 doc PR (R-M3-9).
- **Acceptance:**
  - `a_pane_transition_publishes_exactly_one_event` (drive damage, title,
    history commit, exit through a real pane; assert one envelope per
    transition on a bus subscription)
  - `waits_and_wait_output_still_complete` (the existing suites stay green —
    they were written against at-least-once and must not have depended on
    the duplicate)
  - event goldens unchanged (shape is identical; only duplication stops)
- **Prompt draft:** Close R-M2-3 as `docs/09-m3-plan.md` D-M3-2 decides:
  the pane actor keeps publishing the seven pane-thread facts it already
  publishes; delete Core's six republishes in `core/report.rs` while keeping
  every fold beside them (history window, draining list, effects). Update the
  stale "only publisher" comments to the per-kind rule. Add the
  one-envelope-per-transition test and re-run the waits/drive/flow-control
  suites; if any of them turns out to depend on the duplicate, stop and flag
  it rather than compensating silently.

---

### W03 — M3 contracts: rows, fields, continuity plumbing, splits, stubs

- **Difficulty:** hard · **Wave:** 1 · **Depends on:** —
- **Goal:** every shared surface later waves implement against, frozen; every
  budget-edge file split before the waves press it.
- **Scope:** `amx-proto/src/control/{mod.rs,session.rs,stream.rs}`
  (`session.handoff` row; `generation` on bind params; report row),
  `amx-proto/tests/`, `tests/goldens/**`, `tests/skew.rs`;
  `amx-core/src/event/bus.rs` (`Bus::new_at`), `amx-core/src/state/**`
  (workspace `worktree` block); `amx-server/src/actor/core/mod.rs`
  (`with_session_id`), `history/tracker.rs` (`resume(head, floor)`),
  `amx-vt/src/snapshot.rs` (generation seed), `pty/{handle,runner}.rs`
  (`Control::DupFd`, quiesced-only), `persist/mod.rs` (additive fields),
  `amx-server/src/handoff/mod.rs` (module skeleton so W04/W05 never share a
  file), `dispatch/session.rs` seam stub, `amx-server/Cargo.toml` (rustix
  `net` feature), `crates/amx/src/cli.rs` + `cmd/mod.rs` (routing arms for
  `update`, `work`, `layout`, `apply`, `_bridge`, `_handoff-caps`, `session
  handoff` — planted here so no two wave tasks touch `cli.rs`, the U01/V02
  precedent), budget splits: `actor/persist/actor.rs` (505, over soft) and
  `actor/mod.rs` (502) split by responsibility before anything grows them.
- **Acceptance:**
  - `method_golden_and_skew_arm_cover_session_handoff`
  - `stream_bind_with_generation_reads_at_v1_and_without_it_still_parses`
  - `a_bus_born_at_seq_n_continues_gapless_from_n`
  - `a_tracker_resumed_at_head_floor_commits_the_next_row_id_contiguously`
  - `dup_fd_answers_only_in_quiesced_and_yields_a_working_duplicate`
  - `workspace_worktree_block_round_trips_snapshot_and_state`
  - module-size check green with the two splits landed
- **Prompt draft:** Land M3's shared contracts exactly as
  `docs/09-m3-plan.md` §4 and D-M3-3/4/5 define them, the way V02 did for
  M2: compiling types, seam-stubbed handlers, no behavior. The continuity
  plumbing is the substance — `Bus::new_at`, tracker resume, generation
  seeding, `Core::with_session_id`, and the quiesced-only `DupFd` control on
  the pty actor (answered on the actor thread, `fcntl_dupfd_cloexec`,
  refused in any other state, with tests over a real pty). Regenerate every
  golden the law demands and split the two over-budget files first. The
  handoff module skeleton declares `manifest`, `grid`, `fd`, `protocol`
  submodules as empty shells so wave-2 tasks own disjoint files.

---

### W04 — Handoff manifest: capture, grid synthesis, import seeding

- **Difficulty:** hard · **Wave:** 2 · **Depends on:** W03
- **Goal:** a pane's frozen state becomes bytes and becomes the same pane
  again (D-M3-4, D-M3-5).
- **Scope:** `amx-server/src/handoff/{manifest.rs,grid.rs}`,
  `actor/pane_host/{mod.rs,actor.rs,parser.rs}` (`PaneCommand::ExportHandoff`
  — capture on the parser thread: styled-grid ANSI synthesis from the
  published snapshot, modes/flags/title/cursor, packed recent rows through
  the history path, tracker head/floor, generation; and the import-side
  seeding order: rows, grid, modes), `crates/amx-server/tests/handoff_manifest.rs`.
- **Acceptance:**
  - `a_styled_grid_synthesizes_replays_and_matches_cell_for_cell` (property
    test over random styled grids: synthesize → replay into a fresh
    `Terminal` → published snapshots equal, wide chars and wrapped rows
    included)
  - `modes_survive_the_round_trip` (alt screen, bracketed paste, mouse,
    kitty flags, title, cursor)
  - `recent_rows_ride_packed_and_land_with_contiguous_row_ids`
  - `the_per_pane_byte_cap_truncates_oldest_first_and_says_so`
  - `export_refuses_a_running_pane` (quiesced-only, same rule as `DupFd`)
- **Prompt draft:** Build the handoff manifest codec per
  `docs/09-m3-plan.md` D-M3-4/D-M3-5. The grid synthesizer reads the POD
  snapshot (cells carry resolved rgb + SGR style) and emits
  cursor-addressed SGR runs; get wide-character spacers and wrapped rows
  right and prove it with property tests, not examples. Scrollback rides the
  M1 packed-row format, budgeted (500 rows / 256 KiB per pane), truncation
  recorded. Capture runs as a parser-thread command against a quiesced pane
  only. The import half seeds a fresh terminal in the order rows → grid →
  modes and resumes the tracker and generation counters from the manifest.
  Fidelity bounds you cannot close (styled history — R-M1-1's precedent) are
  stated in the module docs, not papered over.

---

### W05 — SCM_RIGHTS transport and the staged-commit state machine

- **Difficulty:** hard · **Wave:** 2 · **Depends on:** W03
- **Goal:** §3's protocol as two typed state machines that cannot be driven
  out of order, over a real socket, with the fd transfer proven.
- **Scope:** `amx-server/src/handoff/{fd.rs,protocol.rs}`,
  `crates/amx-server/tests/handoff_protocol.rs`.
- **Acceptance:**
  - `an_fd_crosses_a_socketpair_and_reads_the_same_description` (write
    through one, read through the other; offset shared — the unix(7)
    semantics the design leans on, pinned)
  - `each_message_carries_its_pane_index_and_a_mismatch_aborts`
  - `every_stage_times_out_to_abort_not_hang` (each of §3's stages, with a
    fake peer that stalls)
  - `a_crash_at_each_stage_leaves_what_the_table_says` (fake peer killed at
    each step; assert the survivor's terminal state matches §3's table)
  - `the_token_travels_on_stdin_and_a_wrong_token_is_refused`
- **Prompt draft:** Implement `docs/09-m3-plan.md` §3's line protocol and fd
  transfer with rustix's `sendmsg`/`recvmsg` +
  `SendAncillaryBuffer`/`ScmRights` (the `net` feature is already enabled by
  W03). One fd per message, 1-byte pane-index payload. Exporter and importer
  are typed state machines whose transitions are the only public surface —
  a caller cannot send fds before validation because no method exists in
  that state. Test every stage against a scripted fake peer, including
  kill-at-every-step crash injection asserting the §3 table's outcomes.
  herdr's `server/handoff.rs` is reference for shape and timeouts; the code
  is written fresh, and the token rides stdin, never argv.

---

### W06 — Export path: orchestrator, quiesce, retire, abort

- **Difficulty:** hard · **Wave:** 3 · **Depends on:** W01 (gate), W04, W05
- **Goal:** a running server hands itself over — or aborts back to a running
  server.
- **Scope:** `amx-server/src/handoff/export.rs` (the orchestrator task),
  `actor/gateway.rs` (retire: stop accepting, close connections, unlink),
  `actor/core/handoff.rs` (new: capture + pane-handle collection +
  post-commit fencing of the final Persist push), `dispatch/session.rs`
  handoff arm, **`session/serve.rs`** (wiring the orchestrator's spawn
  capability — named here per the M2 W-1 scope rule, not left to
  integration), `crates/amx-server/tests/handoff_export.rs`.
- **Acceptance:**
  - `handoff_to_a_fake_importer_walks_every_stage_in_order`
  - `pre_flight_refuses_before_any_pane_is_touched`
  - `an_abort_at_any_pre_commit_stage_resumes_every_pane_and_reaccepts`
    (clients can connect again; panes echo again; `session report` names the
    reason)
  - `the_socket_answers_until_restored_and_is_gone_after_retire`
  - `no_final_snapshot_is_written_after_commit`
  - `the_drain_after_commit_is_bounded` (the W01 watchdog, whichever outcome
    W01 reached)
- **Prompt draft:** Build the export orchestrator per `docs/09-m3-plan.md`
  §3 and D-M3-6: pre-flight `_handoff-caps` first, quiesce and capture
  second, retire late (after "restored"), fence Persist at commit, resume
  everything on any pre-commit failure. The orchestrator is a runtime task
  spawned from the serve assembly — you own the `serve.rs` wiring; do not
  leave the join to W14. Gateway retirement is a new gateway mode, not a
  second gateway. Test against W05's fake importer, including the abort
  matrix; the storm/watchdog behavior follows the W01 finding, which read
  before starting.

---

### W07 — Import path: the successor assembles from the manifest

- **Difficulty:** hard · **Wave:** 3 · **Depends on:** W04, W05
- **Goal:** `amx server --handoff-import` becomes a serving successor whose
  session is indistinguishable from the exporter's, minus nothing visible.
- **Scope:** `amx-server/src/session/import.rs` (new: the alternate assembly
  — receive, build actors quiesced, probe-bind, staged replies; mirrors
  `serve.rs`'s five-actor order with the inherited Bus/SessionId/state),
  `amx-server/src/platform/pty.rs` (`InheritedPtySession`),
  `actor/core/import.rs` (new: state from manifest instead of disk),
  `crates/amx/src/cmd/server.rs` (`--handoff-import` arm),
  `crates/amx-server/tests/handoff_import.rs`.
- **Acceptance:**
  - `a_manifest_and_fds_become_a_serving_session_with_the_same_session_id`
  - `panes_stay_quiescent_until_committed_and_resume_after`
  - `bus_continues_from_the_inherited_seq_and_welcome_reports_it`
  - `an_inherited_child_exit_is_detected_by_eof_and_reports_unknown_status`
  - `strict_abort_on_missing_commit_serves_nothing` (importer with a peer
    that dies pre-commit unlinks its socket and exits nonzero)
  - `restored_agents_report_their_manifest_status_without_a_flap`
- **Prompt draft:** Build the importer per `docs/09-m3-plan.md` §3 and
  D-M3-5/D-M3-12: the assembly follows `session/serve.rs`'s order and
  comments (hub before anything can produce pane events, Persist subscribed
  after state exists, gateway bound only at the "ready" stage after the
  probe-loop). Panes are inherited quiesced and untouched until commit; the
  `InheritedPtySession` never fakes an exit code. Strict abort is the only
  failure mode. Test with W05's fake exporter and real fds from real ptys.

---

### W08 — Server-side reconnect-resync

- **Difficulty:** hard · **Wave:** 3 · **Depends on:** W03
- **Goal:** a `Hello` with `Resume` gets events-since-or-gap and
  generation-aware streams (D-M3-7).
- **Scope:** `amx-server/src/conn/{mod.rs,events.rs}` (store the hello's
  resume; event subscription opens at `subscribe_after(last_seq)`),
  `dispatch/stream.rs` + `damage/stream.rs` (bind honors the `generation`
  param: equal generation opens delta-only, stale opens with a
  `Generation` keyframe), `crates/amx-server/tests/resync.rs`.
- **Acceptance:**
  - `a_resume_within_the_ring_replays_exactly_the_missed_events`
  - `a_resume_beyond_the_ring_opens_with_a_gap_never_a_silent_skip`
  - `a_bind_with_the_current_generation_sends_no_keyframe`
  - `a_bind_with_a_stale_generation_opens_with_keyframe_reason_generation`
  - `a_verb_connection_without_resume_behaves_exactly_as_before` (goldens
    unchanged)
- **Prompt draft:** Wire the resync per `docs/09-m3-plan.md` D-M3-7. The
  types all exist — `Resume` in the hello, `subscribe_after` on the bus,
  `KeyframeReason::Generation` in the damage layer; your work is the
  connection plumbing that finally reads them, and the additive `generation`
  on bind. Drive every acceptance over the real socket with a real server.
  Do not touch the client; W09 consumes this.

---

### W09 — Client reconnect and transparent CLI retry

- **Difficulty:** normal · **Wave:** 4 · **Depends on:** W08
- **Goal:** an attached client rides a server swap with its screen intact; a
  standing `amx wait` never notices one happened.
- **Scope:** `amx-client/src/app/{wired.rs,reconnect.rs (new — the loop
  splits out, wired.rs is at 471)}`, `amx-client/src/net.rs` (resume
  threading), `crates/amx/src/cmd/{attach.rs,call.rs}` (reconnect-and-reissue
  for `wait`, `agent.prompt --wait`, `pane.wait_output`, `events`),
  `amx-client/tests/reconnect.rs`, `crates/amx/tests/wait_retry.rs`.
- **Acceptance:**
  - `a_dropped_client_reattaches_with_resume_and_repaints_only_stale_panes`
  - `a_new_session_id_drops_caches_and_full_resyncs`
  - `a_standing_wait_survives_a_server_restart_and_returns_on_the_predicate`
    (kill server, restart, fire the transition — the wait returns; the
    transition firing *during* the gap also returns, pinned separately)
  - `events_json_resumes_from_its_cursor_or_reports_the_gap`
  - `the_retry_gives_up_at_the_deadline_with_an_honest_error`
- **Prompt draft:** Build the client half of `docs/09-m3-plan.md` D-M3-7:
  a reconnect loop with backoff and deadline, re-Hello carrying
  `Resume{last_seq, generations}`, the `Welcome.session` branch, and the
  M2-vintage `resync_state`/`sync_state` distinction respected (a reconnect
  keeps this terminal's presentation). CLI waits re-issue after reconnect
  and lean on the state-predicate contract for exactness — write the test
  that fires the transition while no connection exists and prove the wait
  still returns.

---

### W10 — Self-update

- **Difficulty:** normal · **Wave:** 2 · **Depends on:** W03 (cli routing)
- **Goal:** `amx update check|apply` per D-M3-8, honest about hosting.
- **Scope:** `crates/amx/src/cmd/update.rs`, `crates/amx/src/update/{manifest.rs,
  fetch.rs,verify.rs,install.rs,pm.rs}`, config channel-URL field,
  workspace `Cargo.toml` (+`sha2`, justification in the commit body),
  `crates/amx/tests/update.rs`.
- **Acceptance:**
  - `check_against_a_file_url_manifest_reports_newer_older_equal`
  - `apply_verifies_sha256_and_refuses_a_mismatch_without_installing`
  - `apply_stages_then_renames_atomically_and_the_old_exe_keeps_running`
  - `a_brew_mise_nix_exe_path_redirects_and_writes_nothing` (fixture paths,
    herdr's Cellar/mise/nix-store shapes)
  - `no_published_manifest_is_reported_plainly_not_as_an_error_crash`
- **Prompt draft:** Build self-update per `docs/09-m3-plan.md` D-M3-8:
  curl-subprocess fetch (argv as data, herdr's flag discipline —
  `-sfL --retry 3` with timeouts), `sha2` verification, staging in the state
  dir, atomic rename, package-manager detection by exe-path inspection with
  redirect-not-write semantics. The handoff trigger calls `session.handoff`
  and then reconnect-polls; land that glue behind a flag until W06 merges,
  and test everything else against `file://` manifests and fixture paths.

---

### W11 — SSH remote: `_bridge`, `--remote`, seeding

- **Difficulty:** hard · **Wave:** 4 · **Depends on:** W03 (cli routing)
- **Goal:** D-M3-9: attach and drive a remote session over ssh stdio with the
  local client unchanged; skew window honored; seeding same-platform only.
- **Scope:** `crates/amx/src/cmd/bridge.rs` (splice + `--daemonize`),
  `crates/amx/src/remote.rs` (ssh spawn over a socketpair, uname probe,
  streaming install with confirmation), `crates/amx/src/main.rs` (`--remote`
  surface), `crates/amx/tests/bridge.rs`, `tests/skew.rs` (bridge transport
  arm), `scripts/ci.sh` + workflow (Linux loopback-sshd job),
  `tests/remote_ssh.rs` (gated on `AMX_TEST_SSHD`).
- **Acceptance:**
  - `a_bridge_child_over_a_socketpair_attaches_and_round_trips_a_verb`
    (no ssh involved; every platform)
  - `bridge_daemonize_starts_a_server_when_none_answers`
  - `every_skew_sample_row_answers_over_the_bridge_transport`
  - `loopback_ssh_attach_renders_a_real_pane` (Linux CI job; skips cleanly
    elsewhere and says why)
  - `a_missing_remote_amx_offers_seeding_only_on_matching_uname_and_refuses_
    cross_platform_with_the_reason`
- **Prompt draft:** Build the byte bridge per `docs/09-m3-plan.md` D-M3-9.
  `_bridge` is a splice, nothing more — resolve the session, connect,
  `copy_bidirectional`, exit with the connect error before the first
  protocol byte if there is one. The local side hands one end of a
  socketpair to the ssh child as stdin+stdout and gives the other to
  `Session::attach` unchanged. Seeding streams the local binary only when
  `uname -s`/`-m` match, after an explicit confirmation, to `~/.local/bin`
  via temp+chmod+mv; anything else is an honest refusal. The loopback sshd
  CI job generates a throwaway key and runs on Linux only — encode the
  darwin skip with the reason in the test, per the rig's existing lore.

---

### W12 — Worktree flow

- **Difficulty:** normal · **Wave:** 4 · **Depends on:** W03 (worktree field,
  cli routing)
- **Goal:** D-M3-10: `amx work <branch>` up, `amx work done` down, restore
  validates.
- **Scope:** `crates/amx/src/cmd/work.rs`, `crates/amx/src/git.rs` (argv-only
  git helpers), `dispatch/workspace.rs` (worktree block pass-through —
  sequential fill of W03's field), `actor/core/workspace.rs` +
  `actor/core/restore.rs` (membership validation → restore report),
  `crates/amx/tests/work.rs` (against real `git` in a tempdir repo).
- **Acceptance:**
  - `work_branch_creates_worktree_workspace_and_agent_and_names_all_three`
  - `work_done_collapses_all_three_and_refuses_a_dirty_tree_without_force`
  - `a_vanished_worktree_restores_as_a_plain_workspace_with_a_report_entry`
  - `work_dir_template_is_config_overridable`
- **Prompt draft:** Build the worktree verbs per `docs/09-m3-plan.md`
  D-M3-10: git runs client-side as argv (never a shell string), the
  workspace carries the membership block W03 added, `done` is
  kill-workspace + `git worktree remove` with the dirty-tree refusal, and
  restore degrades a missing tree to a plain workspace through the M1
  report — never log-only. Test against real git; the destructive path gets
  the same caution the M1 delete work did (pinned under the derived path,
  never user-supplied).

---

### W13 — Layout export/apply

- **Difficulty:** normal · **Wave:** 1 · **Depends on:** —
- **Goal:** D-M3-11: a session's shape as a file, and back.
- **Scope:** `crates/amx/src/cmd/layout.rs`, `crates/amx/src/layout/{schema.rs,
  build.rs}`, `crates/amx/tests/layout.rs`.
- **Acceptance:**
  - `export_apply_export_round_trips_modulo_ids`
  - `apply_builds_the_bsp_by_splits_in_deterministic_order`
  - `name_collisions_suffix_rather_than_merge`
  - `agent_kinds_apply_but_session_refs_never_export`
  - `a_malformed_layout_names_the_line_and_applies_nothing` (parse fully
    before the first call — no half-applied layouts)
- **Prompt draft:** Build layout export/apply per `docs/09-m3-plan.md`
  D-M3-11, entirely client-side over the public verbs: export renders
  `session.state` to TOML (workspaces, splits with ratios, cwds, labels,
  agent kinds — no refs), apply parses the whole file, then replays
  create/split/start in an order that reconstructs the BSP exactly. The
  round-trip test is the spec; write it first.

---

### W14 — Integration: the seams, the exit, the smoke

- **Difficulty:** hard · **Wave:** 5 · **Depends on:** W01–W13
- **Goal:** the wired product: a real upgrade under real load over the real
  binary, and the milestone's exit evidence.
- **Scope:** the seams the wave plan leaves unowned by exception —
  `session/serve.rs`/`session/import.rs` joins that surfaced late,
  `tests/integration.rs`, `tests/handoff_exit.rs` (new), the seam-ledger
  close (every W03 stub answered or deleted, `tests/hygiene.rs` exemption
  retired with it), `docs/notes/m3-live-smoke.md`, and 08 §6-style wave
  outcomes appended here.
- **Acceptance:** §7 verbatim, plus:
  - the seam ledger empties (no stub answers the retired code)
  - wave outcomes written from what happened, not what was hoped
- **Prompt draft:** Run M3's integration exactly as `docs/09-m3-plan.md` §7
  defines the exit. You own every join the file-ownership discipline could
  not grant a concurrent task — hunt them the way M2's V17 did: things that
  are green in-process and dead over a socket. Then the exit suite over the
  real binary, then the by-hand live smoke recorded with date, versions and
  outcomes in `docs/notes/m3-live-smoke.md`. Write the wave-outcomes section
  before you finish: only the divergences, each with what it costs.

---

## 6. Waves and merge order

Merge in wave order; within a wave, any order — no two tasks in a wave touch
the same file.

| Wave | Tasks | Concurrency | Unblocks |
|---|---|---|---|
| 0 | **W01** wedge spike | 1 (needs the machine loaded alone) | W06's gate |
| 1 | W02 one-publisher · W03 contracts · W13 layout | 3 | wave 2 |
| 2 | W04 manifest · W05 fd+protocol · W10 self-update | 3 | wave 3 |
| 3 | W06 export · W07 import · W08 resync | 3 | wave 4 |
| 4 | W09 client reconnect · W11 ssh remote · W12 worktrees | 3 | W14 |
| 5 | **W14** integration + exit | 1 | M3 exit |

**File-ownership check for concurrent waves** (no overlaps):

- Wave 1 — W02: `actor/core/report.rs`, `actor/mod.rs` comment,
  server tests. W03: proto, core (`bus.rs`, state), `pty/*`, `history/
  tracker.rs`, `amx-vt/snapshot.rs`, `handoff/mod.rs` skeleton, dispatch
  stub, goldens/skew, `cli.rs`+`cmd/mod.rs`, the two budget splits. W13:
  `cmd/layout.rs`, `layout/**`, its tests. W02 and W03 both near
  `actor/mod.rs`: W02 edits only the lines 88-90 comment, W03's split moves
  code — **resolution:** W03 owns the file whole; W02's comment fix rides
  W03's split commit as a declared one-line handoff. Disjoint after that.
- Wave 2 — W04: `handoff/{manifest,grid}.rs`, `pane_host/**`, its test
  file. W05: `handoff/{fd,protocol}.rs`, its test file. W10: `cmd/update.rs`,
  `update/**`, workspace `Cargo.toml`, its tests. The shared parent
  `handoff/mod.rs` was planted whole by W03; neither W04 nor W05 edits it.
  Disjoint.
- Wave 3 — W06: `handoff/export.rs`, `gateway.rs`, `core/handoff.rs`,
  `dispatch/session.rs`, `session/serve.rs`, its tests. W07:
  `session/import.rs`, `core/import.rs`, `platform/pty.rs`,
  `cmd/server.rs`, its tests. W08: `conn/**`, `dispatch/stream.rs`,
  `damage/stream.rs`, its tests. The resume pass-through lives in `conn/`,
  not the gateway, so W06 and W08 stay apart; export and import halves are
  separate files by design. Disjoint.
- Wave 4 — W09: `amx-client/src/**`, `cmd/{attach,call}.rs`, client tests.
  W11: `cmd/bridge.rs`, `remote.rs`, `main.rs`, `tests/skew.rs`, CI scripts,
  its tests. W12: `cmd/work.rs`, `git.rs`, `dispatch/workspace.rs`,
  `core/{workspace,restore}.rs`, its tests. Disjoint (all new-verb routing
  was planted by W03).
- Wave 5 — W14 owns the seams by exception.

Cross-wave sequential edits are declared, not discovered: W04/W05 fill W03's
`handoff/` skeleton; W06 fills W03's `dispatch/session.rs` stub and edits
M1's `serve.rs`; W07 mirrors it in a new file; W12 fills the worktree field
W03 planted; W10's handoff glue lands disabled until W06 merges and W14 turns
it on. All sequential, never concurrent.

---

## 7. The M3 exit test

The roadmap's sentence: "upgrade amx under 5 running agents — none die, no
visible screen content lost, waits keep waiting. Attach to the home machine
from a laptop over SSH."

**In CI, over the real binary** (`tests/handoff_exit.rs`, the rig's real
server + real client on a real tty):

1. Five panes running the M2 fake agents (spike-anchored fixtures — real
   agent binaries do not exist on runners, R-M2-8's standing constraint),
   across three workspaces; one attached client at 200×50 on a real pty; a
   distinctive styled sentinel painted in every pane (colors + attributes,
   not just text).
2. A standing `amx wait --until blocked` on a pane that has not blocked yet,
   from a second connection; an `amx events --json` consumer recording seqs.
3. `amx session handoff --binary <the same build>` — current-vs-current,
   exactly as the M0 skew harness runs until a second version exists.
4. Assert, in order: every child pid alive across the swap (none die); the
   successor's `Welcome` carries the same `SessionId` and a larger seq; the
   client reconnected by itself and `pane.read` plus the client's own screen
   show every sentinel styled identically and **non-blank** (the T19 lesson:
   compare screens that demonstrably hold content); the fake agent then
   blocks and the standing wait returns satisfied — it kept waiting through
   the swap; the events consumer's stream is gapless-or-gap-marked, never
   silently short; the old server process exited zero within the drain
   bound; the row ids a pre-swap history fetch returned still address the
   same rows post-swap.
5. The same suite runs once more with the swap replaced by an abort injection
   at each pre-commit stage: the session must still be serving, panes echoing,
   with the reason in `session report`.
6. SSH: the loopback job (Linux CI) attaches over `ssh 127.0.0.1 exec amx
   _bridge`, renders a pane, drives a verb, detaches. Everywhere else the
   bridge-as-child test stands in.

**By hand, before the milestone closes** (recorded in
`docs/notes/m3-live-smoke.md` with date, versions, checklist — the M2 smoke's
format): five real Claude Code sessions mid-conversation; a real staged
binary (a `--version`-bumped build of the same tree); `amx update apply`;
every conversation scrolls back past the swap and answers its
remembered-word probe; an interrupted upgrade (kill the importer mid-restore)
aborts back to a working session; one SSH attach from a genuinely different
machine. Green CI plus this checklist is the exit — green CI alone is not,
three times proven.

---

## 8. Risks & findings

Flagged for the orchestrator, not silently resolved.

**R-M3-1 — The wedge gates the export path and only that.** §2's outcome tree
is the contract: W06 does not merge over an undiagnosed *and* unbounded
drain. If W01 lands outcome (b) or (c), the watchdog is load-bearing and the
live smoke gains the old-server post-mortem step. The milestone's other
tracks are structurally independent of the drain and do not wait.

**R-M3-2 — Grid-synthesis fidelity is the manifest's real risk.** Wide
characters, spacer cells, wrapped rows, grapheme clustering (mode 2027, the
R7 patch history) all have to survive synthesize→replay. W04's property tests
are the defense; the known bound — history rows cross unstyled — is R-M1-1's
accepted precedent, restated in the module docs rather than discovered by a
user. If property testing finds a cell class that cannot round-trip through
the C API, that is a finding for this section, not a silent papering-over.

**R-M3-3 — Post-handoff children report no exit code.** `try_wait` is a
parent's call; an inherited child's death is detected by pty EOF and reported
`ChildExit::Unknown` (D-M3-12). `wait --until exited` and pane teardown are
unaffected; only the numeric status in `pane.exited` degrades, and nothing in
the tree branches on it today. Recorded so a future consumer of exit codes
knows the constraint predates it.

**R-M3-4 — There is no update channel to point at yet.** D-M3-8 ships
machinery and format with a config-overridable URL defaulting to this
repository's GitHub releases; no release pipeline, no prebuilt binaries, no
preview channel exist, and `update check` says so plainly until they do.
Cross-platform remote seeding (D-M3-9) inherits the same reality. Release
engineering is deliberately outside M3's task DAG; treating this plan as
having shipped a hosted channel would be reading a stub as a service.

**R-M3-5 — The socket-transition window can race an auto-daemonizing `amx`.**
Between the exporter's unlink and the importer's bind (≤5 s, usually
milliseconds), a freshly typed `amx` probes Absent and daemonizes a rival,
which then loses the bind race cleanly (`AlreadyRunning`) — but its spawn was
still user-visible noise. Narrowed by retiring late (D-M3-6 point 4);
eliminated would require passing the *listener itself* through the handoff
socket, which is attractive (it is just another fd) but changes the gateway's
bind-owns-the-session invariant — deliberately not done in M3, recorded as
the known follow-up if the race is ever observed outside tests.

**R-M3-6 — SSH CI is Linux-only at tier 2.** darwin runners cannot host the
loopback sshd; the bridge-as-child tier covers all amx code on both
platforms, and the ssh-transport tier runs on Linux. A real second machine
exists only in the live smoke. The skew-over-bridge suite is
current-vs-current until a second protocol version exists — the M0 harness's
honest label, inherited.

**R-M3-7 — Module budgets bind before M3 writes a line.** Over or at the
soft limit today: `actor/persist/actor.rs` 505, `actor/mod.rs` 502;
warning-adjacent and certain to grow: `app/wired.rs` 471, `gateway.rs` 383 +
retire, `conn/events.rs` 302 + cursor plumbing. W03 front-loads the two
overs; W09 splits `app/reconnect.rs` out of `wired.rs` on arrival. The
R-M1-3 rule stands: no split waits for the hard limit.

**R-M3-8 — One new dependency: `sha2`.** For update verification (D-M3-8).
herdr made the same call; the alternatives — hand-rolled SHA-256 or parsing
`sha256sum`/`shasum` output across platforms — are worse on correctness
grounds. HTTP stays out of the tree via the curl subprocess, also herdr's
documented choice. The justification line lands in the commit that adds it.

**R-M3-9 — 04 §2's "one publisher" sentence needs a doc PR.** D-M3-2's fix
implements one publisher *per event kind* (pane actor for pane-thread facts,
Core for lifecycle), which is what keeps damage's never-drops property. The
stale absolutist wording survives in `core/report.rs:5-6` and reads into
04 §2; W02 fixes the comments and the doc PR carries the architecture
wording. Raised here per HACKING.md rather than silently rewritten.

**R-M3-10 — The exporter's post-commit snapshot is a split-brain hazard amx
must fence, herdr merely outraces.** The final Persist push on shutdown
(07 §2's design) would overwrite the successor's `session.json` with the
exporter's dying view. D-M3-6 point 5 disarms it at commit;
`no_final_snapshot_is_written_after_commit` is the pin. Any future change to
the shutdown-push path must keep the fence.

**R-M3-11 — Per-pane fd messages trade one cliff for many syscalls.** herdr's
single-message design caps sessions at 64 panes; amx's per-pane messages have
no cap but cost a round of `sendmsg` per pane and lengthen the frozen window
linearly. At five agents it is nothing; at hundreds of panes the quiesce
itself (1 s budget per pane, parallelizable) dominates anyway. The exit suite
measures wall-clock swap time and records it; if it ever matters, batching
inside SCM_MAX_FD is a mechanical change behind W05's state machine.

**R-M3-12 — `Resume` was frozen in M2 without a consumer, and that bet pays
off here.** No wire change was needed for the resync because the hello's
resume block and the welcome's seq/session fields were golden-frozen ahead of
use. Recorded as precedent the next time a "field with no reader" is
questioned in review: the M3 resync is the reader, one milestone later.

**R-M3-13 — Agent status across the swap is carried, not re-derived, and the
choice is observable.** D-M3-5 carries hub state so the successor's first
`session.state` answer matches the exporter's last one and the attention
queue keeps its order. The alternative (let tier 2 re-detect within ~100 ms)
was rejected because a wait standing on `blocked` would see a spurious
nothing-then-blocked flap and because queue *order* — block time — is not
recoverable from a screen. If the manifest's status ever disagrees with the
first tier-2 read after resume, tier 2 wins by the ordinary fusion rules;
nothing special-cases the handoff.
