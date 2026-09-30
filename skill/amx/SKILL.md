---
name: amx
description: "Run coding agents as shell commands with amx: spawn one per task, answer what it stops on, and read its answer back. Use when a job splits into pieces that can run at once (review these five tracks, port this API across four services), when work should carry on while you do something else, or when the user says 'spawn an agent', 'run these in parallel', or names amx."
---

# amx: coding agents as shell commands

An amx agent is a coding-agent session (claude, pi, codex or opencode) in its
own tmux pane. You start it with a task, it works, it may stop on a question,
and its turn ends with an answer. Each step is a command with an exit code, so
driving agents is ordinary shell scripting.

## The verbs

| Command | What it does |
|---|---|
| `amx new "<task>"` | Start an agent on the task. Prints its id and nothing else. `--agent`, `--model`, `--permission` and `--effort` choose the vendor and its settings; `--file <path>` (or `-` for stdin) reads a long task. |
| `amx new --exec "<command>"` | Run a shell command as a row of its own. It ends `done` or `failed` by its exit code. |
| `amx result <id> [--timeout N]` | Block until the turn ends, then print the answer. |
| `amx result --children <id> [--json]` | Collect the answers of every child of that agent. |
| `amx sub "<task>" [--bg] [--json] [--timeout N]` | `new` plus `result` in one call. The child's record names your pane's agent as its parent (`--parent <id>` names another). It runs in the parent's directory unless given `--worktree` or `--dir`. Prints the id on stderr and the answer on stdout; `--json` prints one object with both. `--bg` returns once the child exists and prints its id on stdout. |
| `amx wait <id>... [--any] [--for STATE] [--timeout N]` | Block until each agent has settled, printing `<id> <state>` as each does. `--any` returns at the first; `--children <id>` waits on every child of that agent. |
| `amx answer <id> <key>` | Answer the question the agent stopped on. See below. |
| `amx send <id> "<text>"` | Give a working or idle agent its next turn. `--file <path>` (or `-`) for long text. |
| `amx interrupt <id>` | End the turn in progress. The agent and its conversation stay. |
| `amx ls [--json]` | Every agent, one line each. |
| `amx status <id> [--json]` | One agent's state and which signal it came from. |
| `amx events [<ids>] [--follow] [--json]` | The agents' event logs, merged in time order. |
| `amx diff <id> [--stat] [--from <ref>]` | The agent's changes against the commit it started from, or against `--from`. |
| `amx logs <id> [--lines N]` | Recent history without attaching: the vendor's transcript, else the pane, else the recorded answer. |
| `amx fork <id> ["<task>"]` | Start a second agent on a copy of this one's conversation. Prints the new id. |
| `amx rename <id> "<name>"` | Change the name on the user's wall. The id stays the same. |
| `amx stop <id> [--force] [--delete]` | End the agent. `--force` takes the defaults for its worktree and branch without asking; `--delete` removes its record too. |

For a person rather than a script: `amx attach <id>` opens the agent's pane,
`amx resume <id>` restarts a stopped agent on its conversation, and
`amx doctor` reports what the machine is missing.

`amx adopt`, run inside an agent's tmux pane, puts that running session (you,
if you run it) on the user's wall as an amx agent. It starts and sends
nothing. Run it only when the user asks. From codex, use shell mode
(`!amx adopt`) or the shell tool, which pass `$CODEX_SESSION_ID`. opencode
sessions cannot be adopted.

## Exit codes

| Code | Means | Do |
|---|---|---|
| `0` | The turn ended. The answer is on stdout. | Read it. |
| `1` | Failed, stopped, or ended without an answer. Also `interrupt` with no turn to end, and any id that names no agent. | Read stderr; it says what to do. |
| `2` | Blocked. From `result` and `sub`, the agent is asking and the question is on stdout. | Answer it, then call `result` again. |
| `3` | `--timeout` expired. The agent is still working. | Call `result` again later. |
| `64` | Bad command line, including an answer the question does not accept. Nothing reached the agent. | Fix the command. |

Other exit `2` cases: `send` and `interrupt` while the agent is waiting on a
question (both hand back the question, since what they type would answer it);
`answer` with nothing pending; `new`, `sub`, `fork` and `resume` at the agent
cap; `new` and `sub` past `subagent_depth`; `sub` past `max_children` or with
a `--permission` it may not escalate to; `resume` on an agent still running.

