# amx

Run coding agents as tmux panes.

An agent started by amx is an ordinary program in an ordinary tmux pane. amx
starts it, watches it from the outside, and then gets out of the way. It is
never between the agent and your terminal, and nothing it starts stays
resident. Attaching to an agent is tmux attaching. Killing amx kills nothing.

That buys two things at once. A person gets a wall of agents they can look in
on, answer and stop. A program gets four commands that say what happened in an
exit code: start one, ask after it, take its answer, end it.

```
$ amx new "port the importer"
port-the-importer-k3f
$ amx ls
working  port-the-importer-k3f     4s  Running Bash
waiting  fix-the-login-bug-a1b    12s  Claude needs your permission to use Bash
idle     tidy-the-imports-d4e      2m  the imports are sorted
```

## Requirements

- tmux 3.2 or newer. Earlier versions cannot address panes by id.
- A coding agent CLI. amx runs `claude` unless told otherwise, and expects it
  to speak the same hook events.
- git, for the worktrees `new` cuts. Only needed if you use them.

## Installing

```sh
cargo install --path .
amx doctor --fix
```

`doctor` checks the four things that have to be true before an agent can run:
tmux, the agent command, the config file, and the hooks. `--fix` wires amx's
five hooks into `~/.claude/settings.json`, beside whatever is already there,
after asking once and backing the file up. `amx uninstall` puts the backed-up
bytes back and removes amx's records.

Without the hooks amx falls back to reading panes, which is enough to say what
an agent is doing but not enough to hand you what it said. Answers come from
the events. `amx doctor` says when they are missing.

## Starting an agent

```sh
amx new "fix the login bug"            # in a repository: its own worktree
amx new --no-worktree "run the tests"  # in this directory, as it is
amx new --bg "watch the log"           # out of sight
amx new --name importer "port it"      # a name you chose
```

Where the pane goes depends on where you typed it. Inside tmux, agents tile
into an `amx-wall` window of the session you are in. Outside tmux, they go on
amx's own server (`tmux -L amx`), so the tmux on your machine is never the
thing standing between you and your agents. `--bg` puts one in a session
nobody is attached to.

`new` prints the agent's id and nothing else. Everything after this takes that
id.

Anything after `--` is handed to the agent command untouched, and the task is
added after it, where a prompt goes:

```sh
amx new "port the importer" -- --model opus --session-id "$uuid"
```

## Looking, and answering

```sh
amx                    # the front door: the room, or the list
amx ls                 # every agent, one line each
amx status <id>        # one agent, and which signal that state came from
amx attach <id>        # hand this terminal to its pane
amx send <id> "and now the linter"
amx answer <id> y      # the keys a prompt reads: y, n, 1-9, enter, esc
amx diff <id>          # its worktree against the commit it was cut from
amx events --follow    # every agent's log, merged
```

Typing `amx` on its own opens the room. That is amx's own tmux session, with
the list of agents in one window and the wall the agents tile into beside it.
Inside tmux it opens the list where you are. Down a pipe it prints the table and
exits, so `amx | grep waiting` is a reasonable thing to write.

`send` refuses while an agent is waiting on a question. Text typed at a
permission prompt answers the prompt, and that is not something you can take
back. It hands you the question instead. `answer` is for those, and refuses
when nothing is pending.

## Ending one

```sh
amx stop <id>            # asks what to do with the worktree and the branch
amx stop <id> --force    # takes the defaults, asks nothing
amx resume <id>          # start it again on the conversation it had
amx resume --all         # everything that was stopped, as after a server death
```

Stopping asks the pane's process group to stop, waits, and only then kills it.
An agent cut down mid-sentence loses the answer it was writing. The defaults
lose nothing: the worktree goes, the branch stays, the record stays. A
worktree with uncommitted work in it is always kept, whatever you answer.

## Worktrees

In a git repository, `new` gives each agent a worktree of its own at
`<repo>/.amx/worktrees/<id>` on branch `amx/<id>`, cut from whatever was
checked out at the time. That commit is recorded, which is what lets `amx
diff` show the whole of an agent's work, including what it has already
committed, rather than only what it has not.

