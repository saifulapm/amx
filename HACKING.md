# Hacking on amx

A minimal, keyboard-only agent terminal multiplexer in Rust. The complete
design lives in `docs/` — read `docs/README.md` first, then the doc relevant
to your change. `docs/04-architecture.md` is the binding architecture;
`docs/05-roadmap.md` is the build order. Do not contradict a decision in the
D1–D13 table without raising it first.

The design docs study herdr's mechanisms in depth (PTY actor, handoff,
detection manifests). If you keep a herdr checkout at `herdr/` (gitignored)
for reference, treat it as reference only: herdr is Apache-2.0 and amx is an
independent implementation. Learn from its mechanisms; never copy its lines.

## Hard rules

- **Never guess.** Verify behavior against source (the vendored headers,
  crate docs via docs.rs, reference implementations). If a claim can't be
  verified, say so in the PR instead of inventing.
- Write commit messages terse and imperative: `feat(pty): ordered query
  replies`, body only when the why isn't obvious.
- **Module budget**: soft 500 lines, hard 1000 per file (generated code
  exempt). If a file wants to grow past it, split by responsibility.
- **Tests**: integration tests in `tests/` drive the public surface; inline
  `#[test]` only for pure helpers. Every task lands with tests that fail
  without the change. `cargo test && cargo clippy --all-targets -- -D warnings
  && cargo fmt --check` must pass before you finish.
- **Rust**: edition 2024, stable toolchain. No `unwrap()`/`expect()` outside
  tests except on invariants with a comment stating the invariant. Typed
  errors (`thiserror`) in libraries; `anyhow` only in the binary. No new
  dependencies without a one-line justification in the commit body — prefer
  std, keep the tree lean (tokio, serde, thiserror are assumed).
- **Performance**: no per-frame allocations on hot paths (PTY read, damage
  encode, event dispatch); measure before optimizing beyond that.
- Commit in small logical units on a branch. Never commit straight to `main`.

## Working in parallel

Changes land as focused branches with a stated scope. Stay inside the scope
of the change you picked up; if you need an edit outside it, raise it in the
PR description instead of making it. Every PR states what was built, what was
verified (test output), and what was left open.