## Answering a question

`result` never waits through a question. It exits `2` and prints the question
on stdout, with the choices numbered the way `amx answer` takes them:

```
Claude needs your permission to use Bash
1. Yes
2. Yes, and don't ask again for bash commands in /srv/app
3. No, and tell Claude what to do differently
```

`amx status <id> --json` gives the kind of question under `.kind`:

| `.kind` | Answers |
|---|---|
| `permission` | A choice's number, `y`, `n`, `enter` or `esc`. |
| `question` | A choice's number, `enter`, `esc`, or your own words: `amx answer <id> "keep the old importer"`. |
| `trust` | The vendor's folder-trust screen. A choice's number or `esc`. On claude `1` is `No, exit`, which ends the agent. amx answers this screen itself only when the `trust` config key is on. |

Where amx numbered a list itself (pi's dialogs, claude's trust screen), only
the numbers and `esc` are accepted. If a list comes back with no numbers,
move to the row and take it: `amx answer <id> "down enter"`.

- When `.multi` is `true`, name several choices and amx checks each and
  submits: `amx answer <id> 1,3`. When it is `false`, this is refused.
- `--text` sends words that look like a key: `amx answer <id> --text 2` types
  the character `2`, where a bare `2` picks the second choice.
- Where the choices carry a `preview` in `.questions`, add a note beside the
  choice: `amx answer <id> 1 --note "keep the subtitle"`. Anywhere else a note
  is refused.
- Answering clears the question, so the next `result` waits for the turn.

## The loop

Call `result` and branch on the exit code. Copy this, or save it as a script
that takes an agent id and an optional follow-up turn.

```sh
#!/bin/sh
# Drive one agent to an answer, answering what it stops on.
# Usage: loop.sh <id> [follow-up turn]
set -u

: "${AMX_TIMEOUT:=300}"      # seconds any one turn may take
: "${AMX_MAX_ANSWERS:=3}"    # answers one agent may cost before giving up

answer_of() {
    answers=0
    while :; do
        # Capture the code here: after `if cmd; then ...; fi` with no else,
        # `$?` is the if's status, not the command's.
        out=$(amx result "$1" --timeout "$AMX_TIMEOUT") && rc=0 || rc=$?
        case $rc in
            0)  printf '%s\n' "$out"
                return 0
                ;;
            2)  # It is asking, and $out is the question.
                if amx status "$1" --json | grep -q '"kind": *"trust"'; then
                    printf 'at the folder-trust screen: %s\n' "$out" >&2
                    return 1
                fi
                if [ "$answers" -ge "$AMX_MAX_ANSWERS" ]; then
                    printf 'still asking after %s answers: %s\n' \
                        "$answers" "$out" >&2
                    return 1
                fi
                answers=$((answers + 1))
                printf 'Q: %s\n' "$out" >&2
                amx answer "$1" 1 || return 1
                ;;
            3)  printf 'still working after %ss\n' "$AMX_TIMEOUT" >&2
                return 3
                ;;
            *)  printf 'no answer coming from %s\n' "$1" >&2
                return 1
                ;;
        esac
    done
}

first=$(answer_of "$1") || exit $?
printf 'first: %s\n' "$first"

# A follow-up turn on the same agent. The `result` after a send always
# returns that turn's answer, never the previous one.
if [ $# -gt 1 ]; then
    amx send "$1" "$2" || exit 1
    second=$(answer_of "$1") || exit $?
    printf 'second: %s\n' "$second"
fi
```

The loop answers `1`: allow on a permission prompt, the first choice on a
menu. It stops at a folder-trust screen, where `1` can mean exit; answer that
one yourself or ask the user. If an agent seems stuck on its first turn,
`amx doctor` names any that never got past the vendor's own setup.

Spawn, drive, end:

```sh
id=$(amx new "review src/importer and list every risk")
loop.sh "$id"
amx stop "$id" --force
```

Several at once: spawn them all, then `amx wait --any` returns the first that
is ready. Handle it, drop it from the list, and wait again, so answers arrive
in the order agents finish.