`.amx/` is kept out of the repository's status through `.git/info/exclude`, so
nothing about this shows up in a diff of yours. `--no-worktree` runs the agent
in the directory as it is, and `worktrees = false` makes that the default.

## Driving amx from a program

Four questions, four commands, and the exit code is the answer:

| code | means |
| ---- | ----- |
| `0`  | done: the answer, if there is one, is on stdout |
| `1`  | failed, stopped, or ended without an answer; nothing more is coming |
| `2`  | blocked: `result` and `send` on an agent that is asking, `answer` with nothing pending, `new` at the agent cap |
| `3`  | `result --timeout` expired |
| `64` | the command line was wrong |

```sh
id=$(amx new --bg --no-worktree --dir "$dir" "Read $brief and execute it exactly." \
     -- --session-id "$session")

amx ls --json          # every agent: state, since, last_event, summary, question

said=$(amx result "$id" --timeout 900)
case $? in
  0) merge "$said" ;;        # the turn ended, and that is what it said
  1) redispatch "$id" ;;     # it will not answer
  2) park "$id" "$said" ;;   # it is asking, and the question is what came back
  3) amx stop "$id" --force ;;
esac
```

`result` blocks until the turn ends and prints what the agent said, verbatim.
What you capture is what it wrote, not a rendering of it. It never waits
through a question. A question usually arrives *during* the wait, and a caller
that cannot see it cannot answer it. After a `send` it waits for the turn
after that message, never handing back the previous turn's answer.

`ls --json` and `status --json` are stable. Fields are added, never renamed or
removed. Each row carries `id`, `state`, `evidence`, `rule`, `age`, `since`,
`last_event`, `seq`, `summary`, `question`, `result`, `source`, `exit`, `task`,
`dir`, `worktree`, `branch`, `base`, `pane`, `socket`, `session` and `created`,
so one `ls` call answers both "is it still going?" and "when was it last heard
from?" for every agent at once.

`state` is one of `starting`, `working`, `waiting`, `idle`, `done`, `failed`,
`stopped`, `unknown`. `done`, `failed` and `stopped` are endings; every other
state is an agent still worth waiting on.

## How amx knows what an agent is doing

Nothing amx runs stays resident, so there is no process keeping this up to
date. A reader works it out at the moment you ask, in this order:

1. The record ended it. An exit code was written, or a stop was. Nothing
   overrules that.
2. The pane is gone. No pane, no agent.
3. The hooks are fresh. Within 8 seconds, the agent's own events are the best
   account there is.
4. The screen is all there is. Older than that, the pane is captured and
   matched against a ruleset of what the agent's screens actually look like.
5. Neither says anything. No rule claims the screen, so the answer is
   `unknown`, with how long it has been since anything was heard, because "I
   can't tell" is only useful with that beside it.

`amx status` says which of these it used. `evidence` in the JSON is the same
answer for a program.

## Configuration

`~/.config/amx/config.toml`, four keys and no more:

```toml
agent = "claude"        # the command a new agent runs
max_agents = 5          # how many live agents before `new` refuses
worktrees = true        # give each agent its own worktree in a repository
notifications = true    # desktop notification when one needs you or finishes
```

Config is a convenience, never a gate. A file amx cannot read or parse falls
back to these defaults with a warning, because losing an agent to a stray
comma is the worse outcome.

## What is on disk

One directory per agent under `~/.local/state/amx/agents/<id>/`:

- `meta.json` holds how it was started and where to find it again.
- `state.json` holds what it is doing, as the last event left it.
- `events.jsonl` holds one line per event, in the order they arrived.

Writes go through a lock, one at a time; readers never lock and never see half
a document. `ls` sweeps records whose agent finished more than a week ago. A
stopped agent's record is never swept. Somebody stopped it on purpose, and its
record is where the branch it left behind is named.

## Building

```sh
cargo build
cargo test
```

The test suite drives the built binary against throwaway tmux servers and a
stand-in for the agent CLI, so it needs a real tmux but no network and no API
key.
