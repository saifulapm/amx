# Every screen opencode 2.0.16 draws that amx reads

Measured on 2026-09-30 against opencode v2.0.16, the day it was taken on as
amx's fourth vendor. `assets/screen-rules-opencode.toml` has six rules, and
`src/rules.rs` holds each of them against the captures in
`tests/opencode/screens` at 220, 100, 54 and 24 columns. This file is the
inventory those rules were written from: which screens exist, how each was
raised, what it reads as, and the screens that were not raised. It also
records the event feed and message lists captured in the same sitting, which
the plugin (Rulings 3 to 5) is written against.

opencode reports through amx's TUI plugin, so the pane is read where the
plugin cannot answer for the person: a pane with no provider to call, and the
permission card and question form a turn stops on.

## How this was read

Source says which screens exist and how to raise them: the tree at tag
v2.0.16 in `/tmp/vendors/opencode`, which is the code of the installed
binary, and the research at `~/.local/share/amx-research/opencode.md`. No
anchor came from it. Every anchor was read off a live pane.

The rig:

- `OPENCODE_CONFIG_DIR` a scratch directory holding copies of
  `~/.config/opencode/{opencode.json,tui.json,cli.json}` and its themes. The
  model is the one that file names, the free
  `opencode/longcat-2.5-preview-free`, with no login.
- `XDG_DATA_HOME`, `XDG_STATE_HOME` and `XDG_CACHE_HOME` scratch directories
  too, so the database, the recent-model list and the models cache were the
  rig's own. `~/.config/opencode` and `~/.local/share/opencode` were never
  written.
- A private tmux server (`tmux -L ocmeasure -f /dev/null`), one session, 40
  rows, started at 220 columns, in a scratch git repository.
- Every launch was `opencode --standalone`, per Ruling 1. Every prompt was
  typed into the composer and sent with Enter half a second later.
- A recorder TUI plugin at `<config>/plugins/rec/tui.js`, loaded unregistered
  the way Ruling 3's is. It wrote every event `ctx.data.listen` saw, which is
  the object `ctx.data.on` hands its handler, one per line, and at each
  `session.execution.succeeded`, `failed` or `interrupted` it awaited
  `ctx.data.session.message.sync(id)` and wrote `message.list(id)` whole, one
  message a line, as Ruling 5 has the amx plugin do.
- A capture was `capture-pane -p -J`, the call `src/tmux.rs` makes. Each
  screen was captured at 220, 100, 54 and 24 columns by resizing the window
  with the screen up, two seconds after each resize. The tests read each file
  the way amx reads a pane: trailing blank rows trimmed and sanitized.

## The alternate screen

opencode draws on the alternate screen (`#{alternate_on}` was 1 throughout),
so the pane is the whole frame, redrawn at every resize, with the composer
pinned near the bottom. Mouse capture is on. The home route draws the logo
and a centred composer; the session route draws the history above a full-width
composer. `OTUI_USE_ALTERNATE_SCREEN=0` was not tried.

## The screens

What each capture reads as is the rule the test asserts, at all four widths
unless the row says otherwise.

| Screen | How it was raised | Rule | Reads |
| --- | --- | --- | --- |
| Connect dialog | a message sent on the no-provider home | `connect` (setup) | waiting, question |
| No provider selected | `OPENCODE_MODELS_PATH` naming `{}` and `OPENCODE_DISABLE_MODELS_FETCH=1` | `no_provider` (setup) | waiting, question |
| Permission card | `{"permissions":[{"action":"shell","resource":"*","effect":"ask"}]}` in `OPENCODE_CONFIG_CONTENT`, then a prompt to `touch hello.txt` | `permission` | waiting, permission |
| Question form | a prompt asking the model to use its question tool for tea or coffee | `question` | waiting, question |
| A running turn | a prompt running `sleep 40` in the shell tool | `working` | working |
| A running turn after one Esc | the same, Esc once | `working` | working |
| Idle home, fresh | opencode started in the repository | `prompt` | idle |
| Idle after a turn | the working turn, finished | `prompt` | idle |
| Idle after Esc twice | a `sleep 60` turn, Esc, Esc 300 ms later | `prompt` | idle |
| Steered message pending (100 only) | Enter mid-turn | `working` | working |
| Provider error being retried (100 only) | the `opencode` provider's `baseURL` at a closed port | `working` | working |
| Turn ended on a provider error (100 only) | the `baseURL` at a local server answering 400 | `prompt` | idle |

### The no-provider screens

The free OpenCode Zen models count as a provider, so on this machine a fresh
opencode is never without one. Emptying the models catalog took them away.
The home route then draws `Build · No provider selected Connect a provider`
under the composer (`No provider se…` at 24 columns). A task passed with
`--prompt` would sit in the composer. (With a provider, the session route at
220 columns draws a sidebar `Getting started` card offering `/connect`; it is
a card, not a gate, and blocks nothing.)

