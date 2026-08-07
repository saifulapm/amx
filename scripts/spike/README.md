# Spike harnesses

Throwaway-shaped tools that are not throwaway: each one exists because a
question could only be answered by driving the real thing, and each one is kept
so the answer can be re-derived when the ground moves. Two of them so far —
[the hook-coverage matrix](#the-hook-coverage-harness) and
[the shutdown-wedge loop](#the-shutdown-wedge-harness).

---

## The shutdown-wedge harness

`wedge.py` loops the suites the drain hang has been seen behind, under CPU and
fsync load, and photographs any server that survives its own suite. The finding
lives in [`docs/notes/m3-shutdown-wedge.md`](../../docs/notes/m3-shutdown-wedge.md).

```sh
scripts/spike/wedge.py --list                                    # the suites it knows
scripts/spike/wedge.py --out /var/tmp/wedge --minutes 45         # loop the suites
scripts/spike/wedge.py --out /var/tmp/wedge --minutes 55 \
    --suites session_cli --storm-cycles 400                      # ~600 stops a minute
scripts/spike/wedge.py --out /var/tmp/wedge --suites field --minutes 90   # where the bodies were
scripts/spike/wedge.py --out /var/tmp/wedge --iterations 300 --keep  # stop at the first, alive
```

`--suites field` is the four suites the seven wedged servers found on this
machine actually came from, which is where the next hunt for the *unfound* path
of the note's §4 should start.

What makes a wedge detectable without asking a human: every harness in the tree
signals its server and waits (`Env::drop` runs `amx session stop`, which
`SIGTERM`s and polls the socket for ten seconds), so a server still alive after
its own suite exited has already had the signal and the wait. Attribution is by
`XDG_RUNTIME_DIR` out of `/proc/<pid>/environ` — each round gets a private temp
root, so a survivor names the round that leaked it and nothing else on the
machine can be mistaken for one.

`--storm-cycles` turns up `tests/seams/shutdown.rs`'s attached-client storm via
`$AMX_STORM_CYCLES` and sets `AMX_SPIKE_PRESERVE`, which stops the rig from
killing what it gave up on so there is a live process to read.

### Three traps worth knowing

- **The temp root must be a disk.** `/tmp` is tmpfs on most Linux
  workstations, and a session whose state directory never touches a disk cannot
  reproduce a race that involves one. The default is `/var/tmp`.
- **Do not run the suites under `setsid`.** They open pty slaves, and a session
  leader that opens a terminal *acquires* it — so the first pair to close
  `SIGHUP`s the test binary out from under itself. This cost the spike two
  false wedge sightings before the harness switched to a plain process group.
  (The rig now opens its slaves `NOCTTY` as well, so this is belt and braces.)
- **Backtraces are mostly a dead end.** `kernel.yama.ptrace_scope` = 1 lets
  only an ancestor attach, and a harness's `gdb` is a sibling of the server it
  spawned; `kill -ABRT` plus `coredumpctl debug` sidesteps ptrace but needs the
  binary still on disk, which a rebuilt worktree does not have. And a parked
  async task has no thread stack to print anyway. The thing that actually names
  the culprit is the server's own drain census —
  `<runtime_dir>/drain-census`, and the same text in the rig's failure message.

---

## The hook-coverage harness

Measures what an agent's hook system actually emits, by driving the real agent
in a real terminal and reading its hook log against the script that provoked it.
The findings live in [`docs/notes/hook-coverage.md`](../../docs/notes/hook-coverage.md);
this directory is how they are reproduced. Agent CLIs ship weekly, so the
matrix rots — re-running it should cost one command, and does.

```sh
scripts/spike/claude-matrix.py --out /tmp/spike             # ~12 min, unattended
scripts/spike/codex-matrix.py  --out /tmp/spike-codex       # needs a logged-in codex
scripts/spike/analyze.py /tmp/spike                         # timelines per scenario
scripts/spike/analyze.py /tmp/spike --latency               # dispatch-latency table
scripts/spike/analyze.py /tmp/spike --payloads SessionStart # raw payloads
scripts/spike/claude-matrix.py --list                       # scenario names
scripts/spike/claude-matrix.py --out /tmp/spike --only esc-generation
```

## How it fits together

| File | What it is |
|---|---|
| `hook-log.sh` | The command every subscribed event runs. Stamps `$EPOCHREALTIME` first (a bash builtin — no fork, so the timestamp is the earliest a shell hook can take), then appends one JSON line: event tag, full stdin payload, every `AMX_*` variable it can see, and its own `/proc` ancestry. Takes its log path as an **argument**, never from the environment, because environment inheritance is one of the things under test. |
| `lib/ptydrive.py` | A PTY the agent runs inside: watch what it paints (with arrival timestamps), type at it, record the raw byte stream. Patterns match whitespace loosely — a TUI positions words with cursor moves, so the paint log has no spaces between them. |
| `lib/runner.py` | What both matrices share: the log both hooks and driver marks append to, `wait_hook`, `prompt`, and the scenario loop. A scenario that fails is recorded and the run continues. |
| `lib/scratch.py` | Builds the disposable scratch project (Claude) and the throwaway `CODEX_HOME` (Codex, credentials borrowed by symlink). Nothing here touches the user's own configuration. |
| `claude-matrix.py` | Claude Code's scenarios and its startup gate (the folder-trust dialog). |
| `codex-matrix.py` | Codex's scenarios and its two startup gates (directory trust, then hook review). |
| `analyze.py` | Reads the log back: timelines, the latency table, payload dumps. |

## What a run leaves behind

Under `--out`:

- `hooks.jsonl` — hook invocations and driver marks on one clock. The evidence.
- `dumps/<scenario>.txt` — everything the terminal painted, escapes stripped.
- `rec/<scenario>.raw` + `.chunks.jsonl` — the raw byte stream with arrival
  timestamps, so a timing question nobody thought to ask can still be answered
  offline.
- `results.json` — pass/fail per scenario.

## Adding an agent

The driver and the analyzer are agent-agnostic. A new agent needs a scratch
config builder in `lib/scratch.py` and a matrix module with its own
`start_agent` (the startup gates) and scenarios — roughly the size of
`codex-matrix.py`. Keep the scenario names the same across agents: the whole
point is a matrix whose rows line up.

## Two traps worth knowing

- **The clock.** The logger and the driver both use `CLOCK_REALTIME`, which is
  what makes "the dialog painted 11 ms after the hook process started" a
  sentence about one timeline. Do not swap either side for a monotonic clock.
- **The paint log is not a screen.** It records what the program *painted*, in
  paint order, so "when did this first appear" is exact and "is it on screen
  now" is not a question it can answer. `repainted_since()` is the honest
  approximation, and it is only used where a wrong answer would be visible.
