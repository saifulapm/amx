# The hook-coverage harness

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
