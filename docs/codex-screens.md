# Every screen codex 0.157.1 draws that amx reads

Measured on 2026-09-28 against codex-cli 0.157.1, the day it was taken on as
amx's third vendor. `assets/screen-rules-codex.toml` has nine rules, and
`src/rules.rs` holds each of them against the captures in
`tests/codex/screens` at 220, 100, 54 and 24 columns. This file is the
inventory those rules were written from: which screens exist, how each was
raised, what it reads as, and the screens that were not raised. It also
records the hook payloads, rollouts and trust hashes captured in the same
sitting, and the answers to the open questions in the source reading
(`~/.local/share/amx-research/codex.md`).

codex reports through hooks (SessionStart, UserPromptSubmit, PreToolUse,
Stop), so the pane is read where they go quiet: the gates in front of a
session, an Esc'd or errored turn (codex sends no Stop for either), and the
approval box, which PermissionRequest fires ahead of and which a reviewer may
answer with no box at all.

## How this was read

Source says which screens exist and how to raise them: the tree at tag
rust-v0.157.1 in `/tmp/vendors/codex`, which is the code of the installed
binary. No anchor came from it. Every anchor was read off a live pane.

The rig:

- `CODEX_HOME` a scratch directory, holding copies of `~/.codex/auth.json`
  and `~/.codex/config.toml`. `~/.codex` was never written.
- A private tmux server (`tmux -L codexmeasure -f /dev/null`), one session,
  40 rows, started at 220 columns.
- Three scratch git repositories under the same scratch directory: two
  trusted in the scratch config.toml, one not.
- Every launch was `codex --no-daemon`, per Ruling 1.
- A capture was `capture-pane -p -J`, the call `src/tmux.rs` makes. Each
  screen was captured at 220, 100, 54 and 24 columns by resizing the window
  with the screen up, two seconds after each resize. The tests read each
  file the way amx reads a pane: trailing blank rows trimmed (tmux's output
  is trimmed in `Server::run`) and sanitized.
- The agent was the account's default, GPT-6-Luna, at default effort.
  Every prompt was a throwaway written for the measurement.

## The alternate screen

codex draws on the alternate screen by default (`#{alternate_on}` was 1). The
pane is then the whole frame, redrawn cleanly at every resize, with the
composer pinned to the bottom and blank rows between it and the history
until the history fills the pane.

With `--no-alt-screen` (`#{alternate_on}` 0) finished history goes into tmux's
scrollback and the composer is drawn inline under it, floating up the pane.
Measured at idle only (`idle-no-alt-screen-100.txt`): the footer had no `?
for shortcuts` row, and a `WARNING: proceeding, even though we could not
create PATH aliases` line from the scratch CODEX_HOME sat in the scrollback.

