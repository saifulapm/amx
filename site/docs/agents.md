# Agents

amx knows four coding agent CLIs by name. Anything else still runs, as a pane
amx watches from the outside.

| Agent | Command | Measured against | Reports through |
| --- | --- | --- | --- |
| Claude Code | `claude` | 2.1.276 | a plugin in `~/.claude/skills/amx` |
| pi | `pi` | 0.85.1 | an extension in `~/.pi/agent/extensions/amx.ts` |
| Codex CLI | `codex` | 0.157.1 | hook groups in codex's `hooks.json` |
| opencode | `opencode` | 2.0.16 | a TUI plugin in opencode's config directory |

`claude` is the default. To make another the default, set it in the config:

```toml
agent = "pi"
```

or pick one per agent with `amx new --agent codex "..."`, `agent:codex` on the
view's task line, or `alt+a` in the view.

## Setup

Each agent tells amx what it is doing (turn started, tool called, question
asked, turn ended with this answer) through a file amx installs:

```sh
amx setup claude
amx setup pi
amx setup codex
amx setup opencode
```

| Agent | What `setup` writes |
| --- | --- |
| claude | A plugin directory at `~/.claude/skills/amx`: `plugin.json`, `hooks/hooks.json`, and the amx skill (`SKILL.md`). claude loads it as `amx@skills-dir` for every project. Your settings files are not touched. |
| pi | An extension at `~/.pi/agent/extensions/amx.ts`. With `--subagent`, also `amx-subagent.ts`, a `subagent` tool that calls `amx sub`. |
| codex | Four hook groups merged into `hooks.json` in `$CODEX_HOME` (else `~/.codex`), and a `[hooks.state]` entry per group in `config.toml` marking them trusted, so codex starts without its hooks-review screen. |
| opencode | A plugin at `plugins/amx/tui.js` in `$OPENCODE_CONFIG_DIR` (else `~/.config/opencode`). No config file is edited. |

Rules that hold for all four:

- Any file of yours at the target path is copied aside before amx writes, and
  the copy is named in the output.
- Running `setup` again writes nothing if nothing changed. After an upgrade it
  rewrites what changed.
- The installed files call `amx _hook` from your PATH, so keep one `amx` there.
- `amx uninstall` removes everything `setup` wrote and restores your copies.
  For codex, if the files changed since, it removes only amx's own entries.
- `setup codex` and `setup opencode` read `CODEX_HOME` and
  `OPENCODE_CONFIG_DIR` from the environment they run in. Run them with the
  same environment your agents get.

Without the wiring, amx reads the agent's state off its screen. That works for
status, but amx has no answer to hand back when a turn ends. `amx doctor` lists
which agents are unwired.

## What each supports

| | claude | pi | codex | opencode |
| --- | --- | --- | --- | --- |
| Reports state and answers | yes | yes | yes | yes |
| Transcript on the card and in `logs` | yes | yes | yes | yes |
| `resume` | yes | yes | yes | yes |
| `fork` | yes | yes | yes | no |
| `adopt` | yes | yes | yes | no |
| `trust = true` answers the folder-trust screen | yes | yes | not needed | no screen |
| Vendor agents (`@name` at the front of a task) | yes | no | no | no |
| Live answer text on the card while working | no | yes | no | yes |

A verb the agent cannot do is refused before anything starts, naming what is
missing.

## Dials

`--model`, `--permission` and `--effort` (and `m:`, `p:`, `e:` in the view)
become each vendor's own flags. A dial you do not set passes no flag, and the
vendor uses its own default.

| Agent | Model | Permission | Effort |
| --- | --- | --- | --- |
| claude | `--model`: `fable`, `opus`, `sonnet`, `haiku`, or any model id | `--permission-mode`: `acceptEdits`, `auto`, `bypassPermissions`, `manual`, `dontAsk`, `plan` | `--effort`: `low`, `medium`, `high`, `xhigh`, `max` |
| pi | `--model`: any `provider/id` from `pi --list-models` | none | `--thinking`: `off`, `minimal`, `low`, `medium`, `high`, `xhigh`, `max` |
| codex | `--model`: any model codex lists | `--sandbox`: `read-only`, `workspace-write`, `danger-full-access` | `-c model_reasoning_effort=`: `low`, `medium`, `high`, `xhigh`, `max` |
| opencode | the `model` in `OPENCODE_CONFIG_CONTENT` | `--auto`: `auto` | none |

A value outside the list, or a dial the vendor lacks, is refused with exit 64
and the valid values:

```
$ amx new --agent pi --permission plan "x"
amx new: --permission "plan": amx knows no permission dial for pi
```

