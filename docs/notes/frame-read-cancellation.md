# `frame on unbound channel 111`: the client's reader, not the protocol

A full `./scripts/ci.sh` after `27e0b28` ("core: one publisher per pane event
kind") failed once in the rig's `adversarial` suite: the client printed

    amx: run the attached client: frame on unbound channel 111

and rendered nothing further. Run on its own the suite passed. This note
records what that refusal meant, because the refusal itself is worth keeping —
it is the check that catches a genuinely desynchronised peer, and it had just
caught a desynchronised *reader* instead.

## Cause

`Session::read_frame_into` is one arm of the client loop's `select!`
(`app::wired::App::run`, and the same shape in `cmd::viewport`). The other arms
— a keystroke on stdin, a `SIGWINCH`, the resize debounce — win that race
routinely, and winning **drops** the read future wherever it had got to.

The reader kept its progress on that future's stack: a `[u8; 5]` header on the
stack and `read_exact` part way through the caller's payload buffer. Dropping
it therefore threw away bytes the socket had already handed over. The next read
started part way through a payload and took a payload byte for a channel byte.

That is where 111 comes from. It is not a stream id — a client of this session
binds two or three streams, numbered from 1 — it is `b'o'`, a byte out of the
frame the reader was in the middle of. Event notifications are JSON on the
control channel and travel through this same cancellable arm; `"workspace"`,
`"focus"`, `"cols"` are all the letter the refusal named.

## Fix

The read's progress moved into the `Session` (`crates/amx-client/src/net.rs`):
how many header bytes are in hand, the payload buffer, and how much of it is
filled — each committed before the next await, which is the only place
cancellation can land. A cancelled read now resumes the same frame, whichever
buffer the next caller passes. The payload buffer is swapped with the caller's
rather than allocated, so the steady state still allocates nothing per frame.

`crates/amx-client/tests/reader.rs` pins both resume points (mid-header and
mid-payload) with a `biased;` select that makes the cancellation deterministic,
and a payload of `b'o'` so a regression reproduces the exact refusal above.

## What was ruled out, and how

- **The server streaming a bound grid before the bind reply.** Impossible as
  the connection is built: `reader::run` and `writer::run` are two futures in
  *one* task (`conn/mod.rs`), so the writer cannot put a frame on the wire
  while dispatch is running, and there is no await between the pump's
  `tokio::spawn` (`conn/streams.rs`) and the reply reaching the outbound queue
  (`conn/reader.rs`). Once both are queued the writer's strict priority puts
  control first. The bind reply therefore always precedes the first frame.
- **A channel id reused across the killed client's teardown.** `ConnStreams`
  is per-connection, ids start at 1 and never go back, and a channel byte means
  nothing on a connection that did not bind it.
- **`27e0b28` as the cause.** It touched `amx-core`'s event module, the agent
  hub, the core's report path and tests — no client code, and nothing on the
  read path. It changed *when pane events are published*, which changed the
  notification traffic flowing through the cancellable arm. Catalyst, not
  cause: the bug is as old as the `select!`.

## Reproducing it

`/tmp` is tmpfs here, so the timing races want a disk-backed `TMPDIR` plus
load. What surfaced this one is the shape `cargo test --workspace` has: every
rig suite's binary running at once. Eight suites in parallel, pre-fix, hit
`frame on unbound channel 111` in 2 rounds out of 8; the same load post-fix ran
16 rounds with none.

One caveat for anyone reusing that harness:
`flow_control_urandom_pane_with_stalled_client_bounds_memory_and_preserves_grids`
asserts the server ingested more than 8 MiB of `/dev/urandom` in its window.
That is a *rate*, and eight suites competing for twelve cores is exactly the
CPU it needs — it fails under this harness for reasons that have nothing to do
with the client. Read that failure as harness noise, not as a regression.