Sending a message there started no turn and no session: it opened the
`Connect an integration` dialog, a searchable list under `Popular` (`OpenCode
Console`, `openai`, `github-copilot`) and `Services`. At 100 columns and wider
the title and `Search` are above the 24 rows a rule reads, so the rule stands
on `Popular` and the row under it. Both screens are `setup` gates: only the
person can connect a provider. Esc closes the dialog.

A model the catalog does not have (`{"model":"opencode/no-such-model-xyz"}`
in `OPENCODE_CONFIG_CONTENT`) raised nothing: the TUI fell back to the recent
model and the turn ran.

### The permission card

Drawn in the session in place of the composer: `△ Permission required`, the
command (`$ touch hello.txt`), then `Allow once   Always allow   Reject` and
`ctrl+f fullscreen  ⇆ select  enter confirm`, which moves to its own row at
54 and wraps at 24. Enter took `Allow once`. Right, Right, Enter took
`Reject`, with no second stage: the tool row read `The user declined this
tool call` and the turn ended as interrupted (below). The default build agent
ran `sleep`, `touch` and `rm` without asking, so the card needed the scratch
rule. Only the shell card was raised; edit, read and the other actions share
it by source and are not claimed.

### The question form

`Questions`, the question, the numbered options with their descriptions on
the row under each, a last `Type your own answer` opencode adds, then `↑↓
select  enter submit  esc dismiss`, which at 24 is set in three narrow columns
(`↑↓   enter esc` over `sele submi dismi`). It replaces the composer. Enter took
the highlighted option. Esc dismissed it and ended the turn as interrupted.
One question with two options was raised; several questions (`Field N of M`)
and multiselect were not.

### The working turn

The row under the composer is the footer. Mid-turn it opens with the
spinner, eight cells of `■` and `⬝`, then `esc interrupt`, then `↓ 1 shell`
while a shell command runs and `ctrl+p commands`. At 54 the spinner and the
words run together (`■■⬝⬝⬝⬝⬝⬝esc interrupt`); at 24 opencode cuts the words to
`esc...upt` with three full stops. One Esc turns the words into `esc again to
interrupt` for a few seconds (captured at 220 and 100; by the 54 capture it
had lapsed back), and at 24 that is `esc...upt` too.

`Press ctrl+b to move running work to the background` is drawn under a
running shell call. A message sent with Enter mid-turn is drawn at once as a
user row under the running tool call, with nothing to mark it pending. A
provider error being retried draws `⚠ Retrying in 9s · attempt 5 ·
ConnectionRefused: …` in the history under the same footer; the closed port
was still retrying at attempt 5.

### Idle, Esc and error

Idle on the home route is the logo, the composer with the placeholder `Ask
anything… "<an example>"` (the example changes per launch), `Build · <model>
<provider>`, the bar's closing row `╹▀▀▀…`, and a footer of the directory,
`shift+tab agents  ctrl+p commands`. At 24 the footer is the directory alone.
After a turn the placeholder is gone and the footer is the directory, the
context size (`7.1K (1%)`) and `ctrl+p commands`; at 54 and 24 the
directory is all that fits. The closing row is the one string on every idle
screen at every width, so the prompt rule stands on it.

Esc twice ended the turn: the shell call read `Tool execution interrupted`,
the `sleep 60` process was killed (`shell.deleted`, `shell.exited`), the turn's
summary line ended `· interrupted`, and the idle composer came back. A turn
that failed on a provider error drew `Error: <message>` and the idle composer.

### Not raised

- A login or `/connect` flow: the free model needed none.
- The update notice: the scratch `opencode.json` carries `autoupdate: false`.
- The version-mismatch preflight: every launch was `--standalone`.
- The permission card for anything but a shell command, and its `Always
  allow` stage.
- A form with several questions, or multiselect.
- An Esc with a steered message pending.
- `opencode mini`.

## The pane title

`#{pane_title}` as opencode set it, read beside the captures:

| Screen | Title |
| --- | --- |
| Home route | `OpenCode` |
| Session route, any state | `OC \| <the session's title>` (`OC \| Running sleep command`, `OC \| Creating hello.txt file`) |

The title names the session, not its state: the same string mid-turn, under
the permission card and at idle. It is no second signal.

## The event feed

`tests/opencode/events/` holds every event the recorder saw, one JSON object
per line, as `ctx.data.on` hands them over: `{id, created, type, data,
location?, durable?, metadata?}`. Ruling 3's plugin reads `e.data`.

- `turn.jsonl`: a fresh session, one turn running `sleep 40` in the shell
  tool and answering `done`.
- `steer.jsonl`: a `sleep 30` turn with a second message sent by Enter
  mid-turn.
