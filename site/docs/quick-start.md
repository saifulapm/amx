# Quick start

amx starts coding agents (claude, pi, codex, opencode) in tmux panes, keeps a
record of what each one is doing, and gives you one list to watch, answer and
stop them from. Every action is also a shell command with an exit code, so a
script or another agent can drive the same fleet.

```
$ amx new "port the importer"
port-the-importer-k3f
$ amx ls
working  port-the-importer-k3f     4s  Running Bash
waiting  fix-the-login-bug-a1b    12s  Claude needs your permission to use Bash
done     tidy-the-imports-d4e      2m  the imports are sorted
```

## Requirements

| What | Why |
| --- | --- |
| Linux or macOS | amx is a Unix program. |
| tmux 3.2 or newer | Every agent runs in a tmux pane. Older versions cannot address panes by id. |
| An agent CLI | `claude`, `pi`, `codex` or `opencode`. amx runs `claude` unless you configure another. See [Agents](agents.md). |
| git (optional) | For the worktree each agent gets inside a repository. |
| `gh` or `glab` (optional) | To show pull request numbers on rows. `amx new --pr` needs `gh`. |

## Install

The install script downloads a prebuilt binary for Linux or macOS (x86_64 or
aarch64), checks its SHA-256, and puts it in `~/.local/bin`:

```sh
curl -fsSL https://saifulapm.github.io/amx/install.sh | sh
```

Two variables change what it does:

| Variable | Default | Meaning |
| --- | --- | --- |
| `AMX_VERSION` | `latest` | Release tag to install, such as `v0.1.0`. |
| `AMX_INSTALL_DIR` | `~/.local/bin` | Where the binary goes. |

With a Rust toolchain you can build it instead:

```sh
cargo install --git https://github.com/saifulapm/amx
```

Or from a checkout:

```sh
git clone https://github.com/saifulapm/amx
cd amx
cargo install --path .
```

Keep exactly one `amx` on your PATH. The agents' hooks call whichever `amx`
the PATH finds, and `amx doctor` fails when there are two.

## Wire your agent

Each agent reports to amx through a small plugin or hook file that
`amx setup` writes. Name the agent you use:

```sh
amx setup claude      # or: amx setup pi, amx setup codex, amx setup opencode
```

Run it once per agent you have. A bare `amx setup` lists the names and writes
nothing. Without the wiring amx still works, but it has to read the agent's
screen to know its state, and it cannot hand you the agent's answer. What each
`setup` writes is on the [Agents](agents.md) page.

## Check the machine

```sh
amx doctor
```

`doctor` checks the ten things an agent needs before it can run: tmux, the
agent command, the config file, each agent's wiring, a single amx on the PATH,
the state directory, handoff files from older versions, agents stuck on a
vendor's startup screen, trust entries left for removed worktrees, and leftovers
from crashed spawns. Every failed line says how to fix it:

```
  ok  tmux    3.5a
  ok  agent   claude at /home/you/.local/bin/claude
  ok  config  /home/you/.config/amx/config.toml
  ok  hooks   claude: the plugin at /home/you/.claude/skills/amx
  no  hooks   pi: no extension at /home/you/.pi/agent/extensions/amx.ts
         run `amx setup pi`
  ok  amx     /home/you/.local/bin/amx, the only amx on the PATH
  ...
```

It exits 0 when everything passes and 1 otherwise. An agent you have not
installed is not reported.

## Start an agent

```sh
cd ~/code/myapp
amx new "fix the flaky login test"
```

`new` prints the agent's id and returns. The agent runs in a detached tmux
session named `amx-<id>` on your tmux server. Inside a git repository it gets
its own worktree at `.amx/worktrees/<id>` on a branch `amx/<id>`, so several
agents can work in one repository without touching your checkout. Outside a
repository, or with `--no-worktree`, it runs in the directory as it is.

Then:

```sh
amx ls                 # every agent and its state
amx logs <id>          # its recent conversation
amx attach <id>        # jump into its pane; ctrl+z comes back
amx result <id>        # wait for the turn to end, print the answer
amx stop <id>          # end it and decide about the worktree and branch
```

## Open the view

```sh
amx
```

Typed on its own, `amx` opens a full-screen list of every agent on the
terminal you are at, inside tmux or not. Press `n` to start an agent, `space`
to read one, `enter` to go into its pane, `?` for every key. See
[The view](view.md).

Piped, `amx` prints the same table as `amx ls` and exits:

```sh
amx | grep waiting
```

`amx --dir ~/code/myapp` narrows the view, or the table, to agents working
under that directory.

The first time you close the view it prints a tmux line that shows agent
counts in your status bar. amx never edits your tmux config itself.

## Shell completion

`amx completion <shell>` prints a completion script for `bash`, `elvish`,
`fish`, `powershell` or `zsh`:

```sh
amx completion fish > ~/.config/fish/completions/amx.fish
amx completion zsh > ~/.zfunc/_amx     # with ~/.zfunc on your fpath, before compinit
amx completion bash > ~/.local/share/bash-completion/completions/amx
```

It completes verbs and flags, not agent ids.

## Upgrade

Install the new binary the same way you installed the old one: run the install
script again, or `cargo install` again. Then write the wiring again for each
agent, since the files amx ships change between versions:

```sh
amx setup claude
amx doctor
```

`doctor` flags wiring that is out of date. Regenerate your completion script
too.

## Uninstall

```sh
amx uninstall
```

This removes every agent's wiring, puts back any file of yours that amx had
copied aside, and deletes amx's records under `~/.local/state/amx`. It refuses
while any agent is still running, because the records are the only place their
answers are kept. Stop them first.

Your config in `~/.config/amx` and any worktrees are left alone. Delete the
binary yourself:

```sh
rm ~/.local/bin/amx
```
