# Vendors

amx runs coding agents it did not write, and everything it knows about one is
an entry in a table: `src/vendor/`. This file says what an entry carries, what
the rest of amx asks of it, and what adding one takes. pi is the second real
vendor to land, and it cost more than the recipe at the end asks of a third:
one new file, `src/vendor/pi.rs`, and beside it the machinery pi needed that
the table did not have. `resume` and `fork` had claude's session flags written
into them by hand, so those became a [`session`](#what-an-entry-carries)
vocabulary in the table and both verbs were rewritten to read it; `spawn`
learned to hand a vendor the id amx had already minted, because a vendor
reporting through no hooks names no session of its own; and `rules` grew a
per-vendor door for a second screens document. What is left for a third vendor
is what the recipe describes: the measurement, the entry, and the laws that
already hold both.

codex is that third vendor, and it tested the claim. Its entry is one file,
`src/vendor/codex.rs`, and no verb learned codex's name. The table still grew
seven things for it, each a shape neither claude nor pi had: a dial spelled as
a setting (`DialSpec.key`), words every process starts with (`launch`), a fork
that is a subcommand (`ForkSpec::Subcommand`), a model list printed as JSON
(`Models::Json`), a third transcript shape (`Transcript::Codex`), a hooks file
the vendor shares with the person (`Wire::Hooks`), and a catalog opened by
something other than `/` (`Catalog.sigil`). The recipe holds, with the caveat
that a third shape of program brings shapes of its own.

opencode is the fourth, again one file, `src/vendor/opencode.rs`. Its new
shapes were a model carried in the environment (`DialSpec.env`), a task that
is one flag's value (`prompt_flag`), a plugin file amx places in the vendor's
config directory (`Wire::Placed`), a signal the plugin hears to end a turn
before its pane goes (`interrupt_signal`), composer characters that open a
popup (`popups`), a cancel that takes two presses (`cancel_presses`), and a
message list the plugin writes (`Transcript::Opencode`).

## The shape: a descriptor, not a trait

A vendor is a `Vendor` value in a static table — no dynamic dispatch, no
second implementation of anything, and no place for a vendor to hide
behaviour. What one declares is data a person can read in one sitting and
diff against the vendor's own `--help`. Where a field would want a function
is where to think again, and not before.

Three real entries exist today: `claude`, in `src/vendor/claude.rs`, `pi`, in
`src/vendor/pi.rs`, and `codex`, in `src/vendor/codex.rs` — see [pi](#pi) and
[codex](#codex) for what each can do and what it cannot. A test-only vendor,
`src/vendor/second.rs`, is nobody's agent and is not in the table; it exists so
every law about the table is proved against a shape that is none of the real
ones. It answers most questions the other way — resumable but not forkable, no
hooks, no transcript, different flags for the same dials — which is what keeps
the machinery honest against a shape no real entry happens to take.

## What an entry carries

- **`name`** — the program, which is what the table is keyed by. `agent` in
  the config is a command line; the entry is found by the program it runs, so
  `agent = "claude --add-dir .."` still resolves.
- **The dials** — `model`, `permission`, `effort`, each an optional
  `DialSpec`: the values a cycle key offers, whether values off the cycle are
  legal, and the flag the vendor spells it with. The one place a dial becomes
  vendor argv is `inject`, and a flag the caller already wrote wins by the
  dial standing down. `key` is for a vendor with no flag for the dial, only a
  setting: the dial is written `<flag> <key>=<value>`, which is codex's
  `-c model_reasoning_effort=high`, and it stands down only for a `-c` that
  already names its own key, since a `-c` for anything else says nothing
  about this dial. `bare` is for a dial whose flag is the whole of it: a
  closed dial with one value beside the sentinel, written as the flag alone,
  opencode's `--auto`. `env` is for a model the vendor takes only from a
  variable: `inject` writes no argv for it, its `flag` is empty, and
  `vendor::env_dials` gives the pair for a new pane's environment, the
  variable and `{"<key>":"<value>"}`, standing down when the environment
  already carries that variable. opencode's model goes into
  `OPENCODE_CONFIG_CONTENT` as `{"model":"<value>"}`, on `new` only, since a
  resumed session keeps its own model.
- **`models`** — where this vendor's models are written down, so that a model
  somebody named can be looked for without anything knowing a vendor's name.
  `Models::Cycle` says the model dial's own cycle is the whole list, which is
  claude's four aliases and the second vendor's two words; `Models::Printed`
  carries the argv that asks the vendor to print its own, which is pi's
  `--list-models`, since pi's models are whatever its providers hold.
  `Models::Json` carries the argv that prints them as JSON: codex's `debug
  models`, of whose `models[]` amx takes the `slug` of each one whose
  `visibility` is `list`, the ones codex's own picker offers. A listing costs a
  process, so when one is worth running is the reader's
  business and never the entry's.
- **`session`** — a `SessionSpec`, the flags that decide which session a
  process opens, or `None` from a vendor amx has measured no session
  vocabulary for. `start` is the flag that opens a session under an id amx
  minted, and is `None` from a vendor whose own report already names the
  session it opened, the way claude's Started hook does. `resume` says how one
  is carried on: `Resume::Flag { flag, joined }`, where `joined` says whether
  the id rides onto the flag with `=` rather than standing as a word of its
  own, or `Resume::Subcommand(word)`, a word right after the program and then
  the id, the way `codex resume <id>` is spelled. `resume` and `fork` both
  build those words through `SessionSpec::resume_args`, and a law holds that
  whatever it writes, `names_a_session` reads back. `conflicts` lists every other
  flag that also claims to say which session is open, so a resume or a fork
  replaces it instead of leaving two words that disagree. `fork` says how this
  vendor branches a session into a copy, for a vendor that claims `Fork`: a
  bare marker written beside `resume` (claude's `--fork-session`), a flag
  naming the origin (pi's `--fork`), or `ForkSpec::Subcommand(word)`, a word
  right after the program and then the origin's id, `codex fork <id>`, which is
  not a resume and never carries the resume words beside it. `new`, `resume`
  and `fork` build their session argv out of this and nothing else.
- **`launch`** — words every process of this vendor is started with, whatever
  amx starts it for. `new` writes them right after the program, once, and
  `resume` and `fork` keep them where they stand, since every resume and fork
  argv descends from the one `new` handed off. codex's is `--no-daemon`; claude
  and pi have none.
- **`session_env`** — the variable the vendor puts in the environment of
  every process it starts, naming the conversation. It is what lets `adopt`
  know which of the vendor's own agents it was typed inside, and it is on the
  `not_inherited` list too, because an agent that kept its spawner's session
  id would file its events under somebody else.
- **`not_inherited`** — the vendor's own variables a fresh pane must not
  inherit. The vendor's alone: what belongs to any pane is the caller's
  business.
- **`capabilities`** — what amx may ask of this vendor. The whole list:
  `Hooks`, `Transcript`, `Resume`, `Fork`, `Adopt`, `Trust`. A verb asks
  before it acts and refuses naming the gap. A vendor with no entry has none
  of these, which is the floor every unregistered command stands on: a pane
  to watch, and nothing amx pretends to know about what is in it. `Hooks`
  and the `hooks` field below say the same thing, and `Transcript` needs
  `Hooks`: the file is named on a hook payload and nowhere else, so a vendor
  that reports nothing never puts one on a record.
- **`hooks`** — which files amx writes and where (the `wire`: `Wire::Plugin`,
  claude's plugin directory; `Wire::File`, pi's extension; `Wire::Hooks`,
  amx's groups merged into a hooks file the vendor and the person share and
  trusted in the config beside it, which is codex's; or `Wire::Placed`, one
  file of amx's at `path` inside `$<dir_env>`, else `dir` under the home,
  which is opencode's `plugins/amx/tui.js` in `OPENCODE_CONFIG_DIR` and is
  installed, read and removed the way a file wire is), the vendor's own name
  for each of the eight moments amx listens for (started, prompted, taken,
  calling, asked, refused, notified, ended), its tool matcher, its question
  tool, its two notification types, the sentence it writes on a permission
  box, the prompt openings that mark a turn the vendor typed itself
  (`injected`), the `source` that marks a session opening as a new
  conversation (`fresh_start`), and the payload `kind`s that make a notice a
  question (`question_kinds`). `install` writes from this; `hook` reads by
  it, always through the record's own vendor (`vendor::hooks_for`). What the
  payloads have to carry is [the wire](#the-wire). `None` from a vendor that
  reports nothing, and then install has nothing to wire and leaves the
  machine alone.
- **`transcript`** — the shape of the conversation file a report names,
  `Transcript::Claude`, `Transcript::Pi`, `Transcript::Codex` (the rollout
  jsonl codex appends under `$CODEX_HOME/sessions/`) or `Transcript::Opencode`
  (the message list opencode's plugin writes to
  `$AMX_DIR/opencode-messages.jsonl` at each turn's end), for
  `crate::conversation` to read it by. `None` from a vendor whose file nobody
  has sat down with.
- **`ends_options`** — the word after which the vendor reads no word as a
  flag, put in front of a task, a resume's message or a fork's prompt, so a
  task opening with `-` is not read as one. pi's is `--`; `None` from a
  vendor that reads its last word as a prompt whatever it opens with.
- **`attaches_at`** — whether the vendor reads a message word opening with
  `@` as a file to attach even after `ends_options`. pi's does, so amx puts
  one space in front of such a task, message or prompt, which pi reads as
  words.
- **`restores_queued_on_cancel`** — whether a cancel puts the messages the
  vendor was holding behind the turn back in its composer. pi's does, so
  `amx interrupt` writes them down and `send` refuses to type after them
  until the next prompt.
- **`prompt_flag`** — the flag a task, a resume's message or a fork's prompt
  rides on as one `<flag>=<text>` word, for a vendor that reads no bare word
  as a prompt. opencode's is `--prompt`; `None` from a vendor whose last word
  is its prompt.
- **`popups`** — the characters that open a popup in the vendor's composer
  when they open a word. A message whose last word opens with one gets one
  trailing space, or the popup takes the Enter. opencode's are `@` and `/`;
  empty from a vendor whose Enter always sends.
- **`cancel_presses`** — how many Escapes, 300 ms apart, cut a turn. One for
  claude, pi and codex; two for opencode, whose first press only arms.
- **`interrupt_signal`** — the signal `stop` sends the pane's process to end a
  turn before it ends the pane. opencode's is `SIGUSR2`, which its plugin
  answers by interrupting the session; `None` from a vendor whose pane is
  ended as it stands.
- **`screens`** — the document naming what this vendor's screens look like,
  in the format of `assets/screen-rules.toml`: ordered rules, each built from
  measured anchors, with the capture, the version and the date each anchor
  was read at. `None` from a vendor nobody has sat in front of yet, and then
  its pane is watched and never named. Screens are measured against a running
  program; a document written from anywhere else is a transcription.
- **`catalog`** — what this vendor can be asked for by name: the directories
  it loads skills, commands and agents from, the commands it answers out of
  itself, the `sigil` that opens a word asking for one (`/` for claude and pi,
  `$` for codex, which runs a skill as `$name` and runs none as `/name`), and
  what stands between the sigil and a skill's name, which is nothing for
  claude and codex and `skill:` for pi. A directory is a
  `Place` under the person's home or under the project the agent is running
  in, and a `*` segment in one stands for every directory at that level —
  claude keeps its plugins under a market, a plugin and a version, none of
  which is amx's to name in advance. `None` from a vendor whose layout nobody
  has measured, and then a word typed on a line for it is only ever the word
  somebody typed.

## What the rest of amx asks

Nothing outside the table spells a vendor's flags, events, sentences or
variables. The questions the rest of amx puts to an entry:

- `spawn` — which variables to strip, which flags the dials become, which
  flag opens a session under the id amx just minted, and which words go right
  after the program every time.
- `install` / `uninstall` / `doctor` — which files, which events.
- `hook` — which moment a payload's event name is, which tool is the
  question tool, how the permission sentence reads.
- `rules` / `derive` / the card — which screens document, whose chrome
  anchors cut the furniture. Through `rules::of` on the command the record
  kept, so a pane is read by the document of the vendor that drew it: see
  [pi](#pi).
- `fork`, `resume`, `adopt`, `trust` — may I, before anything is spawned.
  The refusal is immediate and says which capability is missing.
- `logs` — whether this vendor keeps a conversation to read back, before it
  opens a transcript path a record carries, and which gap to name when the
  pane has gone and nothing was recorded either. The chrome it cuts off a
  fallback capture comes off the same entry, like everybody else's: the
  anchors that find pi's box are nothing claude draws.

One thing stays outside the table: `trust`'s store,
`$CLAUDE_CONFIG_DIR/.claude.json`, is a literal in `trust.rs`, with a test
tying that file to whichever vendors the table says can answer the screen by
writing a store — `[claude]`, today. pi answers its own with a flag instead.
The keys a hook payload carries are not the table's either, but they are not
a vendor's: they are amx's, the contract every wire delivers, and the next
section is that contract.

## The wire

A vendor that reports runs `amx _hook` once per moment with one JSON object on
stdin. The keys are amx's, whatever the vendor calls things inside its own
process: claude's hook runner happens to send them as they are, and pi's
extension, `assets/pi/amx.ts`, builds them out of pi's event arguments. pi is
the reference for a third vendor, because its payload is one amx wrote and
every key on it is one amx reads. A third vendor either sends these keys
itself or gets a wire of amx's own that builds them, the way pi did.

Every payload carries these, whatever the moment:

- **`hook_event_name`** — the vendor's own name for the event, which is how
  the payload is found in the record's vendor's `Hooks.events`. A name the
  entry does not list is written to the log and moves nothing.
- **`session_id`** — the conversation the report is about. It finds the
  record when the process has no `AMX_ID` (an agent somebody adopted), and a
  report about a session the record does not carry is another process's and
  moves nothing, except a session opening, below. Optional, and a report
  without it is the agent's own.
- **`transcript_path`** — the file the conversation is written to. A record
  with none takes it from the first report about its own session. Needed for
  `Transcript`, and the reason that capability needs `Hooks`.
- **`agent_id`** — present and not null on a report about a subagent's work.
  Such a report is logged and moves nothing on the record. pi sends none.

Each moment reads these besides, and nothing else. What pi sends is the
reference; what claude sends past these keys is claude's and amx ignores it.

| Moment | pi event | claude event | codex event | Keys read | What they are for |
| --- | --- | --- | --- | --- | --- |
| Started | `session_start` | `SessionStart` | `SessionStart`, at the first turn | `source` | A `source` equal to the entry's `fresh_start` under another session is another process opening one, and is dropped; `clear` under a new session is a fresh conversation in the same pane and clears the record's question and answer. pi sends no `source`, and `fresh_start` is `None` for it. |
| Prompted | `agent_start` | `UserPromptSubmit` | `UserPromptSubmit`, a steered message included | `prompt` | A prompt opening with one of `injected` is a turn the vendor typed itself, and keeps the last answer. pi sends no `prompt`. |
| Taken | `message_start` (user messages only) | none | none | none | A queued message went into the running turn. Moves no state; `send` reads it as the message landing. |
| Calling | `tool_execution_start` | `PreToolUse` | `PreToolUse` | `tool_name`, `tool_input` | The tool that runs. `tool_name` equal to `question_tool` is a menu, and its `tool_input.questions` (each `header`, `question`, `options` of `label`, `description`, `preview`) is the question. |
| Asked | none | `PermissionRequest` | none | `tool_name`, `tool_input` | A permission box over `tool_name`, worded by `permission_sentence`; or the menu again, when it is the question tool. |
| Refused | `ui_prompt_end` | `PermissionDenied` | none | none | The box closed without the tool running: back to working inside an open turn, to idle outside one. pi sends `kind`, which amx does not read here. |
| Notified | `ui_prompt_start` | `Notification` | none | `notification_type`, `kind`, `message` | `notification_type` equal to `idle_notice` is the idle nudge and to `permission_notice` a permission box. A `kind` listed in `question_kinds` makes the notice a question (pi: `input`, `editor`, `select`, `confirm`). `message` is the question's words. pi sends `kind` and `message` and no `notification_type`. |
| Ended | `agent_settled` | `Stop` | `Stop`, none for an Esc'd or errored turn | `last_assistant_message`, `stop_reason`, `background_tasks` | The answer, unless `stop_reason` is `aborted` or `error`. `background_tasks` is a list of `{type, status}`; each `running` one keeps the agent working, `type: shell` counted as a shell and anything else as an agent. pi sends no `background_tasks`. |

pi also sends `cwd` on every report and `role` on `message_start`; amx reads
neither. codex sends a `turn_id` and its own `model` and `permission_mode` on
each, and no `stop_reason` on `Stop`, which it sends only for a turn that
completed; amx reads none of those either. `amx events` shows one key per moment as the event's detail —
Started's `source`, Prompted's `prompt`, Calling's `tool_name`, Notified's
`message`, Ended's `last_assistant_message` — so a key a vendor leaves out
is a blank there and nothing worse.

A vendor whose wire `listens()` (a `Wire::File` or a `Wire::Placed`, amx's own
code inside the vendor) reads what `_hook` prints back: the record's directory, which is how
a pane amx did not start learns where to write `live` and `heartbeat`. A
settings, plugin or hooks wire is the vendor's own hook runner, and gets
nothing back: claude puts what a hook prints into the conversation, and codex
feeds it to the model.

## claude

claude is the first entry, in `src/vendor/claude.rs`, and every value in it
carries the version and the date it was read at. The dials come off 2.1.237's
`--help`; the hooks, the question tool and the two notification types were
measured against 2.1.240's own event list on 2026-08-25; the transcript shape
above is 2.1.240's too. The screens were driven again against **2.1.259 on
2026-09-05**, at 220, 54, 40, 30 and 24 columns, and `docs/claude-screens.md`
is that pass: the capture for each screen, the verdict
`assets/screen-rules.toml` gives it, and what every anchor read.

Three of the six rules moved, and every one of them broke narrow: two went
quiet and one went confidently wrong. A claude on its own folder-trust gate
read `unknown` at 24 columns, and an unclaimed screen is not a gate, so
`doctor` had nothing to report about an agent that would sit there until
somebody attached. A menu somebody was standing at read `unknown` there too,
its box taller than the rows a rule may see. And a claude with a turn running
read `idle` at 30 and at 24 — not the honest answer but the confident wrong
one, on the failure the spinner rule's own comment calls the worst available,
since naming a screen non-blocking also clears a pending question off the row.
Four agents tiled on a 160-column terminal is 40 columns each and a fifth takes
them under it, so these are widths amx reaches by itself.

The rest of what the bump cost was above the screens. 2.1.259 draws the
folder-trust gate as two rows with no number on either and the cursor on the
one that ends the agent, so the question read back as an answer, every key
`answer` was allowed to type there was inert or fatal, and the offer a waiting
row printed named keys the screen would not take. `answer` has the cursor moves
now and reads a walk and the take at the end of it as one line; the offer is
worked out from what was read off the screen rather than from the kind of
question; and a key whose effect amx cannot check leaves the record `waiting`,
so the same screen can be answered again instead of being refused while it is
still on the pane.

The pass drove screens and nothing else. `--help`, the hook list and the
transcript shape were not re-driven and still carry their 2.1.237 and 2.1.240
dates. `[furniture] spinner`, which walked over the two punctuation fragments
the `spinner` rule was moved off, was driven the day after and moved onto `ing…`
with the rule.

The screens were driven a third time against **2.1.270 on 2026-09-14**, at 100,
54, 40, 30 and 24 columns, and the second half of `docs/claude-screens.md` is
that pass. Nothing the document already knew had moved. Three screens it did not
know were being read wrong: the transcript viewer and the answered `/btw`
overlay were claimed as `idle` by the rule that stands on the mode row, and the
line claude leaves above the composer while a background subagent runs — `✻
Waiting for 1 background agent to finish` — was claimed as `idle` at four of the
five widths. The first two are refused by name now and claimed by nobody; the
third has a rule of its own.

That pass was driven beside herdr's claude manifest, version `2026.09.04.1`,
which is the only other published measurement of these screens. herdr reads
panes the way amx does and writes what it reads in TOML too: a manifest per
vendor, five states on it (idle, working, blocked, done and unknown), a rule
that can say a screen means nothing about the agent at all
(`skip_state_update`), `blocked` claimed only on a widget it recognises, and the
manifests themselves updated from its own website between releases.

Three of those amx has under other names. Its rules claim `waiting`, `working`
and `idle` and nothing else, because every other phase on the record comes from
a hook or an exit status rather than from a picture. A screen that should say
nothing needs no rule here at all: an unclaimed screen already leaves an idle or
a waiting record its own word and reads `unknown` over a running one, which is
what the viewer and the overlay were given. And claiming `blocked` only on a
recognised widget is the anchor law with the emphasis somewhere else. The one
thing amx will not take is the manifest from a website: a vendor's entry here is
code with its document beside it and a test tying the two together, and a string
nobody on this machine has read off a live screen is not an anchor.

What amx leaves unknown, and means to: the pane title. herdr ranks two title
rules above every screen rule in its claude entry, `osc_title_working` and
`osc_title_idle`, measured at 2.1.227 and 2.1.228, and a title is the one signal
a narrow pane cannot truncate — which made it the most attractive thing on that
manifest. On 2.1.270 there is nothing on it to read: seventy samples of claude's
own title across two turns, taken twice a second, every one of them opening with
`✳` and none of them changing while the state did. Working, a menu, a permission
box and idle all wear the same glyph, so amx reads no title and this bump gives
it no reason to start. The `/model` picker is left unknown for a plainer reason
— amx has no verb that answers it — and herdr's pi manifest, one rule measured
in June against amx's nine, is where the comparison runs the other way.

## pi

pi is the second real entry, in `src/vendor/pi.rs`, every value in it measured
against 0.84.4 on the date it carries and read again against 0.85.1 on
2026-09-06 — `docs/pi-screens.md` says what that re-read moved.

It can be resumed, forked and adopted. `--session-id <id>` is mint-or-open —
it opens the session already under that id, or creates one if none exists —
so the same flag serves as both `start` and `resume`; `pi --fork <origin>
--session-id <new>` branches into an id amx chose, which is `ForkSpec::Origin`
rather than claude's marker; and `PI_SESSION_ID`, which pi puts in the
environment of every command its bash tool runs, is what `adopt` reads to say
which conversation it was typed inside, the same way claude's
`CLAUDE_CODE_SESSION_ID` does.

It reports through hooks, and amx reads its conversation back. pi's extension
events are JS callbacks inside its own process, not entries in a settings
file can name, so its wire is a file: `Wire::File` at
`.pi/agent/extensions/amx.ts`, whose body is `assets/pi/amx.ts`, written by
`amx setup pi` where pi loads a global extension from and removed by `amx
uninstall`. That file runs `amx _hook` once per moment with the payload on
stdin, under pi's own event names — `session_start`, `agent_start`,
`tool_execution_start`, `message_start` (a user message only, which lands as
`Taken`: the word that a message steered into a running turn went in),
`ui_prompt_start`, `ui_prompt_end`, `agent_settled` —
and the keys [the wire](#the-wire) names, so `hook` reads it with no arm of its
own. `ui_prompt_start` lands as `Notified` and `ui_prompt_end` as `Refused`;
there is no `Asked`, because pi asks leave for nothing. `Transcript` comes
with it: a report names the session jsonl pi appends to as a turn runs,
`~/.pi/agent/sessions/<encoded-cwd>/<ts>_<id>.jsonl`, and `Vendor.transcript`
names the shape `crate::conversation` reads it by. While a turn runs the
extension also streams the words of the answer being written to the record's
`live` file, which the card shows under the conversation. It finds that record
by `AMX_DIR` in a pane amx started, and otherwise by what the hook answers:
`amx _hook` prints the record's directory on every report about a vendor wired
through `Wire::File` (`Wire::listens`), which is how a pi `adopt` took over
streams too. A vendor whose own hook runner runs the
entries — claude, through the plugin amx writes — is never answered, because
that runner shows what a hook prints. `Trust` is the one
amx sends rather than writes: `--approve, -a` on the argv of a pane amx was
starting anyway.

The extension beats on that record as well. What a vendor reported is believed
for `derive::FRESH` seconds and then the pane is read instead, and a turn
sends nothing between its tool calls, so a call that runs for a minute leaves
the record quiet for a minute. The screen under it is not always one a rule
claims: the two things a mid-turn pi is recognised by are the braille frames
and the stats-line footer, and an extension can redraw both. Measured on
0.85.1 with throwaway extensions, a static dot alone left the wall right and a
custom footer alone did too, but a pane with both read `unknown` from ten
seconds in until the turn settled, for a forty-second tool call and a
streaming answer alike. The answer is not another anchor. While a turn runs
the extension is alive and knows it, so it says so on disk: `<record>/heartbeat`
written when the turn starts and every three seconds after that, on an unref'd
timer, and taken away at `agent_settled`. `derive` reads that file's mtime as
one more thing heard from the agent, beside `last_event` and `since`, and the
evidence stays `hooks` — a beat is the vendor's report that the turn goes on,
which is what a hook is.

Nothing about that file names pi. It is the record's, the way `live` is, so
any vendor whose wire can write there may beat; claude has no extension and its
chrome is not extensible, so nothing changes for it.

A pi without the extension reads the way a claude with its hooks unwired
reads: off its pane, against the screens document, with `doctor` naming the
gap. The hookless machinery `derive` grew for a vendor that reports nothing —
writing what a confident reading concluded, `read.prompt` and `read.turn-end`
— is not what pi takes any more; it stands for a vendor that has no other
account of itself, and the test-only second vendor keeps it proven.

`screens` is where what amx can read off a pi pane is written down: eight
rules — the first-run setup gate, the folder-trust question, a dialog, an
editor, an input, the spinner, the login box and the prompt — plus the chrome
that comes off a capture before anybody reads it, driven live against 0.84.4,
re-driven against 0.85.1, and checked in as `assets/screen-rules-pi.toml`. The entry declares it with
`include_str!`, so it is in the binary; `rules::of("pi")` finds it, parses it
and hands it back, and `rules.rs`'s own tests read pi's screens exactly that
way. Which screen each rule was measured on, and what the rest of pi's screens
read as beside them, is `docs/pi-screens.md`.

Every reader asks for them. `furniture`, `derive`, `send`, `doctor`, `status`
and the view all reach a document through `rules::of` on the command the
record kept, which is where the vendor a spawn resolved survives: the flag and
the config it came out of are gone by the time anybody reads one. So a pi pane
is cut and claimed with pi's own anchors. A record naming no command — a shell
command, or one written before amx kept that field — reads
`registry::entries().first()`, claude by where the table lists it and a law
holding that order, which is the reading every pane had before there was a
second document to choose from.

## codex

codex is the third real entry, in `src/vendor/codex.rs`. Every value in it was
read off codex-cli 0.157.1 on 2026-09-28: the dials and the session words off
its `--help`, and the rest off the source tag it was built from, rust-v0.157.1,
with a file and line beside each value. The screens were driven live the same
day at 220, 100, 54 and 24 columns, and `docs/codex-screens.md` is that pass.
The measurement ran under a scratch `CODEX_HOME` holding copies of `auth.json`
and `config.toml`, and never wrote the person's `~/.codex`.

**The dials.** `--model` is open, and the list a typed model is looked for in
is the account's own: `codex debug models` prints it as JSON, and amx takes
the slugs codex's picker would offer. `--sandbox` is the permission dial,
closed over codex's three modes; the approval policy is the other half of
codex's permissions and stays whatever codex or the agent command says, since
amx has one dial and every run is under a sandbox. codex has no effort flag,
only a setting, so the effort dial is keyed, `-c model_reasoning_effort=<v>`,
and closed over the levels the catalog's models support between them. It is
partial on purpose: which of those a model takes is per model, and a level the
model lacks is codex's to refuse.

**Sessions.** codex mints its own ids and takes none, so `start` is `None` and
the session is named by the first `SessionStart`, which codex fires at the
first turn rather than at launch. A record learns its session then, and
`resume` and `fork` already refuse one that has none. `codex resume <id>` and
`codex fork <id>` are subcommands; `--last`, `--all` and
`--include-non-interactive` are the other words that pick a session, and they
are what a resume or fork replaces. `CODEX_SESSION_ID` is in the environment of
every command codex's shell runs, which is what `adopt` reads. `--` ends the
options, since clap reads a task opening with `-`, or one that is a
subcommand's name, as something other than a prompt.

**`--no-daemon`.** By default a codex TUI connects to a shared app server, and
that server runs the hooks in its own environment. `AMX_ID` never reaches
`amx _hook` there, and a turn goes on running after its pane is killed. So
every codex amx starts carries `--no-daemon` in `launch`, and it runs its own
app server that ends with the pane.

**The hooks wire, and trust.** codex reads a `hooks.json` beside its
`config.toml`, in `$CODEX_HOME` or `~/.codex`, and runs each handler with the
payload on stdin. That file is the person's too, so `amx setup codex` merges
amx's four groups into it, one per event, each with the single handler
`amx _hook`, and adds nothing else. A group codex has not seen before draws a
hooks-review screen at the next start, which an agent amx started would sit
at, so setup also writes the trust codex would have written had the person
trusted the groups there: a `[hooks.state."<key>"]` table in `config.toml` for
each, with a `trusted_hash`. The key is the hooks file's real path, the event
in snake case and the group and handler indices. The hash is codex's own
recipe, a sha256 over the sorted, compact JSON of the event and the handler
with codex's defaults filled in, checked against the `currentHash` codex's app
server reports. amx trusts its own groups and nobody else's, and never passes
`--dangerously-bypass-hook-trust`. Both files are copied aside before amx
first edits them, `config.toml` through `toml_edit` so the rest of it is
left as written. `amx uninstall` puts the copies back when nothing else has changed
the files since, and otherwise takes out amx's groups and tables alone.
`doctor` fails a missing or stale hash. A hooks wire never listens: codex
feeds what a hook prints to the model.

**No `Trust`.** codex keys a folder's trust on the main checkout's root, so
trusting a worktree amx cut would trust the person's whole repository. The
folder-trust screen, the hooks-review screen, the resume working-directory
prompt and the update prompt are all `setup` rules: the person answers them,
and `doctor` names an agent stopped on one. The same key means a worktree amx
cuts in a trusted repository is trusted already.

**What codex cannot tell amx, and how amx reads it instead.** Four moments are
wired out of codex's twelve events: Started, Prompted, Calling and Ended.
There is no Taken: a message steered into a running turn is a second
`UserPromptSubmit` under the same turn, read as Prompted, which leaves a
working record working. There is no Notified, since codex has no notification
hook, and no Asked or Refused. `PermissionRequest` exists but fires before any
box is drawn, and a reviewer may answer it with no box at all, so the approval
box is read off the screen by the `approval` rule. And there is no Ended for a
turn that did not complete: codex sends no `Stop` for an Esc'd turn or one
that failed on an error. Its `Interrupt` hook is not wired either. The pane
reader closes such a turn, writing `read.turn-end` when the `prompt` rule has
held still long enough, and `result` reads what happened out of the rollout:
`turn_aborted` is a turn that was aborted, a `task_complete` with an `error`
is a provider failure, and a `task_started` with nothing after it is a pane
that was killed mid-turn. None of those has an answer to hand back.

**The catalog.** codex runs a skill as `$name` anywhere in a message, and
`/name` runs none, so the catalog's sigil is `$`. It reads skills from
`~/.agents/skills`, `~/.codex/skills`, and the project's `.codex/skills` and
`.agents/skills`. Custom prompts are gone from codex, roles have no CLI flag
and a slash command typed as the first prompt is sent as words, so there are
no commands, agents or built-ins. amx reads one directory deep and names a
skill by its directory; codex walks six deep, names a skill by its
frontmatter, reads `$CODEX_HOME/skills` rather than `~/.codex/skills`, and
reads every directory from the project root down to where it runs rather than
only where it runs. A skill in any of those places is not offered.

### What the dogfood saw on codex 0.157.1, 2026-09-28

The rig: amx built from the entry above and installed with `cargo install
--path .`; a scratch `CODEX_HOME` holding copies of `auth.json` and
`config.toml`, with the scratch repository trusted in that copy; and a scratch
repository with one commit whose `.amx/config.toml` said `agent = "codex"` and
set `CODEX_HOME` to the scratch directory under `[codex.env]`, allowed with
`amx allow`. The agents went on the real wall, beside every other agent on
the machine, and the model was the account's default, GPT-6-Luna.

`CODEX_HOME=<scratch> amx setup codex` added the four groups to the scratch
`hooks.json` and four `[hooks.state]` tables at indices `0:0` to its
`config.toml`, with the copy of the file as it was named on the way out.
`doctor`'s codex row came up green: *amx's hooks in …/hooks.json, trusted in
…/config.toml*.

**The Show.** `amx new "list the files here"` ran `codex --no-daemon -- list
the files here`, and codex came up with no hooks-review screen, in front of
the composer or anywhere else. The row read `working` on the first look and
`idle` four seconds later, and the events were codex's own: `SessionStart
startup`, `UserPromptSubmit`, `PreToolUse Bash`, `Stop`. `amx result` printed
the answer the `Stop` carried. `amx stop` and then `amx resume <id> "how many
files did you find? one word"` ran `codex resume <session> --no-daemon --
<message>`, the next event was `SessionStart resume` under the same session,
and the answer, *Two*, came out of the turn before the stop. `amx fork <id>
"reply with the word FORKED"` ran `codex fork <session> --no-daemon --
<prompt>`, and its first turn fired `SessionStart fork` under a new session
id, with a second rollout beside the first carrying it. A codex started by
hand in a tmux pane under the same `CODEX_HOME` drew no review screen either,
and a spawn cut into a worktree of the trusted repository met no trust screen:
codex keys that trust on the main checkout, as the entry says.

**Two vendors on the wall.** A claude spawned from the same repository with
`--agent claude` stopped on its folder-trust screen and read `waiting` on
`folder_trust`, beside three codex rows on `prompt`. No row read `unknown` and
none carried the other vendor's rule.

**The `$`.** With a `.agents/skills/demo/SKILL.md` in the repository, the view
opened with `amx --dir <repo>` said `next codex`, and `$de` on its task line
offered `$demo` with the skill's description. `/` offered nothing there. The
line at the foot of a codex agent's card offered the same, and so did a task
line written `agent:codex $de` in a view whose default agent was claude.

**The refusals came back in codex's words.** `--effort ultra`: *codex takes
default, low, medium, high, xhigh, max*. `--permission plan`: *codex takes
default, read-only, workspace-write, danger-full-access*. Both exit 64 before
anything is spawned. `--model gpt-5.5 --effort low` picked codex out of its
listing and ran `codex --no-daemon --model gpt-5.5 -c
model_reasoning_effort=low -- <task>`.

**Esc, interrupt, logs.** `amx interrupt` in the middle of a `sleep 30` ended
the turn at once: `interrupt` and `read.turn-end` in the same second, and the
row on `prompt`. The shell command kept running as codex's background
terminal, as `docs/codex-screens.md` says it does. `amx logs` read the rollout
back, a prompt and an answer per turn and a `› exec` row for each tool call.

**What it found.** Seven things. None is a wrong value in the entry and none
was fixed in this pass; each is here with its evidence and where a fix would
go.

- **`send` of a message that ends in a `$word` does not submit.** `amx send
  <id> 'run $demo'` left `run $demo` staged in the composer and exited 1 with
  *did not start working within 5s; the message may not have reached it*. The
  paste ends with codex's skill list open under the word, and codex takes the
  Enter as inserting the highlighted skill. A second Enter by hand submitted
  it and the skill ran. `use $demo and nothing else`, with the word in the
  middle, submitted on the first Enter. The fix belongs to `send`.
- **Esc typed in the pane leaves the row `working` for about fifty seconds.**
  Esc six seconds into a `sleep 25` turn: codex drew its interrupted line and
  the idle composer at once, and `amx result` said *the turn was aborted*
  straight away, out of the rollout. The row said `working` until
  `read.turn-end`, 55 seconds after the last hook: the freshness window and
  then the patience a quiescent rule is owed. claude's Esc is read at once,
  because `Furniture::cut_by_hand` finds the interrupted row at the foot of
  what claude's chrome leaves. That walk hangs off a composer box, and codex
  draws none, so its document has no `[furniture]` and nothing finds codex's
  `■ Conversation interrupted` row as the news. `amx interrupt` does not have
  this lag. The fix is a furniture walk that can find a composer with no box.
- **A turn that ends on an error is read the same way.** `--model gpt-5.5` is
  in the account's listing with visibility `list`, and the account cannot run
  it: codex answered `404 … does not exist or you do not have access to it`,
  sent no `Stop`, and `read.turn-end` came 53 seconds after the prompt.
  `result` said *the provider failed* and quoted codex. The listing is the one
  codex's own picker shows, so amx offers what codex offers.
- **`adopt` from inside codex found the wrong amx.** codex runs its shell
  tool, and a `!` command, through a snapshot of the login shell, and that
  PATH put an older amx in `~/.local/bin` ahead of the one just installed. The
  older amx has no codex entry, so the pane named no vendor it knew and the
  environment answered alone. The tmux server running the codex was one the
  driving claude session had started, so it held `CLAUDE_CODE_SESSION_ID`, and
  the record came out `agent: claude` with that session on it. The driving
  session's own hooks then filed onto the adopted row. `doctor` had already
  failed the machine for having two amx on the PATH. Run by its full path, the
  installed amx adopted the same kind of pane as `codex` under codex's own
  session id. Its first state was `working`, read off the screen while the `!`
  command ran, and it read `idle` 31 seconds later, the same wait as the Esc
  above.
- **`doctor` asks about the person's agent, not the project's.** In the
  scratch repository, and under `amx --dir <repo> doctor`, the agent row said
  claude and the trust check asked claude's store, though the repository's
  allowed file says codex. `doctor` reads the person's file alone.
- **The bare view is the person's file too.** `amx` from inside the
  repository said `next claude`, and a task typed on its line started a claude
  there. The README says the view reads a project's file only under `--dir`,
  so this is the documented reading, and a person with `agent = "codex"` in
  their own file meets none of it.
- **A project may set `CODEX_HOME`.** A project's `[codex.env]` set it and
  amx took it, where `[claude.env]` may not set `CLAUDE_CONFIG_DIR`. The two
  do the same job: they pick which of the vendor's configs runs, and codex's
  holds the hooks and the trust for them. And `amx setup codex`, `doctor` and
  `uninstall` read `CODEX_HOME` from the environment they run in and never
  from `[codex.env]`, so a home named only there is one setup never wired.
  Closed the same day: a project file may no longer set `CODEX_HOME`, as it
  may not set `CLAUDE_CONFIG_DIR`. A person's own `[codex.env]` still may,
  and setup, doctor and uninstall still read only the environment.

## opencode

opencode is the fourth real entry, in `src/vendor/opencode.rs`. Every value in
it was read off opencode 2.0.16 between 2026-09-29 and 2026-09-30: the flags
off its root command, and the rest off the source tag it was built from,
v2.0.16, with a path under `packages/` and a line beside each value. The
screens were driven live on 2026-09-30, and `docs/opencode-screens.md` is that
pass. The measurement ran under a scratch `OPENCODE_CONFIG_DIR` copied from
`~/.config/opencode`, on the free `opencode/longcat-2.5-preview-free`, which
answers with no provider connected.

**`--standalone`.** By default an opencode TUI joins a shared service, and the
service runs in the environment of whichever client first started it. Under
it a pane's `OPENCODE_CONFIG_CONTENT` does nothing, and a turn goes on after
its pane is killed. So every opencode amx starts carries `--standalone` in
`launch`, once, on `new` and `resume` alike, and runs a server of its own
(`opencode serve --stdio`) that ends with the pane. amx never runs opencode
outside a pane either: even `opencode --version` or `opencode models` starts
the shared service, with the caller's environment.

**The dials.** The TUI takes no model flag, so the model rides in the pane's
environment as `OPENCODE_CONFIG_CONTENT` = `{"model":"<value>"}`, which
opencode loads last, over every config file. That is on `new` only: a resumed
session keeps its own model, and a variable the environment already carries
stands. The dial is open, and the entry lists no models, since the command
that would print them starts the shared service. `--auto` is the permission
dial, bare, closed over `default` and `auto`. There is no effort dial: a
`#variant` on the model loses to the variant opencode stores per model.

**Sessions.** opencode mints its ids and takes none to start under, so the
plugin's Started names the session. `--session <id>` resumes it, as two words;
`-s`, `-c`, `--continue` and `--server` are what a resume replaces. There is
no fork flag in the TUI, no variable naming the session in what opencode runs,
and no trust screen, so there is no Fork, no Adopt and no Trust.

**The task.** The root command's only positional is a directory, so a task or
a message is one word, `--prompt=<text>`. On the home route `--prompt` fills
the composer and does not submit, so the plugin presses `prompt.submit` every
500 ms while the route is home, only under `AMX_ID` and a `--prompt=` word in
its argv, and stops at the first session route, the first turn or 30 seconds.
`--session <id> --prompt=` submits on its own. `@` and `/` open opencode's
autocomplete, which takes the Enter, so a message whose last word opens with
either gets one trailing space.

**The wire.** opencode's TUI loads `plugins/<dir>/tui.*` under its config
directory with no registration, and `OPENCODE_CONFIG_DIR` replaces that
directory. `amx setup opencode` writes `plugins/amx/tui.js` there, plain JS
out of `assets/opencode/tui.js`, and opens no other file: `opencode.json`,
`tui.json` and `cli.json` are never touched. The plugin says nothing without
`AMX_ID`, reports only its own route's session, walked to the root, and hands
each moment to `amx _hook` the way claude's hooks do. A project's
`[opencode.env]` may not set `OPENCODE_CONFIG_DIR`, `OPENCODE_CONFIG` or
`OPENCODE_CONFIG_CONTENT`, since each picks which config runs, and the first
picks where the plugin is.

**The moments.** All eight are wired, three of them under names the plugin
coins: Started is `session.selected`, the first session route, with `source`
`startup` or `resume`; Prompted is `session.execution.started`; Taken is
`session.inbox.delivered` for a message queued while a turn ran; Calling is
`session.tool.called`; Asked is `permission.asked`; Notified is a
`form.created` of kind `question`; Refused is `permission.rejected`, a
`reject` reply or a cancelled form; and Ended is `session.execution.ended`,
over succeeded, failed and interrupted.

**The transcript.** At each Ended the plugin syncs the session's messages and
writes the whole list, one message a line, to `$AMX_DIR/opencode-messages.jsonl`,
and names that file as the transcript. amx never opens `opencode.db`.

**Interrupt and stop.** The first Escape only arms opencode's cancel, so
`interrupt` presses it twice, 300 ms apart. `stop` of an agent in a turn sends
`SIGUSR2` to the pane; the plugin interrupts its session, which ends a turn
even under a permission card, and `stop` waits up to five seconds for Ended
before it ends the pane, warning with the session id when none came. The
order matters: a server killed mid-turn keeps its claim on the session, and
the next boot of the shared service, which shares the database, resumes it
unattended.

**The catalog.** A command runs as `/name`, out of `commands/` or `command/`
in the config directory and in the project's `.opencode/`; agents come from
`agents/` in the same two, and the TUI has no flag to run as one. A skill has
no text spelling, so there are no skills. The built-ins are the app's, the
session's, the prompt's and the server's, aliases and all.

### What the dogfood saw on opencode 2.0.16

On 2026-09-30. The rig: amx built from the entry above and installed with
`cargo install --path . --root <scratch>`, that root first on the PATH; a
scratch `OPENCODE_CONFIG_DIR` copied from `~/.config/opencode` and exported;
and a scratch repository with one commit whose `.amx/config.toml` said
`agent = "opencode"`, allowed with `amx allow`. The install went to a root of
its own rather than `~/.cargo/bin`, which stands ahead of the machine's amx on
the PATH, because another run was driving that amx at the time; for the same
reason the agents went on a state directory and a tmux socket of their own
(`AMX_STATE_DIR`, `AMX_TMUX_SOCKET`), beside a claude on the same wall.
`opencode service start` under the same environment put the shared service up
first (`opencode serve --service`, on 49374), and the database was the
person's own, `~/.local/share/opencode/opencode.db`, read only with
`sqlite3 -readonly`.

`amx setup opencode` wrote the plugin to `<scratch>/plugins/amx/tui.js` and
nothing else. `doctor`'s opencode row came up green: *the plugin at
…/plugins/amx/tui.js*, and its agent row said opencode, read off the project's
file. `doctor` failed only while the machine's own amx was still on the PATH
behind the scratch one, which is its two-amx check doing its job; with that
directory off the PATH (and claude linked in from a scratch bin) every row was
green.

**The Show.** `amx new --model opencode/longcat-2.5-preview-free "list the
files here"` was refused at first; see the first finding. With the model
written under `[opencode] models` it ran `opencode --standalone --prompt=list
the files here`, with `OPENCODE_CONFIG_CONTENT={"model":"opencode/longcat-2.5-preview-free"}`
and the scratch `OPENCODE_CONFIG_DIR` in the pane's environment, and an
`opencode serve --stdio --port 0` under it. The row read `starting`, `working`
two seconds later, `Running read` at six, and `done` (idle, printed) at twelve.
The events were the plugin's own: `session.selected startup`,
`session.execution.started`, `session.tool.called read`,
`session.execution.ended`. `amx result` printed the answer, the four entries
of the directory, out of the message list the plugin wrote to the record.

`amx resume <id> "and count them"` refused an idle agent (*stop it before
starting it again*), as it does for every vendor. After `amx stop` it ran
`opencode --standalone --session ses_… --prompt=and count them`; the next
events were `session.selected resume` and `session.execution.started` under
the same session, and the free model took 33 seconds to answer: *There are
**4** entries*, two directories and two files, the list from the turn before
the stop.

`amx new … "run the shell command sleep 60, then say finished"`, stopped
fifteen seconds into the `sleep`: `amx stop` returned in 0.08 seconds, and
`session.execution.ended` arrived in the same second, with no answer. The
session's row in `opencode.db` read `idle_outcome` `interrupted`, the last
message an `idle` row with outcome `interrupted`, and the `sleep 60` was gone.
The pane's `opencode --standalone` and its `serve --stdio` were still in the
process table the instant `stop` returned, and gone three seconds later; with
the other opencode agent stopped too, `pgrep -f "serve --stdio"` found only
the shells whose own command line carried those words, and no opencode.
`opencode service restart` then came back on 49374, and thirty seconds later
the stopped session still read `interrupted`, three messages, the last at the
same `seq`: nothing resumed it.

**Two vendors on the wall.** A claude spawned from the same repository with
`--agent claude` stopped on its folder-trust screen and read `waiting` on
`folder_trust`, beside opencode rows on `prompt`. No row read `unknown` and
none carried the other vendor's rule.

**The rest of the verbs.** `--permission auto` ran `opencode --standalone
--auto --prompt=…`. `--effort high` was refused with *amx knows no effort dial
for opencode*, and `--permission plan` with *opencode takes default, auto*,
both exit 64 before anything was spawned. A task with `@a.txt` in the middle
submitted on the first try and read the file. `amx send` to the idle agent
started a turn (`session.execution.started`, then `session.tool.called
shell`), and `amx interrupt` eight seconds into it wrote `interrupt` and
`session.execution.ended` in the same second, and the row read `done`. `amx
logs` read the message list back, a `› read a.txt` row for the tool, the
answer, the message as `❯ now run sleep 40 in the shell` and `› shell sleep 40`.

**What it found.** Three things. None is a wrong value in the entry, and none
was fixed in this pass.

- **A typed opencode model is refused where opencode is the configured agent.**
  `amx new --model opencode/longcat-2.5-preview-free …` with `agent =
  "opencode"` in the file exited 64: *opencode takes ; claude takes fable,
  opus, sonnet, haiku; pi takes …; codex takes 0 models (codex debug
  models)*. A typed model picks the harness whose list holds it, and
  opencode's list is its cycle less the sentinel, which is empty; the dial
  being open is never asked. Asking also ran `codex debug models` for a word
  that was never going to be codex's. `--agent opencode` beside the same
  model spawned it, as did the model written under `[opencode] models`, which
  the shipped config already shows. The fix belongs to `picked` in
  `src/verbs/new.rs`: a word no list holds could go to the configured
  harness when its model dial is open, or the refusal could at least say
  where to name the model instead of `opencode takes ` and nothing.
- **The pane's processes outlive `stop` by a moment.** Right after `amx stop`
  returned, the `opencode --standalone` and its server were still running,
  for under three seconds. The turn was already over by then, so nothing ran
  on, but a check typed the instant `stop` returns can find them.
- **`pgrep -f "serve --stdio"` matches more than opencode.** Under a harness
  that runs each command as `bash -c '<the whole line>'`, the shell's own
  argv carries the words, so the check needs those shells filtered out. Not
  amx's.

## What the dogfood saw

This is pi's pass. codex's is at the end of [codex](#codex), and opencode's at
the end of [opencode](#opencode).

pi 0.84.4, 2026-09-05, on a scratch repository with `agent = "pi"` in the
config, amx built from the entry above and put on the PATH in front of whatever
was there, and a state directory and a tmux socket of its own so nothing landed
on anybody's real wall. The provider was
opencode's muse-spark-1.3-contributor-free. This is the pass step 5 below asks
for, written down where the measurements it tests are.

`doctor` came up green on all seven, and the hooks row said what the entry
claims rather than that something was missing: *pi reports nothing amx can
wire, so its pane is what amx reads.*

**The four capabilities.** `new` ran `pi --session-id <the id amx minted>`, and
pi wrote its session to `~/.pi/agent/sessions/<encoded cwd>/<ts>_<id>.jsonl`
under that id. `stop` and then `resume` ran the same flag with the same id, and
the pane came back with the whole conversation replayed on it; a message sent
after that was answered out of the turn before the stop. `fork` ran `pi --fork
<origin> --session-id <new>`, which left a second session file beside the
first, carrying the copy's own id, in the directory the original ran in. `adopt`
was typed inside a pi started by hand, through the vendor's own bash tool,
which is how `PI_SESSION_ID` and `$TMUX_PANE` reach it: the record came out
`agent: pi` with pi's own session id on it, and its first state was read off
the pane it took over. Asked a second time in the same pane it refused —
`amx: %5 is agent the-pi-i-started-hnx already`. With `trust = true` the argv
was `pi --session-id <id> --approve <task>`, the folder-trust screen was not
drawn at all, `~/.pi/agent/trust.json` was never created, and no file of
claude's was written after the pi spawn. pi's own `core/project-trust.ts`
returns on that flag before it reads its store, which is the same answer from
the other side.

**Two vendors on the wall.** Five rows at once, each read by the document of
the vendor that drew the pane: a claude agent `waiting` on `folder_trust` with
*Yes, I trust this folder* beside it, and pi agents on `prompt`, on `dialog`
and on `spinner`. No row carried the other vendor's rule.

**The refusals came back in pi's own words.** `--permission plan`:
*amx knows no permission dial for pi*. `--effort ultra`: *pi takes default,
off, minimal, low, medium, high, xhigh, max*. Both exit 64, before anything is
spawned.

**What it found.** Seven things, and two of them are about which screen amx is
looking at: pi's startup trust gate is not the screen `project_trust` was
measured off, and pi's update notice takes every windowed rule off the pane.
Those two are written down in `docs/pi-screens.md`, beside the rules they are
about. The other five are verbs — four of them the hooks gap showing up in
places the capability list does not obviously cover, and the fifth off the rig
itself — and every one of the five is answered now. Each is here as it was
found, with what closed it under it, so that what a partial entry cost stays
readable after the cost has been paid:

- **`send` always says the message may not have arrived.** It waits five
  seconds for the vendor's `UserPromptSubmit` and pi sends none, so every send
  to a pi agent exits failure with *did not start working within 5s; the
  message may not have reached it*. Measured four times, and the message had
  landed and the turn had run every time. Closed: the word the wait takes is a
  reader's as well as a vendor's. Where the vendor has no hooks to say it with,
  the wait does its own looking, and the turn a look watched begin —
  `read.prompt` in the log — confirms a send the way a `UserPromptSubmit` does.
  A message that genuinely reaches nothing still fails inside the same five
  seconds and still says so.
- **`result` never returns to a pi agent that was sent a message.** It waits
  for a turn that ended after the last `send`, and only a `Stop` event says one
  did. With no `--timeout` it waits forever; with one it exits on the deadline
  and says nothing. On an agent nobody has sent anything to it said it had
  captured none, and that half has been answered since: such an agent gets back
  what the last reading saw on the pane, the way this file describes below.
  After a message it still exits on the deadline, with that same answer on the
  record it will not serve. Closed: the same word on the other edge, which is
  `read.turn-end`, and the wait takes one of those or a `Stop`. Which turn is
  served has not moved — the log is read forward from the last message, so an
  answer from before that message is still the turn before it — and a
  `--timeout` on a turn nothing has watched end still exits 3.
- **An adopted pi whose first reading was `working` stays there.** Nothing ever
  writes a pi record's phase again, and the `prompt` rule is quiescent: from a
  record that says a turn is running it may not decide until the screen has
  held still for thirty consecutive looks, and `still_looks` counts within one
  process. `amx ls` looks once per process, so the row said `working` at an
  empty composer for as long as it was watched, while `amx result` — which
  polls in one process — cleared the same pane in six seconds. `amx status` on
  it named *the vendor's hooks* as the evidence, on a vendor that has none.
  Closed: by two. A quiescent rule's patience is elapsed time on a clock every
  process reads rather than a run of looks one process counted, so a screen that
  has held still long enough is held still for whichever process looks next, and
  `amx ls` settles a pane it could never settle before; and on a vendor with no
  `Hooks` a settled reading writes the phase it read, so nothing is left sitting
  at the word a spawn or an adoption put there. `amx status` names the screen
  and the rule that claimed it now, because the reading that moves the phase
  leaves `last_event` and `since` exactly where they were.
- **The first question amx reads off a pi pane is the question every later one
  shows.** A record learns a question and overwrites nothing, because a hook is
  the vendor's own word and a screen is amx's reading of a picture. On claude
  the next hook clears it. On pi nothing does: an agent driven through a dozen
  screens was still offering *Run echo hi?* as its question when it was stopped
  on the login box, which is the first `ctx.ui.select` it had ever been read on.
  Closed twice, because the first close asked the wrong question. Which law a
  reading was under followed the vendor's `Hooks`, and that held only while pi
  had none: once pi reported through its extension every question on a pi was
  pi's own word again, including the ones pi draws itself and fires no event
  for — `/login`, `/trust`, `/model`, the startup trust gate. Those reach a
  record from the screen and from nowhere else, so a pi stopped on the trust
  selector was still asking for the API key the login box had wanted, with the
  selector's three answers grafted underneath. Closed: the law follows the
  question rather than the agent it is on. The record says whether a hook
  reported the question or a reader read it, a screen fills what a reported one
  left empty and corrects nothing, and a later reading of the pane replaces a
  read one whole — the question and its choices together, and a screen with
  nothing on it to answer clears both.
- **`adopt` takes the first vendor in the table whose session variable is in the
  environment.** This one came off the rig rather than off pi. A pi started from
  a terminal that already had `CLAUDE_CODE_SESSION_ID` in it was adopted as
  claude, with claude's session id on the record and claude's document reading
  the pane — `unknown`, and no rule. Unset that variable and the same pane
  adopted as pi. Closed: the pane decides the vendor, because a session variable
  travels and the program on the other end of a pane cannot. tmux says which
  program is running there and the table is keyed by exactly that, so the
  variable is asked one thing only, which is which of that vendor's
  conversations this is. A pane running a program no entry is keyed by still
  leaves the environment to answer alone.

**Three of those five turn on the edges of a turn, and whose edges they are is
worth saying plainly.** A vendor that reports says where a turn began and where
it ended, in its own words and at the moment each happened. This one says
nothing ever, so the only thing that will ever place either edge is a reading of
the pane — and that is what goes in the log, under amx's own names for them,
`read.prompt` and `read.turn-end`, never a `UserPromptSubmit` or a `Stop` pi did
not send. The reading that writes one leaves `last_event` where it was, so
nothing amx wrote can have the next reader believe the vendor spoke: `amx
status` on that record still names the screen it was read off. It is the same
sentence the answer beside it already carries. pi has not told amx that a turn
ended, amx has looked, and the looking is worth what the screens document that
claimed the screen is worth.

## Adding one

1. **Measure, then write.** Sit in front of the real program. The dials come
   out of its `--help`; the screens come off its panes, captured the way
   `tmux.rs` captures them; the hook names come from its own documentation
   and are verified live. `docs/question-shapes.md` shows the standard a
   measurement is owed — the capture, where it was taken, the payload beside
   it. An anchor nobody measured is a guess wearing the format of a fact.
2. **Write the entry** — a new file beside `claude.rs`, added to the table in
   `src/vendor/mod.rs`. Every field's doc comment says what it means and what
   a wrong value costs. Claim only the capabilities the program has: a
   capability is a promise a verb will act on.
3. **Let the laws hold you.** The table's tests quantify over every entry:
   cycles start at the sentinel, dial flags are distinct and are flags, a
   vendor that reports names a moment at most once and always the three a
   turn stands on — started, prompted, ended — the `Hooks` capability and
   the `hooks` field agree, a vendor that claims a transcript reports through
   hooks, an adoptable vendor
   names its session variable, a session variable never travels, a session's
   own flags are flags too and none of them lists `resume` among what
   conflicts with it, a vendor that resumes reads its own resume words back
   through `names_a_session`, a vendor that can fork says how — and one that forks
   through no hooks declares a start flag, since there is no report coming to
   name the copy's session — declared screens parse, and a vendor that prints
   its models names an argv to print them with that is not empty and opens
   with a flag, a bare dial is closed with one value beside the sentinel,
   only a model dial is carried in the environment and then under a key and
   no flag, a turn is cut in one press or more, and a prompt flag is a flag
   with no `=` in it. The entries before opencode answer its fields as they
   behaved before them: no prompt flag, no popups, one press, no signal and
   no bare or carried dial. A new entry inherits every one.
4. **Prove the conformance.** `tests/mock_claude/` is a stand-in that replays
   scenarios — hook payloads, transcripts, screens — against the real tmux.
   A second vendor's harness takes the same shape: a fake that speaks the
   vendor's dialect, and the suite driven against it. pi's stand-in is
   `tests/mock_pi/pi`, a shell script reached through the PATH, since the
   table is keyed by the program a command runs and that is the whole of what
   makes an agent pi's on a machine with no pi installed; `tests/e2e_pi.rs` is
   the suite driven against it. It replays what pi's entry claims and nothing
   else: the session flags, down to the five pi refuses `--session-id` beside,
   the screens `assets/screen-rules-pi.toml` was measured off, each painted
   in one write, and the reports the extension delivers, as `hook` and
   `transcript` steps of a scenario. Under that sit the table's laws, the
   unit tests in `pi.rs`, and the panes the screens were measured off,
   checked into `rules.rs`.
5. **Dogfood it.** The laws and the stand-in prove the entry against what amx
   measured. Only the real program proves the measurement. Three things to get
   right before the first spawn: install the binary you just built rather than
   the one already on the PATH, name the vendor in the config's `agent` key
   rather than passing `--agent` each time, and point it at a scratch
   repository. The config key earns it twice, because that is how somebody who
   uses this vendor runs and because `doctor` checks the configured agent and
   no other. The scratch repository earns it because every spawn cuts a
   worktree and leaves a branch behind.

   Then one pass per capability the entry claims. Two vendors on the wall at
   once is the reading test: each row should carry the name of a rule from its
   own document and a state that is not `unknown`. Resume is a second pane on
   the session id the first one minted, fork is a second session file carrying
   the copy's own id, adopt is a record whose first state came off the pane it
   took over. The refusals are part of the pass, since a dial the vendor does
   not have and a value off a closed cycle should both come back in the
   vendor's own words. What the capability list leaves off is not a finding by
   itself: a vendor reporting through no hooks lags its pane by `FRESH`
   seconds, which is a partial entry being honest. A finding is amx promising
   what the program does not honour, a pane read with another vendor's anchors,
   or a verb the missing capability costs more than an empty answer. Four of
   pi's five above were the third kind and the fifth was the second — a pi pane
   read with claude's anchors — which is why each is written up with what
   closed it. Write down what you saw beside the measurements it
   tests, with the version on it. An entry is measured on a date, and so is a
   dogfood.

A vendor can also land partially, and honestly. An entry with dials, a session
vocabulary and screens but no hooks still resumes, forks and is adopted, and
carries everything the floor already carries. `logs` is what asks the table
whether such a vendor keeps a conversation to read back, and it names the gap
rather than opening a path that was never going to be one. `result` asks the
table nothing: it reads whatever is on the record, which is what a hook wrote
there, the transcript path a hook named, or — where the vendor has neither —
what a reading of the pane put there.

On a vendor with neither, what reaches the record is a reading. The screen a
rule read as a finished turn is the only account of that turn there will ever
be, and a pane is a picture the next repaint takes away, so the reader writes
down what it saw — the rows the agent earned, with the vendor's own furniture
cut off the bottom the way the card and `logs` cut it — and puts `screen` on
the record beside it as the source. That word is the whole of the honesty here:
an answer that arrived that way is amx's reading of a picture and not the
vendor's word for what it said, it is worth exactly what the screens document
that claimed the screen is worth, and a caller who needs to know which of the
two it is holding asks the record. A vendor that does report is written down no
such way: its own words are already there, and a photograph of them is not
something to put beside them.

**The rows the agent earned are the whole cut pane and not the answer inside
it**, and that is a decision rather than an oversight. Telling pi's own rows
from pi's tools' rows was measured, in `docs/pi-screens.md`'s *Where pi's tools
stop and pi starts*: driven live at 0.84.4 at 220, 100 and 40 columns, against
the renderer behind each of the four shapes a turn leaves. They share one
leading space and carry nothing else at the head of a row; what separates them
is colour, and `capture-pane -p -J` throws colour away; `outputPad = 0` in a
project's own settings takes the indent off the prose and leaves it on the
vendor's rows; a tool row too long for the pane wraps with no head on the
continuation; and pi draws a model's thinking through the same component in the
same column as an answer. Every candidate marker is the agent's to write or the
person's to switch off, so the boundary was refused rather than guessed at —
ruling #QQPQW2VZ — and the walk keeps the law it already had: a wrong number
costs furniture left on the screen and never a row of work taken off it. The
reading that would carry the boundary is `capture-pane -e`, and that document
costs it as an option nobody has taken. The one line a row shows takes the last
thing on the pane, which is where all three widths put the answer.

What the reading does buy is a `result` that ends where the reading ended the
turn. The verb waits for the turn after the last message, as it always did, and
what says one ended is a `Stop` or a reader that watched it happen; a
`--timeout` on a turn nothing has watched end still exits 3, which is the honest
half of it, since a reader that never looked places no edge. The capabilities
list is what keeps a partial entry truthful: nothing is promised that is not
there, and where an unclaimed capability costs a verb more than an empty answer,
the place to write that down is the pass above — with what closed it beside it.
