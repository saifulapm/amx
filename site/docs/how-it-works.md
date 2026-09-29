# How it works

amx has no daemon. Each command reads the records on disk and asks tmux about
the panes, answers, and exits. The view does the same once a second. Agents
keep running when no amx process is alive, and killing amx kills nothing.

## One tmux session per agent

Each agent is a detached tmux session named `amx-<id>` with one pane, on the
tmux server you are in (or your default server when you are not in tmux). amx
brings no tmux config: the server reads your `~/.tmux.conf`. Starting an agent
never changes what your own tmux client is showing.

amx is never between you and the agent. `amx attach` and `enter` in the view
are tmux switching or attaching your client. amx addresses panes by tmux pane
id, which is why it needs tmux 3.2 or newer.

## Starting an agent

`amx new` does this:

1. Picks an id from the task (`port-the-importer-k3f`), or uses `--name`.
2. Cuts a worktree if there is one to cut, and runs the `copy`, `link` and
   `setup` config steps in it.
3. Writes the record: `meta.json` (how it was started), `handoff.json` (the
   task and the vendor's argv), and a one-time copy of your environment.
4. Opens a detached tmux session that runs `amx _boot <id>`.

`_boot` restores the environment, deletes that copy, and runs the vendor
through `sh -c`, followed by `amx _exit <id> <status>` so the record learns
how the process ended. The pane's environment carries `AMX_ID`, `AMX_DIR` and
`AMX_AGENT_DIR` so everything running inside it can find the record.

The underscore verbs are amx calling itself and are hidden from `--help`:

| Verb | Called by | Does |
| --- | --- | --- |
| `_boot` | the new pane | Restores the environment and runs the agent. |
| `_exit` | the pane's shell, after the agent exits | Records the exit status. |
| `_hook` | the vendor's hooks, plugin or extension | Records one event from stdin. |
| `_park` | a tmux timer set when an agent goes idle | Closes the pane of an agent idle for `park_after` seconds, if nobody is attached. |

## Hooks versus the screen

amx learns what an agent is doing in two ways.

**Hooks.** The file `amx setup` installed makes the vendor call `amx _hook`
at each moment: session started, prompt submitted, tool called, question
asked, turn ended. Each call appends a line to `events.jsonl` and updates
`state.json`. The turn-end event carries the agent's answer, which is what
`amx result` prints. The hook finds its agent through `AMX_ID` in the pane's
environment, or through the session id for an adopted agent.

**The screen.** When hooks are missing or have gone quiet, amx captures the
pane and matches it against rules for that vendor: which rows mean a spinner,
a prompt, a permission box, a menu. The rules are TOML files compiled into the
binary (`assets/screen-rules*.toml`), measured against real screens of a
specific vendor version. `docs/*-screens.md` in the repository holds the
captures they were measured on.

Hooks are better: they carry the answer and the question's choices, and they
cannot misread a screen. The screen covers the gaps, such as an agent whose
wiring is missing, a vendor screen that fires no event, or a vendor like
codex that sends no event for an interrupted turn.

## How a state is decided

Every reading works the state out at that moment, in this order:

1. **The record says it ended.** An exit status or a stop was written. That
   is final.
2. **The pane is gone.** No pane means `stopped`, unless amx parked it on
   purpose, in which case the agent keeps its idle state.
3. **The hooks are fresh.** An event from the last 8 seconds is trusted as
   is. pi's extension and opencode's plugin also touch a heartbeat file during
   long tool calls, which counts as a fresh event.
4. **The screen matches a rule.** The captured pane is matched against the
   vendor's rules.
5. **Nothing matches.** The state is `unknown`, with how long since the agent
   was last heard from. On a vendor with hooks, a record the hooks left at
   `idle` or `waiting` keeps that state instead.

`amx status <id>` prints which of these it used, and `evidence` in the JSON
says the same.

The row's text comes from the newest source available: pi and opencode's live
stream while a turn runs, else the last line of the vendor's transcript, else
the last hook (`Running Bash`).

## Screens in front of the work

Some vendor screens appear before the session starts, so no hook can report
them: the folder-trust question, a first-run setup, codex's hooks-review
screen, an update prompt. An agent stopped on one would wait forever for a
caller that never attaches.

amx handles them three ways:

- The screen rules recognise them, so the row reads `waiting` with the
  question, and `amx answer` or the view can answer it.
- `trust = true` answers the folder-trust screen for worktrees amx cuts,
  before the agent starts. See [Worktrees](worktrees.md#folder-trust-screens).
- `amx doctor` fails its `gate` check while any agent is stuck on such a
  screen, and `amx --dir <path> doctor` tells you in advance whether an agent
  started there would meet the folder-trust screen.

## What is on disk

Everything amx keeps is under `~/.local/state/amx`:

```
~/.local/state/amx/
  agents/<id>/
    meta.json        how it was started: task, command, directory, worktree, branch, base
    state.json       what it is doing, as the last event left it
    events.jsonl     every event, one JSON object per line, in arrival order
    handoff.json     the task and argv the pane was started with
    output           what a command row printed (or the first 64 KB of an agent's pane)
    pr.json          the last pull request lookup for its branch
    summary.asked    the last summary_command request, when that key is set
    scratch/         $AMX_AGENT_DIR, the agent's own directory
  view.json          the view's arrangement and recent tasks and replies
  visited.json       the last 20 agents you went into, for attach --last
  allowed/           copies of project config files you allowed
  models/            cached model lists from pi and codex
```

Records are written under a lock, one writer at a time; readers never lock and
never see a half-written file. Each agent's directory is created `0700`, so
only your user can read its task and answers.

`amx ls` deletes records of agents that finished (done or failed) more than a
week ago. Stopped agents are kept, since the record names the branch they left,
and so is any record whose worktree is still on disk. `amx stop --delete`,
`clear` and `sweep` remove records on request.

Deleting `view.json` or `visited.json` loses only the view's arrangement and
the `--last` trail. Deleting a file in `allowed/` un-allows that project file.

claude, pi and codex keep their transcripts where they always do (for example
`~/.claude/projects/` and `~/.pi/agent/sessions/`), and amx reads them in
place. opencode's plugin writes its message list into the record, as
`opencode-messages.jsonl`.

## Privacy

amx has no network code and sends nothing anywhere. Everything it records
stays in `~/.local/state/amx` on your machine.

It does run other programs that may use the network:

| Program | When |
| --- | --- |
| Your agent CLIs | Always; they talk to their model providers as usual. |
| `gh` or `glab` | To read pull requests for the PR column, `sweep` and `new --pr`. |
| `git fetch` | For `new --pr`, `new --branch` on a branch only origin has, `sweep`, and every five minutes per repository while the view is open. |
| `summary_command`, `on_*` commands | Whatever you configured them to run. |

Nothing else is contacted, and there is no telemetry.
