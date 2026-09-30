# Configuration

amx runs with no config file at all. When you want to change something, write
`~/.config/amx/config.toml` (or `$XDG_CONFIG_HOME/amx/config.toml`). The
repository ships a copy with every key explained and every default written
out, `assets/config.toml`; a copy nobody edits behaves exactly like no file.

| File | What it is |
| --- | --- |
| `~/.config/amx/config.toml` | Your settings. |
| `<repo>/.amx/config.toml` | A project's settings, layered over yours once you run `amx allow`. |
| `~/.config/amx/themes/<name>.toml` | Your themes. |
| `~/.config/amx/agents/<name>.md` | Your roles. See [Scripting](scripting.md#roles). |
| `<repo>/.amx/agents/<name>.md` | A project's roles. |

A config problem never stops an agent from starting:

- An unknown key is a warning; the rest of the file applies.
- A dial value the configured agent would not take is a warning and is dropped.
- A file that cannot be read or parsed, including a key with the wrong type,
  falls back to the defaults with a warning.

## Keys

| Key | Default | Meaning |
| --- | --- | --- |
| `agent` | `"claude"` | The command new agents run: `claude`, `pi`, `codex`, `opencode`, or any other command. May include arguments. |
| `max_agents` | `5` | Live agents allowed in one project before `new` refuses. |
| `max_total` | none | Live agents allowed across all projects. |
| `max_children` | `8` | Live children one parent may have. `0` means no limit. |
| `subagent_depth` | `2` | How deep a chain of subagents may go. `0` forbids children. |
| `subagents_may_escalate` | `false` | Whether `amx sub` accepts `--permission`. |
| `worktrees` | `true` | Give each agent its own worktree inside a repository. |
| `notifications` | `"desktop"` | Where notices go: `"desktop"`, `"terminal"`, `"both"` or `"off"`. |
| `trust` | `false` | Answer the vendor's folder-trust screen for worktrees amx cuts. |
| `model` | none | Default model dial. |
| `permission` | none | Default permission dial. |
| `effort` | none | Default effort dial. |
| `summary_command` | none | Command that writes the one-line summary of a turn. |
| `theme` | `"auto"` | Colour theme for the view. |
| `park_after` | `3600` | Seconds an idle, unwatched agent keeps its pane. `0` never parks. |
| `copy` | `[]` | Files copied from the repository root into a new worktree. |
| `link` | `[]` | Directories in a new worktree symlinked to the repository's own. |
| `setup` | `[]` | Commands run in a new worktree before the agent starts. |
| `base` | none | What new worktrees are cut from. Default: whatever is checked out. |
| `diff` | none | Viewer for `amx diff` at a terminal, such as `"delta --paging=always"`. |
| `on_waiting` | none | Command run when an agent stops on a question. |
| `on_idle` | none | Command run when a turn ends and the agent is back at its prompt. |
| `on_done` | none | Command run when an agent's command exits 0. |
| `on_failed` | none | Command run when an agent's command exits non-zero. |
| `on_stopped` | none | Command run when you stop an agent. |
| `[keys]` | empty | Your own key bindings in the view. |
| `[claude]`, `[pi]`, `[codex]`, `[opencode]` | empty | Per-vendor models, arguments and environment. |

An example:

```toml
agent = "claude"
max_agents = 8
model = "opus"
effort = "high"
base = "main"
copy = [".env"]
link = ["node_modules"]
setup = ["pnpm install"]
diff = "delta --paging=always"
summary_command = "claude -p 'Sum this up in eight words. Answer with the words alone.'"
on_waiting = "curl -sX POST https://hooks.example.com/amx -d @-"

[keys]
"alt+g" = "lazygit"
```

## Agent limits

`max_agents` counts live agents in the project the new agent would run in. A
repository at its limit does not stop you from starting agents in another
one. `max_total` limits the whole machine, and is unset by default.

A live agent here is one that is starting, working or waiting on a question,
with its pane still there. Agents idle at their prompt and command rows do not
count. Subagents do not count toward either limit; `max_children` and
`subagent_depth` bound them instead.

At a limit, `new`, `sub`, `fork` and `resume` exit 2 and name the limit.

## Dials

`model`, `permission` and `effort` set the defaults for new agents of the
configured `agent`. Leave a key out to let the vendor choose; there is no
value that means "vendor default". The values each vendor takes are on the
[Agents](agents.md#dials) page. A flag on the command line beats the config.

## Worktree setup

`copy`, `link`, `setup` and `base` prepare new worktrees. Details on
[Worktrees](worktrees.md#setting-up-a-fresh-tree).

## Summaries

A finished row in the view shows the first line of the agent's answer. Set
`summary_command` to have a command write a short summary instead:

```toml
summary_command = "claude -p 'Sum this up in eight words. Answer with the words alone.'"
```

When a turn ends, the view runs the command in the agent's directory with the
answer on stdin, `AMX_ID` set, and `AMX_NESTED=1` so a claude it starts does
not report as the agent. The first line it prints goes on the row. While an
agent works, the view also asks the command every three minutes what the turn
is about, feeding it the conversation so far.

Only the view runs it: `ls`, `status` and `statusline` never do. Each
finished turn is summarised once, one at a time, so opening the view on many
old agents queues them. A failing command costs only the summary.

## Parking

`park_after` is how long an agent idle at its prompt keeps its pane when
nobody is attached. After that amx closes the pane and keeps the record; the
agent comes back on `enter`, `amx attach` or `amx resume`. Pinned agents and
agents you are attached to are never parked. See
[Worktrees](worktrees.md#parking-idle-agents).

## Notifications

amx posts a notice when an agent stops on a question and when an agent's
command finishes. Nothing is posted about a pane you are already looking at.

| Value | Where notices go |
| --- | --- |
| `"desktop"` | `notify-send` on Linux, `osascript` on macOS. |
| `"terminal"` | An OSC 777 escape to every client of every tmux server under your tmux socket directory. foot, kitty, WezTerm and Ghostty show it as a desktop notification. |
| `"both"` | Both. |
| `"off"` | Nowhere. |

`"terminal"` is for SSH sessions: run the view inside a tmux on the remote
machine and the notice reaches your local terminal. `true` and `false` still
work and mean `"desktop"` and `"off"`.

## Moment commands

The five `on_` keys run a command when an agent reaches a moment:

| Key | When |
| --- | --- |
| `on_waiting` | It stops on a question. |
| `on_idle` | A turn ends and it is back at its prompt. |
| `on_done` | Its command exits 0. |
| `on_failed` | Its command exits non-zero. |
| `on_stopped` | You stop it. |

```toml
on_waiting = "curl -sX POST https://hooks.example.com/amx -d @-"
on_done = "notify-send 'amx' \"$AMX_ID $AMX_STATE\""
on_stopped = "echo $AMX_ID stopped >> ~/amx.log"
```

Each runs through `sh -c` in the agent's worktree (or its directory), detached.
amx does not wait for it, and its output goes nowhere: redirect it yourself if
you want a log. The event that caused the moment arrives on stdin as one JSON
line.

| Variable | Value |
| --- | --- |
| `AMX_ID` | The agent's id. |
| `AMX_STATE` | The state word, as in `amx ls --json`. |
| `AMX_DIR` | The agent's record directory. |
| `AMX_AGENT_DIR` | The agent's scratch directory. |
| `AMX_WORKTREE` | Its worktree, when it has one. |
| `AMX_NESTED` | `1`. |
| `AMX_WATCHED` | `1` if someone is attached to the pane, else `0`. Not set for `on_stopped`. |

## Your own keys

`[keys]` binds keys in the view to shell commands:

```toml
[keys]
"alt+g" = "lazygit"
"alt+t" = "cargo test 2>&1 | less"
"f5" = "make deploy"
```

A key is an optional `ctrl+` and `alt+`, in either order, then one character
or `f1` to `f12`. A capital letter means shift; `shift+` is not a spelling.

Pressed on an agent's row, the view hands over the terminal and runs the
command through `sh -c` in the agent's worktree (or directory), with `AMX_ID`,
`AMX_DIR`, `AMX_AGENT_DIR`, `AMX_NESTED=1` and `AMX_WORKTREE` set. When it
exits, the view comes back and reports only failures.

A key the view already uses cannot be rebound: the view says so when it opens
and binds nothing. The keys screen (`?`) lists your bindings under `yours`.

## Vendor tables

```toml
[claude]
models = ["opus", "sonnet"]
args = ["--add-dir", "/srv/shared"]

[claude.env]
CLAUDE_CONFIG_DIR = "~/.claude-work"

[pi]
models = ["anthropic/claude-opus-4-1", "openai/gpt-5"]
args = ["--approve"]
```

| Key | Meaning |
| --- | --- |
| `models` | The model list used to pick this vendor from `--model`, replacing the vendor's own. |
| `args` | Arguments every agent of this vendor gets. A dial amx would set stands down if the same flag is here. |
| `env` | Environment variables for every agent of this vendor, over the environment you ran amx in. A leading `~` expands to your home. `AMX_ID`, `AMX_BIN`, `AMX_DIR` and `AMX_AGENT_DIR` cannot be overridden. |

Tables are only accepted for `claude`, `pi`, `codex` and `opencode`. Any other
table name is a warning.

## Project config

A repository can carry its own `.amx/config.toml`. It is layered over your
file one key at a time, so a project file with one line changes one key.

Two exceptions: a vendor table like `[claude]` replaces yours whole, and
`[keys]` merges binding by binding.

Every worktree of a repository reads the same file, at the repository root. A
directory outside any repository reads `<dir>/.amx/config.toml`. `.amx/` is
excluded from git, so the file stays local unless you commit it.

### amx allow

A project file can name the program a pane runs and shell commands amx runs,
so amx ignores it until you allow it:

```sh
cd ~/code/app
amx allow              # prints the file and records it
amx allow --forget     # stops allowing it
amx allow --dir ~/code/other
```

amx keeps a copy of exactly what it showed you, under
`~/.local/state/amx/allowed/`. If the file changes afterwards (your edit, a
pull, or an agent's), amx warns once and ignores it until you run `amx allow`
again.

Even when allowed, a project file cannot set:

- `permission`, `trust` or `subagents_may_escalate`,
- in a vendor's `env`: `PATH`, `HOME`, `SHELL`, `CLAUDE_CONFIG_DIR`,
  `CODEX_HOME`, `OPENCODE_CONFIG_DIR`, `OPENCODE_CONFIG`,
  `OPENCODE_CONFIG_CONTENT`, or any `LD_*`, `DYLD_*` or `AMX_*` variable.

A project's roles in `.amx/agents/` need no `allow`, but cannot set `agent` or
`permission`.

### Which file the view reads

`amx --dir <path>` uses that project's file for the view: its dials, its
theme, and its `max_agents` in the header. Plain `amx` shows every agent and
uses your file, counting against `max_total` if you set one.

## Themes

The view uses colour for six things, and a theme sets those six:

```toml
# ~/.config/amx/themes/mine.toml
waiting = "#ffc107"   # waiting on a person
done    = "#4eba65"   # went as intended
failed  = "#ff6b80"   # attempted and failed
stopped = "#999999"   # ended by hand
accent  = "cyan"      # the next agent's dials, the line you type on
cursor  = "#373737"   # background of the cursor's row
```

```toml
theme = "mine"
```

| `theme` value | Meaning |
| --- | --- |
| `auto` | Ask the terminal for its background colour and use `light` or `default`. The default. |
| `default` | Built in, matched to claude's palette, for dark backgrounds. |
| `light` | Built in, for light backgrounds. |
| `terminal` | Built in, using only your terminal's named colours. |
| a name | `~/.config/amx/themes/<name>.toml`. |
| a path containing `/` | That file. |

Colours are a name (`cyan`, `bright black`), a 256-colour index (`134`), or
hex (`#4eba65`). A role you leave out keeps the default's colour. A file that
cannot be read falls back to `default` with a warning.

`auto` sends the terminal an OSC 11 query when the view opens and waits up to
200 ms for the reply, then falls back to `COLORFGBG`, then to `default`.

The view checks the theme file once a second, so edits show up without a
restart. Everything else on the screen uses your terminal's own colours, dim
and bold, and a theme cannot change the glyphs or an agent's own output.

## tmux

amx brings no tmux config and uses the tmux server you already run. The
repository's `assets/tmux.conf` explains each line worth adding; copy what you
want:

```tmux
# Let shift+enter reach the view as a newline.
set -g extended-keys on
set -g extended-keys-format csi-u

# Agent counts in the status bar.
set -g status-right '#(amx statusline)'
set -g status-interval 5

# prefix a: the agent waiting on you. prefix A: the next one. prefix C-a: back.
bind a run-shell "amx attach --waiting"
bind A run-shell "amx attach --next"
bind C-a run-shell "amx attach --last"

# prefix v: the view in its own window.
bind v run-shell "tmux select-window -t amx 2>/dev/null || tmux new-window -n amx amx"

# When an agent's session ends, move to your previous session instead of detaching.
set -g detach-on-destroy off
```

You do not need `set -g mouse on` for the view, and amx binds `ctrl+z` itself,
only in its own `amx-*` sessions.