amx never reads scrollback (every capture is of the visible pane), so the
flag buys amx nothing and would change the footer the idle rule stands on.
**amx should launch codex without `--no-alt-screen`.** Every rule was
measured on the alternate screen. This matches the Spec's `launch
["--no-daemon"]`.

## The screens

What each capture reads as is the rule the test asserts, at all four widths
unless the row says otherwise.

| Screen | How it was raised | Rule | Reads |
| --- | --- | --- | --- |
| Update prompt | `version.json` in CODEX_HOME naming 0.158.0, codex run from a standalone install under that CODEX_HOME, no prompt on argv | `update` | waiting, question |
| Hooks need review | a hooks.json with eleven untrusted handlers | `hooks_review` (setup) | waiting, question |
| Folder access (trust) | codex started in the untrusted repository | `folder_trust` (setup) | waiting, trust |
| Working directory · resume | `codex --no-daemon resume <id>` from a directory other than the session's | `cwd_prompt` (setup) | waiting, question |
| Command approval | `--sandbox read-only` and a prompt asking for escalated permissions for `touch hello.txt` | `approval` | waiting, permission |
| request_user_input | `--enable default_mode_request_user_input` and a prompt asking for a tea-or-coffee question | `question` | waiting, question |
| A running turn | a prompt running `sleep 45` in the shell | `working` | working |
| Idle composer, fresh | codex started in a trusted repository | `prompt` | idle |
| Idle composer, after a turn | the working turn, finished | `prompt` | idle |
| Esc'd turn, queued message restored | a Tab-queued message, then Esc | `interrupted` | idle |
| Steered message pending (100 only) | Enter mid-turn | `working` | working |
| Tab-queued message (100 only) | Tab mid-turn | `working` | working |
| Turn ended on an API error (100 only) | `-m no-such-model-xyz` | `prompt` | idle |

### The update prompt

codex offers the blocking prompt only when it can tell how it was installed:
with a standalone install it looks for its own executable under
`$CODEX_HOME/packages/standalone/releases`. Run from the scratch CODEX_HOME
with the PATH `codex` (whose release lives under `~/.codex`), it printed an
`✨ Update available! 0.157.1 -> 0.158.0` banner into the history instead,
which blocks nothing. Copying the release directory into the scratch
CODEX_HOME and running that copy raised the prompt. On Saiful's machine,
where the binary is under `~/.codex`, codex would draw the prompt.

codex skips it when a prompt is on the argv, so `amx new` with a task never
meets it. A new with no task, or a resume with nothing after the id, can.
Ruling 7 names three setup gates and this is not one of them, so the rule is
a plain waiting question. `enter continue · esc skip`; Esc skips.

### The hooks review

`Hooks need review`, a count line, `Hooks can run outside the sandbox after
you trust them.`, then `1. Review hooks`, `2. Trust all and continue`, `3.
Continue without trusting (hooks won't run)`. The count line is `11 hooks
are new or changed.` or `1 hook is new or changed.`, so it is no anchor.
Down then Enter on `Trust all and continue` trusted all eleven in one step,
with no second confirmation, and wrote the hashes (below).
`--dangerously-bypass-hook-trust` took the screen down for that run, and the
next run without it drew the screen again for the one untrusted handler.

### The folder trust screen

`Folder access`, the path, the paragraph opening `Trust this folder?`, then `1.
Trust and continue` / `2. Quit`, and `enter continue · esc quit`. The pane
title was the host name here, not a codex title. `-c
'projects."<dir>".trust_level="trusted"'` on the argv did not take the screen
down. A trust entry in config.toml does.

### The working directory prompt

`Working directory · resume`, `Session = latest cwd recorded in the resumed
session`, `Current = your current working directory`, then four numbered
choices (use session, use current, always use session, always use current)
and `enter continue · esc use session · ctrl+c quit`. At 24 columns the
screen is 26 rows, so the title is above the 24 rows a rule sees. The rule
stands on `session = latest` for that reason. The fork variant was not raised.

### The approval box

Drawn at the bottom of the pane with no composer or footer under it. The
title `Would you like to run the following command?`, `Environment: local`,
`Reason: <the model's justification>`, `$ <command>`, then `1. Yes, proceed
(y)`, `2. Yes, and don't ask again for commands that start with
`<command>` (p)`, `3. No, and tell Codex what to do differently (esc)` and
`Press enter to confirm or esc to cancel`. `y` approved it.

A plain `--sandbox read-only` with "create a file" did not raise a box: the
model tried, the write failed, and it said so. It took a prompt asking for
escalated permissions. `-c approval_policy=untrusted` is refused at startup:
`approval_policy = "untrusted" is no longer supported`. The edit, network,
permissions and MCP boxes were not raised and are not claimed.

### The question

`Question 1/1 (1 unanswered)`, the question, the options with their
descriptions in a second column and a third `None of the above` codex adds,
then `tab to add notes | enter to submit answer | esc to interrupt`, which
wraps to two rows at 54 and three at 24. Enter took the highlighted choice.
Plan mode was not used; the feature flag put the tool in default mode.

### The working turn

The status row `• Working (7s • esc to interrupt)` sits three rows above the
composer. codex appends to it after the bracket: `· 1 background terminal running · /ps to view · /stop to
close`. At 24 columns the row is cut to `• Working (12s • esc to…`, which is
why the rule takes `• esc to` beside `to interrupt)`. Past one minute the
source formats the elapsed time as `1m 05s`, which would cut a 24-column row
before `esc to` and quiet the rule there. No turn that long was captured at
24.

The footer under the composer during a turn is the idle footer, `? for
shortcuts`, and with text staged it is `tab to queue message`.

A steered message (Enter mid-turn) is drawn under the status row as `•
Messages to be submitted after next tool call (press esc to interrupt and
send immediately)` with `↳ <message>`. A Tab-queued one is `• Queued
follow-up inputs`, `↳ <message>` and `shift+← edit last queued message`.

### Idle, Esc and error

Idle is `› Ask Codex to do anything`, a blank row, the status line
(`GPT-6-Luna default · <cwd>`, and after a turn ` · <what the last turn
did>`), and `? for shortcuts`. A `Tip:` row sometimes sits above the
composer. After the question turn, a `⚠ 1 warning · f2 to view` was drawn at
the right end of the footer row.

With text staged in the composer at idle, the footer row is empty. Esc at
idle puts `esc again to edit previous message` there for a moment. The
prompt rule holds on neither.

Esc during a turn drew `■ Conversation interrupted - tell the model what to
do differently. Something went wrong? Hit /feedback to report the issue.`
and put the Tab-queued message back into the composer, unsent. The shell
command the turn had started (`sleep 60`) kept running: the next row was `1
background terminal running · /ps to view · /stop to close`. The rule
`interrupted` reads this screen as idle, because the restored text empties
the footer row.

A turn that failed on an API error drew `■ {"type":"error","status":400,…}`
and then the idle composer.

### Not raised

- Welcome and login (`Sign in with ChatGPT`…): the scratch CODEX_HOME carried
  auth.json.
- The model migration screen (`Codex just got an upgrade`).
- The daemon recovery screen: every launch was `--no-daemon`.
- The fork variant of the working directory prompt.
- The edit, network, permissions, terminal-input and MCP approval boxes.
- A question with more than one question, or in plan mode.
- The subdirectory note on the trust screen.

## The pane title

`#{pane_title}` as codex set it, read beside the captures:

| Screen | Title |
| --- | --- |
| Folder trust, hooks review, cwd prompt | the host name (codex has not set one yet) |
| Fresh idle | `trusted` (the project name) |
| Working | `⠙ Run shell command \| trusted` (a braille frame, the activity, the project) |
| Approval | `[ ! ] Action Required \| Request permission for touch \| trusted` |
| Question | `[ . ] Action Required \| Ask tea or coffee \| trusted` |
| Idle after a turn | `Run shell command \| trusted` (the last activity, no frame) |

So a leading braille frame means working and `Action Required` means waiting,
but the title says nothing during the startup gates and keeps a stale
activity at idle. It is a usable second signal, not a replacement for the
rules.

## Hook payloads

`tests/codex/hooks/` holds every payload the scratch hooks.json recorded, one
JSON object per line, per event: SessionStart (source `startup` and
`resume`), UserPromptSubmit (a first prompt and a steered one), PreToolUse
(`Bash` and `request_user_input`), PermissionRequest, Stop, Interrupt and
SessionEnd. `SessionStart.env` is the `AMX_*` and `CODEX_*` environment the
SessionStart handler saw.

What they show:

- `permission_mode` was `"default"` throughout. `model` is the slug
  (`gpt-6-luna`).
- A shell call is `tool_name: "Bash"` with `tool_input.command`, and a
  `tool_use_id` like `exec-…`. The question tool is `request_user_input`
  with `tool_input.questions[]` of `{header, id, question, options[{label,
  description}]}`.
- PermissionRequest carries `tool_input.description`, the model's reason,
  beside the command.
- A steered message is a second UserPromptSubmit with the running turn's
  `turn_id`.
- Stop carries `last_assistant_message` and `stop_hook_active`, and no stop
  reason. It fired for every turn that completed, and for neither the Esc'd
  turns nor the errored one.
- Interrupt carries `turn_id`, `model`, `permission_mode`, and no reason. Its
  default timeout is 1 second (`hooks/list` reports `timeoutSec: 1`).
- SessionStart fired at a session's first turn, never at launch: of two
  resumes of the same session, the one with no turn fired nothing.
- SessionEnd (`reason: "other"`) fired on `/quit`.
- The hook environment carried the pane's `AMX_ID` and `AMX_NESTED` under
  `--no-daemon`, and no `CODEX_SESSION_ID` or `CODEX_THREAD_ID`.

## Rollouts

`tests/codex/rollouts/`, copied out of the scratch `sessions/` with the two
account identifiers in `session_meta` (`creator_user_id`,
`creator_account_id`) replaced and nothing else touched:

- `turn-steer-abort-kill-resume.jsonl`: a turn with a shell call and a
  steered message ending in `task_complete`, a turn Esc'd mid-command ending
  in `turn_aborted` with `reason: "interrupted"`, a turn whose pane was
  killed (a `task_started` and nothing after it), then a resume and a
  one-word turn.
- `aborted-first-turn.jsonl`: a session whose only turn was Esc'd.
- `approval.jsonl`: an approved escalated command.
- `question.jsonl`: a request_user_input turn; its `task_complete` has a
  null `last_agent_message`.
- `error.jsonl`: a turn that failed on an unsupported model; its
  `task_complete` has an `error` object and a null `last_agent_message`.

All are `history_mode: "paginated"` with `originator: "codex-tui"`.

## Hook trust: the oracle

`tests/codex/trust/` holds:

- `hooks.json`, the scratch hooks file: a recorder group for each of seven
  events, then for SessionStart, UserPromptSubmit, PreToolUse and Stop a
  second group that is exactly amx's (`{"hooks": [{"type": "command",
  "command": "amx _hook"}]}`, no matcher, timeout or async).
- `hooks-list.json`, the `codex app-server` answer to `hooks/list` for that
  file before anything was trusted. Each entry has its `key` and
  `currentHash`.
- `config.toml`, the `[hooks.state]` tables codex wrote into the scratch
  config.toml when `Trust all and continue` was chosen, verbatim. codex
  wrote an empty `[hooks.state]` table followed by one
  `[hooks.state."<key>"]` table per handler, each with only `trusted_hash`.

Each `trusted_hash` equals the `currentHash` for its key. amx's four groups
sit at index 1:

| Key suffix | trusted_hash |
| --- | --- |
| `session_start:1:0` | `sha256:4add63b3f2cf92a907d37f2e9b7b5d7972234d4357db8dd35851c32c02603c53` |
| `user_prompt_submit:1:0` | `sha256:ddebddeb874f57494d55880d6f70adbee7136bb60988e2a9616f5d59ab304738` |
| `pre_tool_use:1:0` | `sha256:62ca2d9301c902140298733d9e0be308345dda945b627cb0385a231adebdc012` |
| `stop:1:0` | `sha256:4de4283a553cfcda6ea020eec3ae1d66555d7f3bce089906e6723de26975e6d7` |

All four are the sha256 of `{"event_name":"<event_snake>","hooks":[{"async":false,"command":"amx _hook","timeout":600,"type":"command"}]}`,
Ruling 2's recipe, checked with `sha256sum`.

## The open questions

The numbering is the source reading's.

1. **Anchors and the pane title.** The anchors the source proposed hold under
   tmux on the alternate screen, except `to interrupt)` at 24 columns (cut;
   `• esc to` stands in for the first minute) and the cwd prompt's title at
   24 (above the floor). `#{pane_title}` works as a second signal (braille
   frame means working, `Action Required` means waiting) but is the host
   name during the startup gates and stale at idle.