- `interrupt.jsonl`: a `sleep 60` turn, Esc, then Esc twice.
- `permission.jsonl`: a shell call allowed once, then a second turn whose
  shell call was rejected.
- `question.jsonl`: a question answered, then a second one dismissed with Esc.
- `failure.jsonl`: a turn whose provider answered 400.

What they show:

- A fresh session is `session.created` with `{sessionID, slug, version,
  projectID, location, subpath, agent, model}`, then `session.inbox.enqueued`
  with the typed text in `item.payload.text`, then
  `session.execution.started`, then `session.inbox.delivered` for the same
  `inboxID`. The routed session changed from `{"type":"home"}` to
  `{"type":"session","sessionID":…}` (`ctx.ui.router.current()`) about the
  same moment.
- `session.execution.started` and one of `succeeded`, `failed` or
  `interrupted` bracket every turn, once each. In a turn that answered, the
  last `session.text.ended` (its `text` whole) comes before the terminal
  event.
- `session.tool.called` carries `{sessionID, assistantMessageID, id, input,
  executed}` and no tool name. The name (`shell`, `question`) is on
  `session.tool.input.started`, which shares the `id` and comes first. The
  question tool's input is `{questions: [{question, header, options:[{label,
  description}]}]}`.
- A steer is a second `session.inbox.enqueued` mid-turn and its
  `session.inbox.delivered` at the next step boundary, inside the same
  execution: no second `started`, one `succeeded` for both.
- `permission.asked` is `{id, sessionID, action: "shell", resources:
  ["touch hello.txt"], save: ["touch *"], source: {type: "tool", messageID,
  id}}`, with no `message`. `permission.replied` is `{sessionID, requestID,
  reply}`, `once` or `reject`.
- `form.created` is `{form: {id, sessionID, title: "Questions", metadata:
  {kind: "question", tool: {messageID, id}}, fields: [{key: "q0", title:
  <header>, description: <question>, type: "string", options: [{value,
  label, description}], custom: true}]}}`. `form.replied` carries `answer:
  {"q0": "Tea"}`; `form.cancelled` only `{id, sessionID}`.
- Esc twice gave `session.execution.interrupted` with `reason: "user"`.
  **A rejected permission and a dismissed form gave `reason: "shutdown"`**,
  the reason `core/src/session/execution.ts:53` defaults to when an interrupt
  names none. `execution.ts:131-137` keeps the execution claim for a
  `shutdown` interrupt, so such a turn is one the next service boot would
  resume, and its message list has no `idle` row (below).
- A provider error is `session.step.failed` then `session.execution.failed`,
  both with `error: {type: "provider.invalid-request", message, status:
  400}`. A refused connection was retried instead (`session.retry.scheduled`
  per attempt) and never failed while watched.
- `session.viewed` followed each terminal event but the two `shutdown`
  interrupts, which end their files; `session.usage.updated`,
  `session.renamed`, `shell.*` and `project.updated` come and go mid-turn.
  `session.text.delta` and `session.reasoning.delta` are in the files as
  they came.

## The message lists

`tests/opencode/messages/<scenario>.jsonl` is what the recorder wrote at the
scenario's last terminal event: `message.list(id)` of that session, whole,
one message a line, the file Ruling 5 has the plugin write to
`$AMX_DIR/opencode-messages.jsonl`. Each line is `{id, type, …}`:

- `user`: `{text, files, agents, time}`.
- `assistant`: `{agent, model: {id, providerID, variant}, content: [text,
  reasoning and tool items], finish, rawFinish, error, tokens, cost, snapshot,
  time}`. `finish` is `tool-calls` for a step that called a tool and `stop`
  for the answer.
- `idle`: `{outcome, time}` closing a turn, `succeeded`, `failed` or
  `interrupted`.

An Esc'd turn ends in an assistant with `finish: "error"`, `error.type:
"aborted"`, and an `idle` of `interrupted`. A failed one ends in an assistant
with `error.type: "provider.invalid-request"` and an `idle` of `failed`. A
turn ended by a rejected permission or a dismissed form ends in the aborted
assistant **with no `idle` row after it** (`permission.jsonl`,
`question.jsonl`), so a reader of the list cannot take the last `idle` as the
end of the last turn.

## What differs from the plan

- Ruling 4's Refused is coined from a `reject` reply or `form.cancelled`, and
  both of those also end the turn: `session.execution.interrupted` follows at
  once, with `reason: "shutdown"`. Ended has to fire for it like any other
  interrupt.
- That `shutdown` keeps the execution claim (`execution.ts:131-137`), the
  same way Ruling 6 says a killed server's does, so a turn ended by a
  rejection or a dismissed question is left for the next service boot to
  resume. Not measured past the event.
- Calling (`session.tool.called`) needs the name from the
  `session.tool.input.started` before it.
- A steer shows no pending mark on the screen: it is drawn as a user row the
  moment it is sent.