```sh
ids=""
for dir in services/*/; do
    ids="${ids:+$ids }$(amx new "review $dir and list every risk")" || exit 1
done

while [ -n "$ids" ]; do
    # `<id> <state>` for the first one ready. The rest keep working.
    ready=$(amx wait $ids --any --timeout 900) || {
        printf 'nobody ready after 900s: %s\n' "$ids" >&2
        exit 3
    }
    id=${ready%% *}
    state=${ready#* }

    printf '== %s (%s)\n' "$id" "$state"
    case $state in
        waiting) loop.sh "$id" ;;    # stopped on a question: answer, then read
        *)       amx result "$id" ;; # turn is over, so this returns at once
    esac

    rest=""
    for other in $ids; do
        [ "$other" = "$id" ] || rest="${rest:+$rest }$other"
    done
    ids=$rest
done
```

Settled means the turn is over or the agent stopped on a question: `waiting`
needs an answer; `done`, `failed`, `stopped` and `idle` are ready to read.
`--for <state>` waits for one named state instead; `amx wait $ids --for
working` confirms a fleet started. `wait` only says who is ready; `result`
returns the answer.

From inside an agent, fan out with `amx sub --bg "<task>"` per task,
then collect with `amx wait --children "$AMX_ID"` and
`amx result --children "$AMX_ID" --json`.

While they run: `amx ls` for a snapshot (`--json` for programs),
`amx events --follow` for the merged log, and `amx status <id>` when one is in
a state you did not expect.

## Guardrails

- **Respect the cap.** `max_agents` (default 5) counts live agents in the
  project the new one runs in. `max_total`, if set, counts every project on
  the machine. Both are set in `~/.config/amx/config.toml` or the project's
  `.amx/config.toml`. `amx new` exits `2` past either; collect results and
  stop finished agents rather than working around it.
- **Never allow a project file yourself.** A project's `.amx/config.toml`
  takes effect only after the person runs `amx allow`, and any edit needs a
  new allow. Do not run `amx allow` or write that file; tell the person what
  you would change.
- **Only touch agents you started.** `amx ls` shows every agent on the
  machine, the user's own included. Never send to, answer or stop an id you
  did not create.
- **Give the task at spawn.** An agent started empty has a turn you must catch
  first, and its row says nothing about what it is for.
- **Read answers with `result`.** It returns what the agent wrote, verbatim.
  The pane holds a redrawn screen and escape codes. `amx logs <id>` is for
  history, such as an agent taking too long; it never blocks.
- **Spawning never moves anybody.** Each agent runs in a detached tmux session
  `amx-<id>`, so `amx new` inside tmux leaves the user where they were.
- **Each agent gets its own worktree** at `<repo>/.amx/worktrees/<id>` on
  branch `amx/<id>`. Review the work with `amx diff <id>`. Nothing is merged
  for you.
- **Fork to try another approach.** `amx fork <id> "<task>"` starts a second
  agent on a copy of the conversation, in the same directory as the original,
  so do not drive both at the same files at once. An agent that never
  recorded a session cannot be forked, and neither can an opencode agent.
- **Every vendor takes the same verbs.** `--agent opencode` (or `pi`, `codex`)
  starts that vendor instead of the configured one. A `--model` selects the
  vendor whose model list holds it. opencode lists none, so pass
  `--agent opencode` with an opencode model unless the config lists it under
  `[opencode] models`.
- **Long commands can have a row.** `amx new --exec 'cargo test --all'` runs
  it in a pane and prints an id. Do not wait on it with `result`: a command
  gives no answer. `amx status <id> --json` has `.state` (`done` or `failed`)
  and `.exit`. The pane closes when the command ends, so redirect output you
  need: `amx new --exec 'make release > build.log 2>&1'`. Every pane amx
  starts gets a scratch directory at `$AMX_AGENT_DIR`, removed with the
  agent's record.
- **Never block forever.** Give every `result` and `wait` in an unattended
  script a `--timeout`. A question ends the call with exit `2`, so the timeout
  does not bound answering; bound that yourself, as the loop does with
  `AMX_MAX_ANSWERS`.
- **If answers come back empty, run `amx doctor`.** Answers come from the
  agent's hook events. Without hooks amx can still tell what an agent is
  doing, but has nothing to return when the turn ends.