2. **Hook trust and the session id.** `--dangerously-bypass-hook-trust` took
   the review screen down for that run (no turn was driven under it). The
   SessionStart `session_id` equalled `CODEX_SESSION_ID` and
   `CODEX_THREAD_ID` in a tool shell (`01a0e495-b6aa-7022-ba93-84d2107c2d0e`
   all three).
3. **Per-run folder trust.** No. `-c 'projects."<dir>".trust_level="trusted"'`
   still drew the trust screen.
4. **What `send` produces.** Text typed with `send-keys -l` and Enter sent
   immediately after did not submit: the text stayed staged (the paste-burst
   window). A second Enter half a second later submitted it. A typed `$` opens
   the skill and app list (`enter insert · esc close`) and a typed `@` the
   file list (`enter/tab insert · esc close`), so Enter after either inserts a
   selection instead of submitting. A steer (Enter mid-turn) is a second
   UserPromptSubmit with the same `turn_id`.
5. **Esc.** Tab-queued messages go back into the composer, unsent. The history
   gets `■ Conversation interrupted - …`. A shell command the turn started
   keeps running as a background terminal. The Interrupt hook fires with
   `turn_id` and no reason, and the rollout gets `turn_aborted` with `reason:
   "interrupted"`. A pending steer at Esc was not measured.
6. **Killing the pane under `--no-daemon`.** The codex process ended with the
   pane. No SessionEnd, Interrupt or Stop fired, and the rollout was left with
   a `task_started` and no `task_complete` or `turn_aborted` for that turn,
   which a later resume did not add. `codex --no-daemon resume <id>` worked
   afterwards and replayed that turn as interrupted.
7. **Argv.** `codex --no-daemon resume <id> -- '<prompt>'` resumed and ran the
   prompt. `codex --no-daemon -- resume` started a new session with the
   prompt `resume`. `codex --no-daemon -- -v` sent `-v` as a prompt, and
   without the `--` clap refused it.

## What differs from the plan

- The Spec sets `restores_queued_on_cancel` false. On 0.157.1 Esc put a
  Tab-queued message back into the composer, unsent. That is the true
  behaviour for Tab-queued messages. The source says a steer still pending at
  Esc is resubmitted as a new turn instead; that was not measured.
- Esc does not stop a shell command the turn started. It keeps running as a
  background terminal after the turn is aborted.
- A killed pane leaves its turn open in the rollout for good, with no
  `turn_aborted`, so a rollout reader has to treat a trailing `task_started`
  with nothing after it as ended.