To pass a flag amx has no dial for, put it after `--`:

```sh
amx new "port it" -- --add-dir /srv/shared
```

If the vendor's own `--model` appears after `--`, amx does not pass its model
dial.

### Models pick the vendor

`--model` without `--agent` starts whichever vendor offers that model. amx
checks your configured agent first, then the others:

```sh
amx new --model opus "port it"         # claude
amx new --model gpt-5-mini "port it"   # pi, if pi --list-models has it
```

Where each list comes from:

| Agent | Model list |
| --- | --- |
| claude | `fable`, `opus`, `sonnet`, `haiku` |
| pi | `pi --list-models`, cached for an hour |
| codex | `codex debug models`, the ones its own picker shows, cached for an hour |
| opencode | none; use `--agent opencode` or the config's `[opencode] models` |

A `models` list in a vendor's config table replaces the vendor's own. A model
nobody lists is refused, naming what each vendor takes. `--agent` settles the
vendor, and then the model is just its dial.

## Vendor notes

### claude

amx never passes a session id; claude's first hook names the session. A
message sent mid-turn is folded into the running turn, and the card shows it as
`queued` until then.

### pi

amx starts pi with `--session-id <id>` so the session is known from the first
moment. The extension streams the answer being written to the card while a
turn runs, and keeps a heartbeat so long tool calls do not look stalled. pi has
no permission dial and no agents of its own, so `@` on a pi task line is always
a file.

### codex

Every codex amx starts runs as `codex --no-daemon`, with its own app server.
Under the shared server, hooks run in that server's environment, never learn
which agent they belong to, and a turn keeps running after its pane is killed.

codex wires four moments (session start, prompt, tool call, turn end). It has
no notification hook, so amx reads its approval box off the screen, and it
sends nothing for an interrupted or failed turn, so amx reads those endings
from the screen and the rollout file. Skills are `$name` on the task line.

`setup codex` never passes `--dangerously-bypass-hook-trust`; it writes the
same trust hash codex would write if you approved the hooks yourself.

### opencode

Every opencode amx starts runs as `opencode --standalone`, a server that ends
with its pane. The shared service would run turns in whatever environment first
started it, and keep them running after the pane is gone.

The TUI has no model flag, so amx passes the model as
`OPENCODE_CONFIG_CONTENT={"model":"..."}` in the pane's environment, on new
agents only. The task goes on `--prompt`. `interrupt` presses Escape twice,
since opencode's first press only arms the cancel. `stop` signals the plugin
to end the turn first, so the shared service does not resume it later.

opencode puts no session variable in the environment of the commands it runs,
so it cannot be adopted, and its TUI has no way to fork a session.

## Vendor tables in the config

Each of the four can have a table named after its command:

```toml
[claude]
models = ["opus", "sonnet"]
args = ["--add-dir", "/srv/shared"]

[claude.env]
CLAUDE_CONFIG_DIR = "~/.claude-work"

[codex.env]
CODEX_HOME = "~/.codex-work"
```

| Key | Means |
| --- | --- |
| `models` | The models that pick this vendor, replacing its own list. |
| `args` | Arguments added to every agent of this vendor. A dial stands down if the same flag is here. |
| `env` | Variables set for every agent of this vendor. A leading `~` is expanded. amx's own `AMX_*` variables always win. |

`env` is the way to run agents under a second account or through a proxy. See
[Configuration](configuration.md#vendor-tables).

## Other agent commands

`agent` can name any command. One amx has no entry for, such as `aider`, gets
a pane, a row and a state read from its screen, and nothing more: no dials, no
answers, no resume or fork. Pass its flags after `--`.

```sh
amx new --agent aider "fix the tests"
```

## Commands as rows

`--exec` runs a shell command in a pane of its own, beside your agents:

```sh
amx new --exec 'cargo test --all'
amx new --exec 'ssh build01 make release && curl -fsS "$HOOK"'
```

In the view, the same thing is a task line starting with `!`.

- The whole string goes to `sh -c`, so pipes and `&&` are one row.
- It runs in the directory as it is, never in a worktree.
- The dials, `--role` and arguments after `--` are refused, since there is no
  vendor.
- The row reads `working` with the last line printed, then `done` or `failed`
  by exit status. `exit` in the JSON is the code.
- Everything it prints is saved to `output` in the record, so `amx logs <id>`
  and the card work after the pane is gone. `amx result` has nothing to return
  for a command; check `amx status <id> --json` instead.
- `$AMX_AGENT_DIR` points at a scratch directory, as in every amx pane.
