# Commands

Every verb, what it does, and its flags. `amx <verb> --help` prints the same
from the binary. For exit codes see [Scripting](scripting.md#exit-codes).

An `<id>` is what `amx new` printed, such as `port-the-importer-k3f`. An id
that names no agent exits 1 from every verb.

| Verb | Does |
| --- | --- |
| [`new`](#new) | Start an agent on a task. |
| [`sub`](#sub) | Start a subagent and wait for its answer. |
| [`ls`](#ls) | List agents and their states. |
| [`status`](#status) | Show one agent in detail. |
| [`result`](#result) | Wait for a turn to end and print the answer. |
| [`wait`](#wait) | Wait for several agents at once. |
| [`send`](#send) | Send a message to an agent. |
| [`answer`](#answer) | Answer a question or permission prompt. |
| [`interrupt`](#interrupt) | Cut short the current turn. |
| [`attach`](#attach) | Put an agent's pane in front of you. |
| [`logs`](#logs) | Print an agent's recent conversation. |
| [`events`](#events) | Print the event log. |
| [`diff`](#diff) | Show what an agent changed. |
| [`rename`](#rename) | Change the name shown in the view. |
| [`stop`](#stop) | End an agent and decide about its worktree and branch. |
| [`resume`](#resume) | Restart a stopped or parked agent on its conversation. |
| [`fork`](#fork) | Start a copy of an agent's conversation. |
| [`adopt`](#adopt) | Register an agent you started yourself. |
| [`sweep`](#sweep) | Clear finished agents whose work has landed. |
| [`clear`](#clear) | Clear every finished agent. |
| [`allow`](#allow) | Trust a project's `.amx/config.toml`. |
| [`statusline`](#statusline) | Print counts for a tmux status line. |
| [`doctor`](#doctor) | Check the machine. |
| [`setup`](#setup) | Wire an agent's hooks. |
| [`uninstall`](#uninstall) | Remove amx's wiring and records. |
| [`completion`](#completion) | Print a shell completion script. |

`amx` with no verb opens [the view](view.md), or prints the `ls` table when
stdout is not a terminal. `amx --dir <path>` narrows either to agents working
under that directory.

## new

Start an agent on a task. Prints the new agent's id and nothing else.

```sh
amx new [OPTIONS] [TASK] [-- AGENT_ARGS...]
```

| Flag | Does |
| --- | --- |
| `--file <PATH>` | Read the task from a file, or stdin for `-`. One trailing newline is dropped. |
| `--edit` | Write the task in `$VISUAL`, `$EDITOR` or `vi`. Quitting with an error (`:cq`) starts nothing. |
| `--name <NAME>` | Use this as the id instead of deriving one from the task. |
| `--role <NAME>` | Take dials and a brief from a role file. See [Scripting](scripting.md#roles). |
| `--dir <DIR>` | Run in this directory instead of the current one. |
| `--no-worktree` | Run in the directory as it is. |
| `--base <REF>` | Cut the worktree from this ref instead of the current checkout. |
| `--branch <NAME>` | Put the worktree on this existing branch. |
| `--pr <N>` | Put the worktree on this pull request's branch. Needs `gh`. |
| `--with-changes` | Move your uncommitted work into the new worktree. |
| `--exec` | Run the task as a shell command instead of an agent. |
| `--check` | Resolve the role, vendor and dials, start nothing: exit 0, or the refusal a spawn would meet. |
| `--agent <COMMAND>` | The agent command to run, such as `pi` or `codex`. |
| `--model <MODEL>` | Model. Also picks the vendor when `--agent` is not given. |
| `--permission <MODE>` | Permission mode. |
| `--effort <LEVEL>` | Reasoning effort. |

The task comes from exactly one of: the argument, `--file` or `--edit`. An
empty task is refused.

```sh
amx new "fix the login bug"
amx new --model opus --effort high "review the parser"
amx new --no-worktree --dir /srv/app "read the deploy log"
amx new --base main --name importer "port the importer"
amx new --file brief.md
git diff | amx new --file -
amx new --exec 'cargo test --all'
amx new "port it" -- --add-dir /srv/shared
```

Anything after `--` goes to the agent command unchanged, before the task. If
you pass the vendor's own `--model` there, amx does not pass its own.

Dials not given fall back to the config, then to the vendor's default: amx
passes no flag for a dial nobody set. A value the vendor would refuse is
refused here with exit 64.

`--model` alone picks the vendor whose model list contains it, asking your
configured agent first: `--model opus` starts claude. Add `--agent` to keep a
vendor and only change its model. See
[Agents](agents.md#models-pick-the-vendor).

Worktree flags that cannot go together (`--pr` with `--base`, `--branch` with
`--no-worktree`, and so on) are refused. See [Worktrees](worktrees.md).

`new` always starts a top-level agent, even when typed inside another agent's
pane. Use `sub` for a child.

## sub

Start a subagent and wait for its answer: `new` and `result` in one call.

```sh
amx sub [OPTIONS] <TASK> [-- AGENT_ARGS...]
```

| Flag | Does |
| --- | --- |
| `--worktree` | Give the child its own worktree. By default it shares the parent's directory. |
| `--dir <DIR>` | Run the child in this directory. |
| `--no-worktree` | Run in the directory as it is. For a `sub` from outside any pane, which would otherwise cut a worktree. |
| `--name <NAME>` | Use this as the id. |
| `--role <NAME>` | Take dials and a brief from a role file. |
| `--parent <ID>` | Record this agent as the parent. For callers outside a pane. |
| `--no-parent` | Record no parent, even inside a pane. |
| `--context <fresh\|digest>` | `digest` puts the parent's task and latest answer in front of the child's task. Default `fresh`. |
| `--json` | Print one JSON object instead. |
| `--bg` | Print the id and return without waiting. |
| `--timeout <SECONDS>` | Stop waiting after this long. Exit 3. |
| `--agent`, `--model`, `--permission`, `--effort` | As for `new`. |

The answer goes to stdout and the child's id to stderr. Exit codes are
`result`'s. Inside an agent's pane the parent is that agent; the child
inherits its model and effort when it runs the same vendor. Details in
[Scripting](scripting.md#subagents).

```sh
amx sub "find where the auth middleware is registered"
amx sub --worktree "rewrite the parser"
amx sub --bg "run the slow check"
amx sub --json --timeout 300 "summarise the failure"
```

## ls

List every agent, one line each.

```sh
amx ls [--json] [--dir <PATH>]
```

| Flag | Does |
| --- | --- |
| `--json` | Print the stable JSON array. See [Scripting](scripting.md#json-output). |
| `--dir <PATH>` | A directory filter: only agents working under this directory. A worktree agent counts as its repository's. |

```
$ amx ls
working  port-the-importer-k3f     4s  Running Bash
waiting  fix-the-login-bug-a1b    12s  Claude needs your permission to use Bash
done     tidy-the-imports-d4e      2m  the imports are sorted
```

Columns: state, id, time spent working, and what the agent last said or did.
The table shows `idle` as `done`; the JSON keeps `idle`.

`ls` also deletes records of agents that finished more than a week ago, except
stopped agents and agents whose worktree is still on disk.

## status

Show one agent: its state, where that state came from, and details.

```sh
amx status <ID> [--json]
```

```
$ amx status abc-5vo
abc-5vo  failed
  evidence  the record says how it ended
  exit      127
  task      abc
  dir       /tmp/work
  pane      %2
```

Messages sent and not yet taken by the agent are listed as `queued`.
`--json` prints one object with the same fields as `ls --json` plus `queued`.

## result

Wait for the agent's current turn to end and print its answer, exactly as the
agent wrote it.

```sh
amx result <ID> [--timeout <SECONDS>]
amx result --children <ID> [--json] [--timeout <SECONDS>]
```

| Flag | Does |
| --- | --- |
| `--timeout <SECONDS>` | Give up after this long. Exit 3. |
| `--children <ID>` | Wait for every child of this agent and print each answer. |
| `--json` | With `--children`: one object keyed by child id. |

If the agent stops on a question, `result` returns at once with exit 2 and
prints the question and numbered choices. After a `send`, it waits for the turn
that message started. An agent that has already ended returns immediately.

```sh
answer=$(amx result "$id" --timeout 900)
```

## wait

Block until each named agent has settled, printing `<id> <state>` as each one
does.

```sh
amx wait <IDS>... [--any] [--for <STATE>] [--timeout <SECONDS>]
amx wait --children <ID> [...]
```

| Flag | Does |
| --- | --- |
| `--any` | Return when the first one settles. |
| `--for <STATE>` | Wait for this state instead: `starting`, `working`, `waiting`, `idle`, `done`, `failed`, `stopped`, `unknown`. |
| `--children <ID>` | Wait on every child of this agent. |
| `--timeout <SECONDS>` | Give up after this long. Exit 3. |

Settled means `done`, `failed`, `stopped`, `idle` or `waiting`. `wait` prints
no answers; call `result` on each id for those.

```sh
amx wait a b c --timeout 900
first=$(amx wait a b c --any)
amx wait a b c --for working
```

## send

Send a message to a working or idle agent.

```sh
amx send <ID> [TEXT]
amx send <ID> --file <PATH>
```

| Flag | Does |
| --- | --- |
| `--file <PATH>` | Read the message from a file, or stdin for `-`. |

A message sent mid-turn waits in the vendor's queue until the agent takes it.
`send` refuses (exit 2) while the agent is waiting on a question, since text
typed there would answer it: use `answer`. It also refuses a parked agent and
tells you to `resume` it.

```sh
amx send "$id" "now run the linter"
amx send "$id" --file notes.md
```

## answer

Answer the question or permission prompt an agent is waiting on.

```sh
amx answer <ID> <ANSWER>
amx answer <ID> --text <WORDS>
amx answer <ID> <CHOICE> --note <WORDS>
```

| Answer | Means |
| --- | --- |
| `y`, `n` | Yes or no. |
| `1` .. `9` | Pick that choice. |
| `1,3` | Pick several, on a question that takes several. |
| `enter`, `esc` | Take the highlighted choice, or dismiss. |
| `up`, `down` | Move the cursor on a list with no numbers, such as `down enter`. |
| any other words | Type them in the question's free-text field. |

| Flag | Does |
| --- | --- |
| `--text <WORDS>` | Put these words in the free-text field, even if they look like a key. `--text 2` answers with the character `2`. |
| `--note <WORDS>` | Add a note beside the choice, on questions whose choices have previews. |

A permission prompt or folder-trust screen takes one key. A question the
vendor asked takes a choice or words. Anything the question would not take is
refused with exit 64 before a key reaches the pane. With nothing pending,
`answer` exits 2.

`amx status <id> --json` tells you what is pending: `kind` is `permission`,
`question` or `trust`, `options` are the choices, and `multi` is true when
several may be picked. Numbers past the question's own choices are refused.

```sh
amx answer "$id" 1
amx answer "$id" 1,3
amx answer "$id" "keep the old importer"
amx answer "$id" esc
```

## interrupt

End the turn the agent is in the middle of, like pressing Escape in its pane.
The agent, its pane, its worktree and its conversation stay; send it a new
message to continue.

```sh
amx interrupt <ID>
```

On an agent waiting on a question it refuses with exit 2 and prints the
question: dismissing a question is `amx answer <id> esc`. On an agent that is
not working, or a command row, it exits 1.

## attach

Hand this terminal to the agent's pane. Inside tmux your client switches
session; outside tmux, `amx` becomes a tmux client. `ctrl+z` inside the
session takes you back.

```sh
amx attach <ID>
amx attach --next | --prev | --waiting | --last
```

| Flag | Goes to |
| --- | --- |
| `--next` | The next agent on the wall after the one you are in, wrapping. |
| `--prev` | The previous one. |
| `--waiting` | The first agent waiting on a question, else the first ready for review, else the most recently finished. |
| `--last` | The agent this terminal was in before. Pressed twice, it toggles. |

The flags are for tmux key bindings:

```tmux
bind a run-shell "amx attach --waiting"
bind C-a run-shell "amx attach --last"
```

If the agent's pane has gone, `attach` resumes it on its recorded session
first.

## logs

Print the agent's recent history without attaching.

```sh
amx logs <ID> [--lines <N>]
```

| Flag | Does |
| --- | --- |
| `--lines <N>` | How many lines. Default 100. |

`logs` reads the vendor's transcript when there is one: prompts, answers and
tool calls. Otherwise it reads the pane, and after the pane has gone, the
answer amx recorded. For a command row it reads what the command printed.

## events

Print the event log of one or more agents, merged in time order.

```sh
amx events [IDS...] [--follow] [--json]
```

| Flag | Does |
| --- | --- |
| `-f`, `--follow` | Keep printing as events arrive. |
| `--json` | One JSON object per event, with the full payload. |

With no ids it reads every agent.

```
$ amx events echo-hello-from-the-5iu
23:42:32Z  echo-hello-from-the-5iu  exit              code 0
```

## diff

Show what the agent changed, including what it has committed.

```sh
amx diff <ID> [--stat] [--from <REF>]
```

| Flag | Does |
| --- | --- |
| `--stat` | One line per file, and totals. |
| `--from <REF>` | Measure from this ref instead of where the agent started. |

On a terminal, if the config sets `diff` (such as `delta --paging=always`),
the patch opens in that viewer. Down a pipe or with `--stat` it is always
git's own output. See [Worktrees](worktrees.md#diff).

## rename

Change the name the view shows for an agent. The id does not change, and every
verb still takes the id.

```sh
amx rename <ID> <NAME>
```

Names are at most 24 characters. `amx rename <id> <id>` goes back to the
default name.

## stop

End the agent, then decide what happens to its worktree and branch.

```sh
amx stop <ID> [--force] [--delete] [--worktree keep|delete] [--branch keep|delete]
```

| Flag | Does |
| --- | --- |
| `--force` | Take the default for every question without asking. |
| `--delete` | Also remove the agent's record. |
| `--worktree keep\|delete` | Answer the worktree question. Default delete. |
| `--branch keep\|delete` | Answer the branch question. Default keep. |

`stop` asks the agent's process group to stop, waits up to five seconds, then
kills it. A worktree with uncommitted work is always kept, and a branch with
commits on no other branch is always kept. `--delete` keeps the record while
its worktree still exists.

```
$ amx stop fix-login-a1b
fix-login-a1b stopped
delete the worktree /home/you/code/app/.amx/worktrees/fix-login-a1b? [Y/n]
removed /home/you/code/app/.amx/worktrees/fix-login-a1b
delete the branch amx/fix-login-a1b? [y/N]
kept amx/fix-login-a1b
```

`stop` ends only the agent named. Its children keep running. More in
[Worktrees](worktrees.md#stopping-an-agent).

## resume

Start an agent again on the conversation it had, in a new pane.

```sh
amx resume <ID> [MESSAGE]
amx resume --all
```

| Flag | Does |
| --- | --- |
| `--all` | Every stopped agent, such as after the tmux server died. |

A message becomes the agent's first turn when it comes back. Without one it
opens at its prompt and waits. `resume` brings back parked agents too. It
refuses an agent that is still running (exit 2), a command row, and an agent
amx has no launch command or session for, such as an adopted one.

```sh
amx resume fix-login-a1b "and now add a test"
```

## fork

Start a second agent on a copy of this one's conversation. Prints the new id.

```sh
amx fork <ID> [TASK]
```

The copy runs in the same directory as the original, with no worktree or
branch of its own, and from then on the two are independent. `amx stop` on a
fork only ends its pane. It needs a vendor that can fork (claude, pi, codex)
and an agent with a recorded session.

```sh
amx fork fix-login-a1b "try it with sqlite instead"
```

## adopt

Register an agent you started yourself, so it appears in the list and every
verb works on it. Run it inside that agent's tmux pane: ask the agent to run
it, or run it from the agent's shell mode.

```sh
amx adopt [--task <TEXT>] [--name <NAME>]
```

| Flag | Does |
| --- | --- |
| `--task <TEXT>` | What the row should say the agent is doing. Default: the directory name. |
| `--name <NAME>` | Use this as the id. |

It reads `$TMUX_PANE` and the vendor's session variable
(`CLAUDE_CODE_SESSION_ID`, `PI_SESSION_ID` or `CODEX_SESSION_ID`). opencode
sets no such variable, so opencode sessions cannot be adopted. Nothing is sent
to the agent. amx made no worktree for it and has no launch command, so `stop`
only ends its pane, and `resume` and `fork` refuse it.

## sweep

Clear finished agents whose work has landed: record, worktree and branch.

```sh
amx sweep [--force]
```

An agent qualifies when it is finished or idle, has a branch, and one of
these is true: its pull request merged or closed, git sees the branch as
merged into the main branch, or the branch is gone from origin. `sweep` lists
them with the reason, asks once, then removes each one. `--force` skips the
question. See [Worktrees](worktrees.md#sweep).

```
fix-login-a1b  #12 merged
port-import-b2c  amx/port-import-b2c merged into main
sweep 2? [y/N]
```

## clear

Forget every finished agent (done, failed or stopped), whether or not its work
landed.

```sh
amx clear [--force]
```

Agents whose work landed go as `sweep` takes them. The rest lose their record
and worktree and keep their branch. Idle agents are not finished and are not
cleared. See [Worktrees](worktrees.md#clear).

## allow

Let amx read the current project's `.amx/config.toml`. It prints the file and
keeps a copy. If the file changes later, amx ignores it until you allow it
again.

```sh
amx allow [--dir <PATH>] [--forget]
```

| Flag | Does |
| --- | --- |
| `--dir <PATH>` | The project, if not the current directory. |
| `--forget` | Stop allowing it. |

See [Configuration](configuration.md#project-config).

## statusline

Print agent counts for a status bar: `✽` agents starting or working, `⚠`
agents waiting or unknown. Prints nothing when both are zero.

```tmux
set -g status-right '#(amx statusline)'
```

## doctor

Check that the machine has what amx needs, and say how to fix what is
missing. Exits 0 when every check passes, 1 otherwise.

```sh
amx doctor [--fix]
amx --dir <PATH> doctor
```

| Check | Passes when |
| --- | --- |
| `tmux` | tmux 3.2 or newer is installed. |
| `agent` | The configured agent command is on the PATH. |
| `config` | The config file parses. |
| `hooks` | Each installed agent is wired and up to date. One line per agent. |
| `amx` | Exactly one `amx` on the PATH, and it is this one. |
| `state` | The state directory is usable. |
| `env` | No old-format handoff file still holds a copy of your environment. |
| `server` | The running tmux server's own directory still exists. |
| `gate` | No agent is stuck on a vendor's startup screen. |
| `store` | No removed worktree is still listed in claude's trust store. |
| `orphans` | Every directory and worktree amx made has a record. |
| `trust` | With `--dir`: an agent started there would not meet a folder-trust screen. |

`--fix` repairs what amx owns: it rewrites old handoff files, removes deleted
worktrees from claude's trust store (after copying the file aside), and
removes id directories that have no record once they are ten minutes old. It
never removes a worktree and never writes an agent's wiring; that is `setup`.

## setup

Write the plugin, extension or hooks an agent needs to report to amx.

```sh
amx setup <claude|pi|codex|opencode> [--subagent]
```

| Flag | Does |
| --- | --- |
| `--subagent` | Also install the agent's `subagent` tool. pi only. |

A bare `amx setup` lists the agents and writes nothing. Any file of yours at
the target path is copied aside first. Running it again when nothing changed
writes nothing. What each vendor gets is on the [Agents](agents.md#setup)
page.

## uninstall

Remove every agent's wiring, restore files amx copied aside, and delete amx's
records. Refuses while any agent is still running.

```sh
amx uninstall
```

## completion

Print a completion script for `bash`, `elvish`, `fish`, `powershell` or `zsh`.

```sh
amx completion fish > ~/.config/fish/completions/amx.fish
```
