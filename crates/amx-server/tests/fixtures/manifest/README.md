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

The five `claude-blocked-*` files other than `claude-blocked-permission.txt`
were recorded the same way on **2026-08-09** from **Claude Code 2.1.226**, the
version the M4 exit smoke drove, after that smoke found the permission rule
matching one dialog class in six (`docs/notes/m4-live-smoke.md` §6.8). One
dialog class per file, driven by asking a real session for the tool that raises
it; the credentials were copied into a scratch `CLAUDE_CONFIG_DIR` and `HOME`,
so nothing about the run touched the operator's own configuration.

| File | The transition it is |
|---|---|
| `claude-idle-prompt.txt` | at the prompt, composer empty, its placeholder hint showing |
| `claude-idle-after-interrupt.txt` | a permission dialog answered "No" — `Interrupted · What should Claude do instead?`, prompt box back (V01 §7 case 3) |
| `claude-working-stream.txt` | mid-turn, answer text streaming, `esc to interrupt` in the footer |
| `claude-working-spinner.txt` | mid-turn, before any answer text: `· Percolating… (2s · thinking)` |
| `claude-blocked-permission.txt` | the permission dialog up and unanswered (V01 §7 case 3, before the answer) |
| `claude-blocked-write-create.txt` | a `Write` to a new file: `Do you want to create exit-probe.txt?` |
| `claude-blocked-write-overwrite.txt` | a `Write` over an existing file: `Do you want to overwrite exit-probe.txt?` — the dialog the M4 exit smoke sat in front of for 35 s |
| `claude-blocked-edit.txt` | an `Edit`: `Do you want to make this edit to exit-probe.txt?` |
| `claude-blocked-fetch.txt` | a `WebFetch`: `Do you want to allow Claude to fetch this content?`, and no `Esc to cancel` footer under it |
| `claude-blocked-plan.txt` | a plan waiting for approval: `Claude has written up a plan and is ready to execute. Would you like to proceed?`, whose answers are three yeses and a "tell Claude what to change" |
| `codex-idle-prompt.txt` | at the composer, nothing running |
| `codex-idle-after-interrupt.txt` | an approval cancelled with Esc — `■ Conversation interrupted` (V01 §7 case 13) |
| `codex-working.txt` | mid-turn: `• Working (0s • esc to interrupt)`, with a previous turn's interrupt marker still in the transcript above it |
| `codex-blocked-approval.txt` | the approval dialog up: `Would you like to run the following command?` |

Two window titles were recorded alongside and live in `tests/manifest.rs` rather
than here, since a title is one line: a braille spinner (`⠂ …`, `⠋ scratch`)
while a turn runs, and no spinner at rest (`✳ …`, `scratch`). Both agents paint
their turn into the title before the grid says anything about it, which is why
both manifests carry a title rule.

A third title reading was taken on 2026-08-09 and it is why the blocked
fixtures need no title of their own: with a permission dialog up and the tool
call still outstanding, Claude Code's title reads `✳ Create exit-probe.txt with
hello` — at rest, no braille. The title rule outranks the dialog rule, so a
spinner there would have made every dialog read `working`; it does not.

## Re-recording them

Agent CLIs ship weekly, so these rot. Re-record by running the agent in a
terminal you can read back — `tmux new-session -d -x 100 -y 30 <agent>`, drive
it to the state you want, `tmux capture-pane -p` — and replace the file whole.
If a rule needs changing to match, change the rule and say in its comment what
the UI did, the way the current comments do.

**Re-record the whole blocked set, not one file.** The gap the M4 exit found was
not a rule that stopped matching; it was a rule tested against one dialog while
five others existed, so the suite was green and the feature was blind. The
dialogs to drive, and a prompt that raises each on a scratch project:

| Dialog | Ask the session for |
|---|---|
| write, new file | `Use the Write tool to create exit-probe.txt containing the single word hello.` |
| write, existing | the same again, with different content |
| edit | `Use the Edit tool to change the word goodbye in exit-probe.txt to farewell.` |
| bash | `Run the bash command: touch y02-bash-probe.txt` — `echo` is allowed without asking |
| fetch | `Use the WebFetch tool on https://example.com/ and tell me its title.` |
| plan | shift+tab to plan mode, then any question that ends in a plan |

Answer each with `3` or `Esc` rather than `1` if you would rather the run left
nothing behind. Type the prompt and submit it as a separate keystroke: DR-3's
single-`write()` swallow ate the submit on 3 of 3 attempts against a real
composer during the M4 exit.
