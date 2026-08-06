# amx

A minimal, keyboard-only terminal multiplexer for coding agents: a background
server owns your terminals, so agents survive disconnects and client crashes —
and the whole interface is panes plus one status line.

Early days: this is the M0 milestone — a daily-drivable multiplexer core.
Agent-native features (status detection, attention queue, session resume)
arrive in the next milestones. The full design lives in [docs/](docs/).

## Build

Linux and macOS. Needs stable Rust (pinned via `rust-toolchain.toml`) and
Zig 0.15.2 for the vendored [libghostty-vt](https://github.com/ghostty-org/ghostty)
terminal core — `scripts/vendor-libghostty-vt.sh toolchain` fetches the pinned
Zig into `vendor/toolchain/` if you don't have it.

```sh
cargo build --release
./target/release/amx
```

## Use

- `amx` — attach; starts the session server if none is running
- `ctrl+a` — prefix: `w` navigate (hjkl focus, HJKL resize, x/v split,
  s+direction swap, m move, d close, digits jump), `d` detach, `z` zoom
- `amx session list|attach|stop|delete` — manage named sessions (`--session`)
- `amx attach --pane <id>` — one pane, full screen, no chrome

Kill your terminal any time; `amx` brings everything back.

## License

MIT or Apache-2.0, at your option.
