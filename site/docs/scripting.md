# Scripting

Everything the view does is also a command, and each command reports its
outcome in its exit code. A shell script, a CI job or another coding agent can
start agents, wait for them, answer their questions and collect their answers
without reading a screen.

The core loop is four verbs: `new` starts an agent, `status` or `ls` asks
about it, `result` takes its answer, `stop` ends it.

## Exit codes

| Code | Meaning |
| --- | --- |
| `0` | Done. Any answer is on stdout. |
| `1` | Failed, stopped, or ended with no answer. Also: an id that names no agent. |
| `2` | Blocked. See below. |
| `3` | `--timeout` ran out. The agent is still going. |
| `64` | The command line was wrong, including an answer the question would not take. Nothing reached the agent. |

Exit 2 means something stands in the way:

| Verb | Blocked when |
| --- | --- |
| `result`, `sub` | The agent stopped on a question. The question is on stdout. |
| `send`, `interrupt` | The agent is waiting on a question. |
| `answer` | Nothing is pending. |
| `new`, `sub`, `fork`, `resume` | The agent cap (`max_agents` or `max_total`) is reached. |
| `new`, `sub` | The spawn is deeper than `subagent_depth`. |
| `sub` | The parent already has `max_children` live children, or `--permission` was given without `subagents_may_escalate`. |
| `resume` | The agent is still running. |

Errors and advice go to stderr, one line each.

## One agent, start to finish

```sh
id=$(amx new --no-worktree --file brief.md)

answer=$(amx result "$id" --timeout 900)
case $? in
  0) echo "$answer" ;;                 # the turn ended; that is what it said
  1) echo "failed" >&2 ;;              # it will not answer
  2) echo "asking: $answer" >&2 ;;     # stopped on a question; see below
  3) amx stop "$id" --force ;;         # still going after 15 minutes
esac
```

`result` prints the agent's final message verbatim. It never waits through a
question: if the agent stops to ask, `result` returns 2 with the question on
stdout and its choices numbered the way `amx answer` takes them.

```
Claude needs your permission to use Bash
1. Yes
2. Yes, and don't ask again for bash commands in /srv/app
3. No, and tell Claude what to do differently
```

Answer it and call `result` again:

```sh
amx answer "$id" 1
amx result "$id" --timeout 900
```

`amx status "$id" --json` says what the question takes:

| Field | Meaning |
| --- | --- |
| `kind` | `permission`, `question` or `trust`. |
| `question` | The question text. |
| `options` | The choices, in order. `1` is the first. |
| `multi` | True when several choices may be picked, as `1,3`. |

A permission prompt or trust screen takes one key (`y`, `n`, a digit,
`enter`, `esc`). A question the vendor asked also takes words of your own:
`amx answer "$id" "keep the old importer"`.

## Follow-up turns

```sh
amx send "$id" "now add a test for it"
amx result "$id" --timeout 900
```

After a `send`, `result` waits for the turn that message started. It never
hands back the answer from before the message.

`send` exits 2 if the agent is waiting on a question, because text typed at a
permission prompt answers it. Answer first. To start a stopped agent again
with a message, use `amx resume "$id" "message"`.

## Many agents

`wait` blocks on several agents and prints `<id> <state>` for each as it
settles (its turn ends, it fails, it stops, or it asks a question):

```sh
a=$(amx new "port the users service")
b=$(amx new "port the billing service")
c=$(amx new "port the search service")

amx wait "$a" "$b" "$c" --timeout 1800
for id in "$a" "$b" "$c"; do
  amx result "$id"
done
```

`result` on an agent that has already settled returns at once.

To handle each agent as soon as it is ready, use `--any` in a loop:

```sh
ids="$a $b $c"
while [ -n "$ids" ]; do
  ready=$(amx wait $ids --any --timeout 1800) || exit 3
  id=${ready%% *}
  state=${ready#* }
  if [ "$state" = waiting ]; then
    amx result "$id"       # prints the question, exits 2
    amx answer "$id" 1     # your policy here
    continue
  fi
  amx result "$id"         # its turn is over, so this returns at once
  ids=$(printf '%s\n' $ids | grep -vx "$id" | tr '\n' ' ')
done
```

