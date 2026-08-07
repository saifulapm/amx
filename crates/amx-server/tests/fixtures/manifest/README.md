# Recorded agent screens

What the shipped manifests are tested against. Each file is one visible grid,
exactly as the agent painted it — no editing, no reflow — so a rule that stops
matching the real UI fails `tests/manifest.rs` instead of a user's status line.

## Provenance

Captured on **2026-08-07**, on the machine that ran the V01 spike (Arch Linux,
x86_64), from **Claude Code 2.1.224** and **Codex CLI 0.147.0** — the versions
`docs/notes/hook-coverage.md` measured. Each agent ran in a 100×30 terminal in a
throwaway project directory, with the launching environment cleared of the
enclosing agent's variables so nothing about the capture was inherited state.
The grid was read back with `tmux capture-pane -p`, which gives the visible
screen rather than a paint log — the distinction matters, because the manifest
engine reads the live grid and only the grid.

The spike's own `dumps/` are paint logs and its runs are disposable, so these
were recorded fresh rather than salvaged. They correspond one-to-one to
transitions §7 of the findings names.

| File | The transition it is |
|---|---|
| `claude-idle-prompt.txt` | at the prompt, composer empty, its placeholder hint showing |
| `claude-idle-after-interrupt.txt` | a permission dialog answered "No" — `Interrupted · What should Claude do instead?`, prompt box back (V01 §7 case 3) |
| `claude-working-stream.txt` | mid-turn, answer text streaming, `esc to interrupt` in the footer |
| `claude-working-spinner.txt` | mid-turn, before any answer text: `· Percolating… (2s · thinking)` |
| `claude-blocked-permission.txt` | the permission dialog up and unanswered (V01 §7 case 3, before the answer) |
| `codex-idle-prompt.txt` | at the composer, nothing running |
| `codex-idle-after-interrupt.txt` | an approval cancelled with Esc — `■ Conversation interrupted` (V01 §7 case 13) |
| `codex-working.txt` | mid-turn: `• Working (0s • esc to interrupt)`, with a previous turn's interrupt marker still in the transcript above it |
| `codex-blocked-approval.txt` | the approval dialog up: `Would you like to run the following command?` |

Two window titles were recorded alongside and live in `tests/manifest.rs` rather
than here, since a title is one line: a braille spinner (`⠂ …`, `⠋ scratch`)
while a turn runs, and no spinner at rest (`✳ …`, `scratch`). Both agents paint
their turn into the title before the grid says anything about it, which is why
both manifests carry a title rule.

## Re-recording them

Agent CLIs ship weekly, so these rot. Re-record by running the agent in a
terminal you can read back — `tmux new-session -d -x 100 -y 30 <agent>`, drive
it to the state you want, `tmux capture-pane -p` — and replace the file whole.
If a rule needs changing to match, change the rule and say in its comment what
the UI did, the way the current comments do.
