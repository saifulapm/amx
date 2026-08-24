---
name: amx
description: "Run coding agents as shell commands with amx: spawn one per task, answer what it stops on, and read its answer back. Use when a job splits into pieces that can run at once (review these five tracks, port this API across four services), when work should carry on while you do something else, or when the user says 'spawn an agent', 'run these in parallel', or names amx."
---

# amx: agents as shell commands

An amx agent is a real coding-agent session in a tmux pane. You start it, it
works, it may stop to ask you something, and it hands you an answer. Each of
those is a command with an exit code, so driving agents is shell scripting.
Nothing here needs a screen scraped or a state file polled.

## The verbs

| Verb | What it does |
|---|---|
| `amx new "<task>"` | Start an agent on the task. Prints its id, and nothing else. |
| `amx result <id> [--timeout N]` | Block until the turn ends, then print what it said. |
| `amx answer <id> <key>` | Answer the question it stopped on. |
| `amx send <id> "<text>"` | Give a working or idle agent its next turn. |
| `amx ls [--json]` | Every agent, one line each. |
| `amx status <id> [--json]` | One agent, and which signal that state came from. |
| `amx events [<ids>] [--follow] [--json]` | Every agent's log, merged in time order. |
| `amx diff <id> [--stat]` | What it has changed, against the commit its tree was cut from. |
| `amx stop <id> [--force]` | End it, and say what happens to its worktree and branch. |

Three more are for a person rather than a script: `amx attach <id>` hands the
terminal to the agent's pane, `amx resume <id>` starts a stopped agent again on
the conversation it had, and `amx doctor` says what the machine is missing.

## Exit codes are the interface

| Code | Means | Do |
|---|---|---|
| `0` | The turn ended. The answer is on stdout. | Read it. |
| `1` | Failed, stopped, or ended with no answer to give. | Read stderr, which names the remedy. |
| `2` | Blocked. From `result` that means a question, and the question is on stdout. | Answer it, then call `result` again. |
| `3` | `--timeout` expired. The agent is still working. | Call `result` again, or go and do something else. |
| `64` | The command line was wrong, including an answer the question would not take. | Fix the command line. Nothing reached the agent. |

`send` exits `2` while the agent is waiting on a question: text typed at a
permission prompt answers the prompt, so answer it first rather than queueing a
message behind it. `answer` exits `2` when nothing is pending. `new` exits `2`
at the agent cap.

## Reading the question

A wait never goes through a question. The question usually arrives during the
wait, and a caller that cannot see it cannot answer it, so `result` gives the
wait up and puts the question where the answer would have gone: stdout, with
the choices under it, numbered the way `amx answer` takes them.

```
Claude needs your permission to use Bash
1. Yes
2. Yes, and don't ask again for bash commands in /srv/app
3. No, and tell Claude what to do differently
```

What it will take depends on what kind of question it is, which
`amx status <id> --json` says under `.kind`:

| `.kind` | What answers it |
|---|---|
| `permission` | `y`, `n`, `1` to `9`, `enter`, `esc` |
| `question` | `1` to `9`, `enter`, `esc`, or words of your own: `amx answer <id> "keep the old importer"` |
| `trust` | `enter`. This is the folder-trust screen, and amx has already answered it for a worktree it cut. |

A `question` may take more than one choice, and the screen does not say which
sort it is. `.multi` in that same JSON does: when it is `true`, name the
choices you want and amx checks each and submits, `amx answer <id> 1,3`. When
it is `false` that command line is refused and nothing reaches the agent.

Words that read as a key need `--text`, which says they are words:
`amx answer <id> --text 2` answers with the character `2`, where a bare `2` is
the second choice.

Some questions take a note beside the choice, the ones whose options carry a
`preview` in `.questions`: `amx answer <id> 1 --note "keep the subtitle"`. A
question without one is refused, and so is a note with no choice to ride
beside.

Answering clears the question from the record, so the next `result` waits for
the turn rather than handing you the same question back.

## The loop

This is the whole pattern: call `result`, and branch on how it came back. Copy
it, or run it as a script that takes an agent id and, optionally, a follow-up
turn.

```sh
#!/bin/sh
# Drive one agent to an answer, answering whatever it stops on.
# Usage: loop.sh <id> [follow-up turn]
set -u

: "${AMX_TIMEOUT:=300}"      # seconds any one turn may take
: "${AMX_MAX_ANSWERS:=3}"    # answers one agent may cost before we give up

answer_of() {
    answers=0
    while :; do
        # Keep the code in a variable: after `if cmd; then ...; fi` with no
        # else, `$?` is the if's status and not the command's.
        out=$(amx result "$1" --timeout "$AMX_TIMEOUT") && rc=0 || rc=$?
        case $rc in
            0)  printf '%s\n' "$out"
                return 0
                ;;
            2)  # It is asking, and the question is what came back.
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

# A follow-up turn on the same agent. The `result` after a send can only be
# that turn's answer, never the one before it.
if [ $# -gt 1 ]; then
    amx send "$1" "$2" || exit 1
    second=$(answer_of "$1") || exit $?
    printf 'second: %s\n' "$second"
fi
```

The loop answers `1`, the first choice, which is what a permission prompt and
a menu both read. The folder-trust screen is the one that wants `enter`
instead, and amx has already answered that for a worktree it cut. If an agent
seems stuck at its very first turn, `amx doctor` names any that never got past
the vendor's own setup.

Spawn, drive, end it:

```sh
id=$(amx new "review docs/plan/tracks/03-orchestration.md and list every risk")
loop.sh "$id"
amx stop "$id" --force
```

Several at once. Spawn them all, then collect, since each `result` blocks on
its own agent and the rest keep working while it does:

```sh
for track in docs/plan/tracks/*.md; do
    ids="$ids $(amx new "review $track and list every risk")"
done
for id in $ids; do
    amx result "$id" --timeout 900
done
```

While they run: `amx ls` for a snapshot, `amx ls --json` when a program is
reading it, `amx events --follow` for the merged log, and `amx status <id>`
when one is in a state you did not expect.

## Guardrails

- **Respect the cap.** `max_agents` in `~/.config/amx/config.toml` defaults to
  5, and `amx new` refuses past it with exit `2`. Collect some results and let
  the finished agents go rather than working around it.
- **Only touch agents you spawned.** `amx ls` shows every agent on the machine,
  the user's own included. Never send to, answer or stop an id you did not
  create.
- **Give the task at spawn.** An agent started empty and prompted afterwards
  has a turn you have to catch first, and nothing in `ls` says what it is for.
- **Read the answer with `result`.** It hands back what the agent wrote,
  verbatim. The pane holds a redrawn screen, escape codes and whatever has
  scrolled past.
- **Spawning never moves anybody.** Every agent goes in a detached tmux
  session of its own, `amx-<id>`, so `amx new` from inside tmux leaves whoever
  typed it looking at what they were looking at.
- **Worktrees are already the default.** Each agent gets its own at
  `<repo>/.amx/worktrees/<id>` on branch `amx/<id>`, so two of them cannot
  collide in one checkout. `amx diff <id>` is how you read that work. Nothing
  is merged for you.
- **Never block for ever.** Every `result` in an unattended script takes
  `--timeout`. A question ends the call on its own with exit `2`, so a deadline
  cannot bound the answering: bound that yourself, the way the loop above stops
  after `AMX_MAX_ANSWERS`.
- **If answers keep coming back empty, run `amx doctor`.** Answers are taken
  from the agent's own hook events. Without the hooks wired, amx can still say
  what an agent is doing, but it has nothing to hand you when the turn ends.