`--for <state>` waits for a state instead of the end of a turn. `amx wait
"$a" "$b" --for working` confirms both started.

Put a `--timeout` on every `result` and `wait` in unattended scripts.

## JSON output

`amx ls --json` prints an array with one object per agent. `amx status <id>
--json` prints one object with the same fields plus `queued`. Fields are only
ever added, never renamed or removed.

| Field | Meaning |
| --- | --- |
| `id` | The agent's id. |
| `name` | The name set with `rename`, or null. |
| `state` | `starting`, `working`, `waiting`, `idle`, `done`, `failed`, `stopped` or `unknown`. |
| `evidence` | Where the state came from: the record, the hooks, the screen, and so on. |
| `rule` | The screen rule that matched, when the screen was read. |
| `summary` | One recorded line about what it is doing, such as `Running Bash`. |
| `result` | The last answer amx captured. |
| `question`, `options`, `kind`, `multi` | The pending question, when `state` is `waiting`. |
| `task` | What the agent was asked to do. |
| `agent`, `model`, `effort` | The vendor command and the dials the spawn set. Null when not set, and on command rows. |
| `role` | The role it was spawned with. |
| `dir`, `worktree`, `branch`, `base` | Where it runs, and the commit its work is measured from. |
| `parent`, `depth` | Its parent's id and depth in the family. |
| `pr` | Pull requests on its branch: `number` and `standing` (`merged`, `closed`, `draft`, `failing`, `changes`, `running`, `ready`, `open`). |
| `exit` | The exit code of a command row. |
| `created`, `since`, `last_event`, `ended` | Unix timestamps. |
| `age` | Seconds: how long a finished run worked, how long a waiting agent has waited, or how long since a working one was heard from. |
| `worked` | Seconds spent working. |
| `context` | Input tokens of the conversation at the last turn, or null. |
| `last_words` | The agent's last message in its transcript, or null. |
| `pane`, `socket`, `session` | tmux pane, tmux socket, vendor session id. |
| `queued` | `status --json` only: messages sent and not yet taken, oldest first. |

`done`, `failed` and `stopped` are endings. The other states can still change.
The `ls` table prints `idle` as `done`; the JSON keeps `idle`.

```sh
amx ls --json | jq -r '.[] | select(.state == "waiting") | .id'
amx ls --json --dir . | jq length
```

`amx events <id> --json` prints each event with its full payload, one object
per line.

## Subagents

`amx sub` starts a child agent and waits for its answer in one call. Run
inside an agent's pane, the child's record names that agent as its parent, and
the view draws the child under it.

```sh
amx sub "find where the auth middleware is registered"
```

The child's answer goes to stdout and its id to stderr. With `--json` both
come back in one object:

```json
{"id": "find-where-the-a1b", "parent": "port-auth-k3f", "phase": "idle",
 "answer": "...", "evidence": "hooks", "question": null, "options": [], "kind": null}
```

Exit codes are `result`'s. On a 2, answer the child with `amx answer <id>`.

How a child starts:

- In the parent's directory, with no worktree, so it sees the parent's
  uncommitted files. `--worktree` gives it one; `--dir` sends it elsewhere.
- With the parent's model and effort when it runs the same vendor. Flags you
  pass win.
- `--model` alone picks the vendor, as with `new`, so `--model opus` from a pi
  pane starts a claude child. Add `--agent pi` to stay on pi.

Limits from the config:

| Key | Default | Limits |
| --- | --- | --- |
| `subagent_depth` | `2` | How deep a chain may go. 0 forbids children. |
| `max_children` | `8` | Live children per parent. 0 means no limit. |
| `subagents_may_escalate` | `false` | Whether `sub` accepts `--permission`. |

Children do not count toward `max_agents` or `max_total`.

`amx stop` on a parent does not stop its children.

### Fan out, then collect

`--bg` starts the child and returns without waiting. Later, collect the whole
family by parent id:

