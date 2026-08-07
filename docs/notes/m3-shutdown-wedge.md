# The shutdown wedge, diagnosed

W01 of [09-m3-plan.md](../09-m3-plan.md) §2. The rare `SIGTERM`-immune hang in
`Runtime::shutdown` has been carried as an undiagnosed risk since M1 (R-M1-2,
R-M2-6, R-M3-1) and gates M3's export path, because a handoff exporter's life
ends in exactly that drain.

**Outcome, in one paragraph.** One mechanism found, reproduced on demand, and
fixed with a test that fails without the change: a connection's handshake
observed no cancellation token, so one silent peer could hold the drain open
forever. The corpse it leaves is the field corpse, descriptor for descriptor.
It is *not* the mechanism that produced the seven wedged servers found alive on
this machine — the evidence rules that out, [§4](#what-this-does-not-settle-and-how-far-the-evidence-goes)
says how — so a second, rarer path is still open, now narrowed to two lines of
code and instrumented to announce itself. [§6](#6-what-this-means-for-the-export-path-w06)
says what W06 may rely on and what it may not.

Everything below was measured on this tree during the spike. Where a claim is a
hypothesis it says so.

---

## 1. What was found alive

The spike began by looking for a wedge to catch. There were seven on the
machine already: leftover `amx server` processes from six earlier milestone
worktrees, three to nine hours old, none of them anybody's session.

The discriminator is a second `SIGTERM`. A server that was merely leaked —
started and never stopped — is still parked in `watch_signals`, and a signal
ends it. A server whose drain wedged already consumed its signal hours ago;
tokio's handler is still installed, so the second one is caught and dropped,
and nothing happens. Of nine leftover servers, **two exited on `SIGTERM` and
seven did not**. The seven are wedges, caught alive rather than reconstructed.

Their state, read out of `/proc` (no debugger — see [§5](#5-why-there-are-no-backtraces)):

| | wedged (7) | merely leaked (2) |
|---|---|---|
| threads named `amx-pty-*` / `amx-vt-*` | 0 | 2 |
| pipes (the pty actor's wake pipes) | 0 | 4 |
| `inotify` (the config watcher) | 0 | 1 |
| listening socket | 1 | 1 |
| accepted connection | **1** | 0 |
| CPU consumed since the signal | 00:00:00 | — |
| thread run states | all `S`, futex or epoll | — |

Read straight off: in every wedged server the config watcher and every pane
have finished and released what they held, and what is left is **the gateway
task and exactly one connection task**. Nothing is running, so it is a
deadlock, not a spin.

`ss -x` adds one fact that turns out to matter: each stuck connection is
`ESTAB` with `Recv-Q 0`, `Send-Q 0`, and **no peer** — the client process that
opened it is long gone.

---

## 2. The drain now names what it is waiting for

Nothing in the tree could say *which* task a stuck drain held, which is the
reason three milestones of post-mortems produced no diagnosis. So before
anything else, `Runtime::spawn` gained a static name per task, `Runtime`
remembers which names are still outstanding, and a drain that overruns
`CENSUS_INTERVAL` publishes what is left — to the log, and to
`<runtime_dir>/drain-census`, because a daemonized server's stderr is
`/dev/null` and a wedge that only logged would be a wedge nobody could read.

The rig reads that file into its own failure message, so a wedge caught in CI
now says which actor held the drain rather than only which threads are parked.
It is what turned the reproduction below from a symptom into a diagnosis in one
run.

---

## 3. The five pty masters were the harness's, not the server's

§2 of the plan records, as a symptom to be explained before anything is
proposed: *"five `/dev/ptmx` masters open in tests that attach once — the pane
spawn repeated — plus an unreaped zombie shell."* Both halves of that reading
are wrong, and the first sent the whole line of inquiry after a loop that does
not exist.

**Measured.** Sampling every `amx` process during a run of `tests/integration.rs`
and reading each pty master's `tty-index` out of `/proc/<pid>/fdinfo`:

```
pid … outlives  masters=[24, 40, 41, 44, 45, 49]  also-held-by-the-test-binary=[24, 40, 41, 44, 45]  not=[49]
pid … winch     masters=[24, 40, 51]              also-held-by-the-test-binary=[24, 40]              not=[51]
pid … picker    masters=[24,40,41,44,45,46,52]    also-held-by-the-test-binary=[24,40,41,44,45,46]   not=[52]
```

Every server holds exactly **one** master of its own — its one pane, confirmed
independently by asking a live server `session state` and counting: one attach,
one pane, one master — plus every master the *test binary* happened to have
open at the instant it was forked. `rig::term::open_pty` and the T17 harness's
copy opened both pty halves inheritable, so each harness terminal travelled
into every `amx` the suite spawned and into the server that client daemonized.
Five inherited plus one own is six, and "five masters in a test that attaches
once" is that, seen from the wrong end.

Fixed: both halves are `CLOEXEC` now (and the slave `NOCTTY`), matching what
the server's own `platform::pty` has always done. After the fix the same probe
reports no server holding any master the test binary holds.
`hygiene.rs::a_harness_terminal_does_not_leak_into_the_processes_it_spawns`
pins it and fails without the change.

**The zombie shell is a symptom, not a cause.** When a pane is taken down by
command rather than by its child ending, `pty::runner::Runner::run` returns
`Ending::Commanded` and never calls `collect_exit`, so nothing `waitpid`s the
child; `UnixPtySession` has no `Drop` that reaps either. In a healthy server
that costs nothing — the process exits moments later and init reaps — and in a
wedged one the zombie sits there for as long as the wedge does. Recorded, not
changed: it is a consequence of the wedge, and giving the commanded path a
reap is a pty-actor change with no bug of its own to fix.

---

## 4. The mechanism, and the one-line experiment that shows it

A connection's opening is the one stretch of its life that watches nothing:

```rust
let hello = negotiate::read_hello(&mut reader).await?;   // waits on the peer
if hello.attach { router.call(… Attached …).await; }     // waits on Core
let identity = Dispatch::ping(&mut router, …).await?;    // waits on Core
negotiate::write_welcome(&mut write_half, &welcome).await?;  // waits on the socket
// ── only here does anything observe ctx.cancel ──
let outcome = tokio::select! { reader::run(…, &ctx.cancel), writer::run(…, cancel) };
```

`Gateway::run` breaks its accept loop on cancellation, cancels the client
token — which none of those four awaits is watching — and then joins its
`JoinSet` of connection tasks. A connection parked in the prologue is never
joined, so the gateway's task never returns. And because that task still holds
an `AgentHandle`, `AgentHub`'s mailbox can never close, so the hub's
drain-to-closure never returns either. `gateway.rs`'s own comment on
`set_agent` names this shape — *"a sender kept alive across the join would be a
mailbox that can never close, which is the shape the undiagnosed drain wedge is
made of"* — and puts it on the gateway's own handle. The live senders are the
connections'.

**The experiment.** Start a server. Connect a socket. Send nothing. `SIGTERM`.

```
connected and saying nothing
WEDGED: the server did not exit within 20 s of SIGTERM
pid 3353219
waiting 15036 ms
2 task(s) not returned: agent-hub, gateway
  0..2 -> /dev/null      3 -> eventpoll   4 -> eventfd   5 -> eventpoll
  6,7,8 -> the signal socketpair
  9  -> socket   (listening)
  13 -> socket   (one accepted connection)
  13 threads, none of them amx-pty-* or amx-vt-*
```

That is §1's table, reproduced on demand: gateway plus one connection, no
pipes, no inotify, no pane threads. Every run, not one in three hundred.

**The fix** puts the prologue in a `select!` against the session's
cancellation, and a cancelled handshake returns rather than being waited for —
no `ClientAttached` was published, so there is no detach to publish either.
Everything after that point was already cancellation-aware, which is where the
guard stops. `tests/seams/shutdown.rs::a_peer_that_never_finishes_saying_hello_does_not_wedge_the_drain`
connects three ways of being silent (nothing, half a header, half a hello)
beside a live session and asserts the stop completes; without the change it
fails on every run, with the census in the failure message.

### What this does not settle, and how far the evidence goes

The prologue stall needs a peer that is still *connected*. The seven found
alive have none: `ss -x` reports each stuck connection's peer inode as `0`,
which — checked against a socket pair built by hand — is what a closed peer
reads as, where a live one shows its inode. And a peerless socket refuses
every await in the prologue rather than parking on it: a read returns
end-of-file immediately (`b""`), a write returns `EPIPE`. Both measured, not
assumed.

So the honest reading is sharper than "probably the same bug":

- **A wedge with the field's exact terminal state is diagnosed and closed**, on
  a trigger — a client that has connected but not yet finished saying hello —
  that an upgrade produces in bulk (see [§6](#6-what-this-means-for-the-export-path-w06)).
- **The seven found alive were stuck somewhere else**: with no peer, the
  handshake cannot be where they were parked. What remains, in a connection
  whose reader has already taken its end-of-file, is the epilogue —
  `ConnEvents::shutdown` awaiting the event pump and the long polls, and
  `ConnStreams::shutdown` awaiting the grid pumps.

That second one is **an open bug this spike did not find**. Every future those
two joins await does select on the connection's token, which is why reading
turned nothing up; 800 rounds of kill-the-client-then-stop-the-server under a
flooding pane produced no wedge either, and neither did ~18,000 storm cycles.
It is rare in the way the original was rare. What has changed is that the next
occurrence arrives with the census attached, and the search starts at those two
joins instead of at the whole server.

---

## 5. Why there are no backtraces

§2 asks for thread backtraces of a captured process. There are none here, for
two reasons, both worth writing down so the next attempt does not spend the
time again.

- `kernel.yama.ptrace_scope` is 1 on this machine: only an ancestor may attach,
  and a harness's `gdb` is a sibling of the server it spawned. `SIGABRT` plus
  `coredumpctl` sidesteps ptrace entirely and is the route the spike harness
  documents — but every one of the seven specimens had had its binary rebuilt
  underneath it (`/proc/<pid>/exe` reads `… (deleted)`), so nothing could be
  symbolised.
- More fundamentally: **a native backtrace cannot name a stuck async task.** A
  parked future is heap state, not a thread stack; the wedged process's threads
  are exactly what a healthy idle server's are. Every wedged specimen showed
  the same thing — main thread in futex, one worker in `epoll_wait`, the rest
  parked — which is the *absence* of information, not the presence of it.

This is why the drain census is the instrument and the debugger is not. §2's
step 2 should be read as "make the process able to say what it is waiting for",
which is now true.

---

## 6. What this means for the export path (W06)

The handoff exporter's shutdown is this drain, on every upgrade rather than
once per `session stop`, and §3 of the plan puts a client-facing gateway
retirement immediately before it. Three things follow.

1. **The prologue hole is closed, and it was the hole the export path was most
   exposed to.** An upgrade retires the gateway while clients are reconnecting,
   which is precisely a burst of freshly-connected peers that have not yet said
   hello — the condition that reproduces the wedge every time. Before this
   change, `session.handoff` would have been rolling that die on every upgrade;
   `amx update apply`'s own reconnect-poll is one of the peers.
2. **The watchdog of §2's outcome (b) is still load-bearing** — more clearly so
   than if the field trigger had merely been unproven, because the evidence
   says it was *not* this one. W06 keeps the bounded post-commit drain: a
   deadline, the census logged, exit non-zero rather than wedge silently. The
   census is what makes that watchdog's output actionable instead of a bare
   timeout — it names the actor, and on this evidence the name to expect is
   `gateway`, with `agent-hub` beside it whenever a connection is the one
   holding on.
3. **Quiesce-and-join may rely on the pane half.** Every wedged specimen had
   already finished every pane: no pty or parser threads, no wake pipes, its
   own master closed. Across seven corpses and every reproduction, the
   `Core`↔`PaneHost` teardown that D-M3-3's quiesce depends on completed. The
   drain's remaining risk is on the *connection* side, which is the side
   handoff retires deliberately rather than races.

---

## 7. Ruled out, with how

Recorded so the next occurrence does not re-derive them.

- **A pane actor parked mid-report.** `Actor::report` awaits `Core`'s bounded
  mailbox and cannot reach the `cancel` arm of its own `select!` while it does.
  Closed already: `Core::run` drops the mailbox *receiver* before `join_panes`,
  so a mid-report send fails rather than waiting (`core/mod.rs`, and the
  comment there says so). Confirmed by reading; consistent with every corpse
  having zero pane threads.
- **A blocking write to a full pty.** The master is `O_NONBLOCK` from
  `UnixPty::spawn`; `flush_once` treats `WouldBlock` as "later".
- **The parser thread taking `response_order`.** The pty thread holds that lock
  across the read callback while it waits for the parser, so the parser taking
  it would deadlock. It does not: the only parser-thread calls into the pty
  handle are `try_write_input*` (non-blocking) and `resize` (a different lock).
- **Every blocking pty control.** `quiesce`, `resume`, `release`,
  `foreground_group` all go through `request(…, timeout)`; none can wait
  forever.
- **A livelock in `Core`'s inner drain loop.** `Core::run` and
  `PaneHost::actor::run` both drain their mailboxes with `try_recv` and no
  cancellation check, so a saturating producer could in principle hold either
  past the signal. Not what happened: every wedged process had consumed
  **zero** CPU since its signal and every thread was asleep.
- **Repeated pane spawning.** [§3](#3-the-five-pty-masters-were-the-harnesss-not-the-servers).
- **Shutdown storms.** ~18,000 attach-and-stop cycles with a real client on a
  real terminal, under eight-way CPU load and a disk-backed `$TMPDIR`, produced
  no wedge — before the fix. The wedge is not a race that repetition finds,
  which is why three milestones of bounded-repetition canaries missed it. That
  is also the honest limit of the storm added to the seams suite: it watches
  the drain, it does not hunt this.

---

## 8. What is in the tree because of this

| | |
|---|---|
| `crates/amx-server/src/runtime.rs` | named tasks, `outstanding()`, the drain census and its file |
| `crates/amx-server/src/session/serve.rs` | the six actors named at their spawn |
| `crates/amx-server/src/conn/mod.rs` | the handshake under cancellation (**W08's file**, taken here by W01's naming rule) |
| `tests/seams/shutdown.rs` | the silent-peer regression, and an attached-client shutdown storm |
| `tests/support/{term,env,platform}.rs` | `CLOEXEC` terminals, the census in a wedge report, `AMX_SPIKE_PRESERVE`, `pty_masters` |
| `crates/amx/tests/support/mod.rs` | `CLOEXEC` terminals in the T17 harness |
| `tests/hygiene.rs` | the terminal-inheritance guard |
| `crates/amx-server/tests/runtime.rs` | the census tests |
| `scripts/spike/wedge.py` | the repetition harness, and how to photograph a wedge |

Cross-wave notes for the orchestrator:

- **W08 shares `conn/mod.rs`, and inherits the open half of this.** The change
  here is fifteen lines around the existing prologue; W08's resume plumbing
  lands beside it, not on it. The unfound path of
  [§4](#what-this-does-not-settle-and-how-far-the-evidence-goes) is in W08's
  neighbourhood too — `ConnEvents::shutdown` and `ConnStreams::shutdown`, both
  in `conn/` — so whoever picks W08 up is the natural owner of a second look,
  with the census to point at it. It does not gate W08: the resync reads a
  connection's *opening*, and nothing about it makes the epilogue worse.
- **W02 shares nothing.** Reports still flow pane→`Core`; only `Core`'s
  republishes go.
Three things seen under load during the spike and **not** chased, because none
of them is the drain. They are recorded with their evidence so the next person
does not have to rediscover them, and none is W01's to fix.

- **`flow_control_urandom_pane…` has a load-sensitive threshold.** It asserts
  the server ingested more than 8 MiB during a fixed flood window; under
  eight-way CPU load with three other suites running it lands just under —
  `the server ingested only 7923383 bytes` — and did so on **every** round of
  a 35-minute field run. The server is fine; the number is a machine-speed
  constant in a test, which is the shape the rig's own wall-clock guard exists
  to keep out. Worth turning into a bound on rate-over-observed-time, or into
  a wait on the consequence.
- **`frame on unbound channel` under the same flood**, three times in thirty
  rounds, killing the client with a timed-out render rather than a threshold.
  This one is *not* a test artefact: a client refusing a frame on a channel it
  has not bound is a stream-lifecycle race, and it belongs to whoever owns
  `stream.bind` next (W08 binds a `generation` onto exactly that path).
- **A server can be signalled before it can hear.** Seen once: a clean stop
  that ended in `SIGTERM`'s *default* disposition, killing the process outright
  with no drain, no final capture and the socket left behind. `Gateway::bind`
  is what makes the socket answer, and `watch_signals` is spawned several
  statements later — so an `amx session stop` that arrives in that window is
  fatal rather than graceful. Narrow, but real, and it is in `session/serve.rs`,
  which W06 already owns.
