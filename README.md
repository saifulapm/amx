<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="site/assets/mark.svg">
    <img src="site/assets/mark-light.svg" alt="amx" width="72">
  </picture>
</p>

<h1 align="center">amx</h1>

<p align="center">
  <a href="https://saifulapm.github.io/amx/">website</a> ·
  <a href="#install">install</a> ·
  <a href="https://saifulapm.github.io/amx/docs/quick-start/">quick start</a> ·
  <a href="https://saifulapm.github.io/amx/docs/">docs</a>
</p>

<p align="center">
  <a href="https://github.com/saifulapm/amx/releases/latest"><img src="https://img.shields.io/github/v/release/saifulapm/amx?label=release&labelColor=333333&color=666666" alt="latest release"></a>
  <a href="https://github.com/saifulapm/amx/actions/workflows/ci.yml"><img src="https://img.shields.io/github/actions/workflow/status/saifulapm/amx/ci.yml?branch=main&label=ci&labelColor=333333&color=666666" alt="CI status"></a>
  <a href="#license"><img src="https://img.shields.io/badge/license-MIT%20or%20Apache--2.0-666666?labelColor=333333" alt="MIT or Apache-2.0"></a>
</p>

---

https://github.com/user-attachments/assets/4ed78eda-e58c-406b-be81-680c1a420f82

**Run coding agents as tmux panes.**

- **Ordinary panes, no daemon.** Every agent is the real claude, codex, pi or opencode in a tmux session of its own. amx starts it and watches from the outside. Attaching is plain tmux, and killing amx stops nothing.
- **One view for all of them.** Run `amx` to see every agent grouped by project, marked working, waiting or done, with what it is doing right now. [The view →](https://saifulapm.github.io/amx/docs/view/)
- **Answer without switching.** Permission prompts and questions open as cards. Press `y`, pick a number or type a reply, from the view or from a script with `amx answer`.
- **A worktree each.** In a git repository every agent gets its own worktree and branch, with its pull request number on the row. `amx diff` shows what it changed and `amx sweep` clears what landed. [Worktrees →](https://saifulapm.github.io/amx/docs/worktrees/)
- **Scriptable, by you or by an agent.** `amx result` waits and prints the answer, and the exit code says done, failed, blocked on a question or timed out. An agent can fan work out with `amx sub` and collect it. [Scripting →](https://saifulapm.github.io/amx/docs/scripting/)
- **Pick up where it stopped.** `amx resume` restarts a stopped agent on its recorded session, and `amx fork` copies a conversation into a second agent.
- **One small binary.** Prebuilt for Linux and macOS, under 6 MB. State is plain JSON under `~/.local/state/amx`.

## Install

```sh
curl -fsSL https://saifulapm.github.io/amx/install.sh | sh
```

or `cargo install --git https://github.com/saifulapm/amx` · [binaries](https://github.com/saifulapm/amx/releases)

amx needs tmux 3.2 or newer. Wire up the agents you use, then check the machine:

```sh
amx setup claude    # or pi, codex, opencode
amx doctor
```

## Use

```sh
amx new "fix the flaky login test"
amx new "add pagination to /api/orders"
amx                 # open the view
```

In the view, `space` opens an agent's card, `enter` attaches to its pane and `?` lists every key. [Quick start →](https://saifulapm.github.io/amx/docs/quick-start/)

## Docs

Everything is at [saifulapm.github.io/amx/docs](https://saifulapm.github.io/amx/docs/): [quick start](https://saifulapm.github.io/amx/docs/quick-start/) · [the view](https://saifulapm.github.io/amx/docs/view/) · [commands](https://saifulapm.github.io/amx/docs/commands/) · [scripting](https://saifulapm.github.io/amx/docs/scripting/) · [worktrees](https://saifulapm.github.io/amx/docs/worktrees/) · [agents](https://saifulapm.github.io/amx/docs/agents/) · [configuration](https://saifulapm.github.io/amx/docs/configuration/) · [how it works](https://saifulapm.github.io/amx/docs/how-it-works/)

## Development

```sh
git clone https://github.com/saifulapm/amx
cd amx
cargo build --release
cargo test          # the end-to-end suites drive real tmux servers
```

## License

amx is licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