```sh
amx sub --bg "review the parser changes"
amx sub --bg "review the storage changes"
amx sub --bg "review the API changes"

amx wait --children "$AMX_ID" --timeout 1800
amx result --children "$AMX_ID" --json
```

`result --children` prints one block per child (`<id> <state>` then its answer
or question), or with `--json` one object keyed by child id, each with
`phase`, `answer`, `evidence`, `question`, `options` and `kind`. Its exit code
is the most urgent outcome: 3 timed out, 2 a child is asking, 1 a child
failed, 0 every child answered. A parent with no children exits 1.

`sub --bg` prints the child's id to stderr. Use `--bg --json` to get it on
stdout:

```sh
child=$(amx sub --bg --json "run the slow check" | jq -r .id)
```

### Giving a child context

`--context digest` puts the parent's task and its latest answer in front of
the child's task. The default, `fresh`, gives the child its task alone. A child
that needs the full conversation can read it with `amx logs $AMX_PARENT`.

### From outside a pane

A program that is not running in an agent's pane can still record a parent:

```sh
amx sub --parent wf-t1 --name t1-review --no-worktree --dir "$repo" "review the diff"
```

`--parent` names any agent amx has a record of, running or ended. Outside a
pane `sub` would otherwise cut a worktree like `new`; `--no-worktree` runs it
in the directory as it is. `--no-parent` makes a `sub` inside a pane a
top-level agent.

## Roles

A role is a spawn recipe in a Markdown file: dials in the front matter, a brief
in the body.

```md
---
description: fast recon, returns compressed context
agent: claude
model: sonnet
effort: low
worktree: false
---

You are a scout. Investigate quickly and report findings another agent can
use without re-reading the files.
```

Save it as `~/.config/amx/agents/scout.md`, or in a repository as
`.amx/agents/scout.md`, which replaces a personal role of the same name. Then:

```sh
amx new --role scout "map the payment code"
amx sub --role scout "find every caller of charge()"
```

| Key | Means |
| --- | --- |
| `description` | One line, for listings. |
| `agent` | The agent command. Ignored in a repository's role. |
| `model`, `effort` | Dials. |
| `worktree` | `true` or `false`. |

The body goes in front of the task the agent receives. The record keeps only
the task, and `amx status` names the role. Flags on the command line beat the
role, and for `sub` the role beats what the parent passes down. A role cannot
set the permission mode. An unknown role name exits 64 and lists the roles
amx can see.

## Inside an agent's pane

Every pane amx starts has these variables:

| Variable | Value |
| --- | --- |
| `AMX_ID` | The agent's id. |
| `AMX_DIR` | Its record directory. |
| `AMX_AGENT_DIR` | A scratch directory the agent may write to. Deleted with the record. |
| `AMX_BIN` | The path of the `amx` that started it. |
| `AMX_PARENT` | The parent's id, for a child. |
| `AMX_PARENT_DIR` | The parent's record directory, for a child. |
| `AMX_DEPTH` | 0 for a top-level agent, 1 for a child, and so on. |

`AMX_ID` is how an agent's hooks find its record. If an agent starts another
copy of claude by hand from its shell, that copy inherits `AMX_ID` and its
hooks would report as the parent. Set `AMX_NESTED=1` for it and its hooks stay
silent.

## Teaching an agent to use amx

The repository ships a skill, `skill/amx/SKILL.md`, that teaches a coding
agent this whole loop: the verbs, the exit codes, answering questions, and
running several agents at once.

`amx setup claude` installs it with amx's plugin, so every Claude Code session
has it. For another agent, point it at that file.

`amx setup pi --subagent` gives pi a `subagent` tool that calls `amx sub` and
returns the child's answer. It takes `task`, and optionally `agent`, `model`,
`effort` and `role`.

## Hooks for moments

To run a command whenever an agent waits, finishes, fails or stops, use the
`on_waiting`, `on_idle`, `on_done`, `on_failed` and `on_stopped` config keys.
See [Configuration](configuration.md#moment-commands).
