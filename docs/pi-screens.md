# Every screen pi 0.85.1 draws

Since 2026-09-05 pi reports through the extension `amx doctor --fix` writes
(`assets/pi/amx.ts`), so what a pi is doing, and what a turn answered, come
from its own reports and its session file rather than from these screens. The
screens are still what a reader holds against the pane where the report has
gone quiet: every gate pi draws before a turn, and every pi whose extension is
not installed. Nothing below has changed for that; it is the inventory the
rules were measured against.

Measured on 0.84.4 across 2026-09-04 and 05, and read again against 0.85.1 on
2026-09-06, the day it went on this machine. Three things moved, and each is
written where it landed: the working indicator sits in the composer's top
border (the status lines), the model and thinking selectors spell their cancel
key the way the login dialog does (the blocking widgets, and the login rule's
anchor), and the trust selector's mark for the saved decision moved in front
of the label. The last section says how the re-read was done and what it
found.

`assets/screen-rules-pi.toml` has eight rules in it, and each one was written
from a screen somebody sat in front of. This file is the other half of that
sitting: the whole list of screens the vendor can put on a pane, and what each
one reads as today. It is an inventory and not a second ruleset — no anchor in
it, and nothing here changes what amx recognises. What it is for is knowing
which screens the eight rules cover and which they walk past, so the next pass
at that document starts from a list rather than from whatever screen somebody
happened to hit.

One section is about something other than a screen, and it is near the end. A pi
turn's answer is read off the pane now, which puts the rows *above* the box on
the record as well as on the screen, and no rule has anything to say about
those. *Where pi's tools stop and pi starts* is the measurement of them.

The short version: nineteen of pi's components draw something that waits for a
keystroke. Six are claimed by a rule of their own and read `waiting`. One is
the composer, which is the screen `prompt` was written for. The remaining
twelve read `unknown` — amx saying it cannot account for the screen — and one
of those is a separate program rather than a pane amx watches.

That last number is the one that moved. When this file was first written the
readings were `idle` or `unknown`, and `idle` was the worse of the two: `send`
types into whatever widget is up, and a row that says idle is a row saying
there is nothing to look at. Nothing in the table below reads `idle` any more
except the composer, which is the screen that rule was written for. Three of
those fourteen got a rule of their own, and the other eleven read `unknown`,
which is the right answer about a screen no rule was measured on.

## How this was read

Two sources, and they are not interchangeable.

**What exists, and what raises it,** comes off the tree beside the binary —
`packages/coding-agent/src/modes/interactive/components` in pi's own repo,
shipped as `dist/modes/interactive/components` in the npm package, one compiled
file per source file and forty-three of them. The anchor law in
`assets/screen-rules-pi.toml` says a string
recognised from a reference tree is not a measurement, and nothing here becomes
an anchor on the strength of being in it. A tree is good for one thing a pane is
bad at: telling you a screen exists before you have hit it.

**What each screen reads as** is measured. The whole Reads column was read
again on 2026-09-05, against the eight rules the document carries now: the
three it was first written against had become five more, and a column measured
against three of them was a column of stale verdicts. The rig this time was one
pi agent amx itself spawned — `amx new -- --offline -e screens.js`, in a
detached tmux session at 100 columns and 30 rows — with every screen raised in
that one pane by `send-keys` and read two ways on each capture. `amx ls --json`
is the first way and the one that decides: the state and the rule name amx put
on the row, which is the reading a person gets. The second is the same capture
put back through the document by hand, the way `Rule::holds` puts one through —
the last 24 rows (`FLOOR_LINES`) of the capture with its trailing blank rows
trimmed off, case folded, every row carrying each `all` string and any `any`
string, and whether some choice of one row per anchor spans at least `apart`
and at most `within` (until 2026-09-06 it was the topmost row carrying each,
and the notice section below is why that changed). The two agreed on every
screen in this table, which is what the second one is for: a span in the
Reads column is a number somebody can check, and the rule beside it is what
amx really said. The agent was opencode's muse-spark-1.3-contributor-free; the
readings do not depend on which model, only on the rows pi draws. `screens.js`
is a six-command extension that does nothing but raise `ctx.ui.select`,
`confirm`, `input`, `editor` and `custom`, which is the only way to reach four
of these screens.

The 0.85.1 re-read of 2026-09-06 was the same rig with three differences. The
agent was spawned `--no-extensions` beside `-e screens.js`, so amx's own
extension stayed out of the pane and the screen was the only witness, which
is the reading these rules exist for; it ran on a state directory and a tmux
socket of its own, so nothing landed on the wall; and every screen was read
at 100, 54, 40, 30, 24 and 20 columns rather than at 100 alone, since what
had moved was measured by width. Before the rig, the package on disk was
diffed against 0.84.4's: nine of the forty-three components had changed, and
the changelog named the one that mattered — the working indicator moved into
the editor's border in 0.85.0. A tree says what moved; the rig says what it
reads as. A mid-turn dialog was driven with a second extension gating the
bash tool with `ctx.ui.select`, which is the one screen where the two had to
be seen together.

Trailing blank rows are worth one sentence, because they move every span in the
table. `Server::run` ends on `trim_end`, so the rows a pane has left empty
under the box are not rows a rule may see, and the floor is counted up from the
last row with anything on it. Read a capture without trimming it and pi's own
trust screen reads `dialog` instead of `project_trust`, which is how this pass
found the trim in the first place.

`--offline` earns its place in that command line. With the update check on, pi
0.84.4 draws an Update Available box above the composer, and its border lands
inside the floor on any pane short enough to hold it there. That cost every
windowed rule its window until 2026-09-06, when a rule learned to stand on the
rows that fit rather than the topmost border it could see — see the last
section, and the table under the status lines.

Two screens were not driven, and the table says so on each row. `/share`
uploads a gist. The branch summary status line wants a branch worth
summarising, and none was arranged; the retry and compaction lines beside it
were driven on 2026-09-05 against a provider an extension registered and a
server on this machine answered, and `assets/screen-rules-pi.toml` carries that
measurement at the spinner rule. The first-run setup gate was driven too: it
wants an agent directory with no `settings.json` in it, and `HOME` pointed at
an empty directory of its own is the whole of what that takes, so nothing of
anybody's was moved to reach it.

## Where pi puts a screen

The pane is seven containers in a fixed order, built once in
`interactive-mode.js`: the document, pending messages, the status line, widgets
above, **the editor**, widgets below, the footer. Every blocking widget in the
table below is mounted the same way — `editorContainer.clear()`, then the widget
where the composer was — so it is drawn between the same two neighbours the
composer had, with the working directory and the stats line underneath it.

This is why `assets/screen-rules-pi.toml` has no `not_below` to work with, and
it holds for pi's own selectors and not just for the extension dialogs that
document measured. Two exceptions, both of them their own program rather than a
pane amx watches: `pi config` and the first-run setup build a separate TUI with
no composer and no footer in it at all.

## What reaches each rule

Eight rules, in the order the document holds them.

- **`first_time_setup`** wants `Welcome to pi,` and twenty columns of rule. One
  component draws that banner: `first-time-setup.js`, over both of its steps.
- **`project_trust`** wants `Project trust`, twenty columns of rule, one of
  `Saved decision:` or `Current session:`, and no more than seven rows between
  the first and the last of them. One component draws that title:
  `trust-selector.js`, which is what `/trust` raises. It is not what a pi
  stopped by its own startup gate draws — see finding 1 below.
- **`dialog`** wants `↑↓ navigate` and twenty columns of rule. Three components
  draw that hint row: `extension-selector.js`, `trust-selector.js` and
  `first-time-setup.js`, and the two rules above take the last two off it first.
  Everything else pi draws spells its keys some other way, and `enter submit`,
  `Enter to select · … · Escape/Ctrl+C to cancel` and `Enter to select · Esc
  to go back` are three separate other ways.
- **`editor`** wants `enter submit`, `shift+enter/ctrl+j` and twenty columns of
  rule. One component draws both terms: `extension-editor.js`.
- **`input`** wants `enter submit` and twenty columns of rule, which is
  everything the rule above stands on minus the term that tells them apart, so
  it sits under it. `extension-input.js` is what reaches it.
- **`spinner`** wants twenty columns of rule and one of the ten braille frames
  within four rows of it. `status-indicator.js` opens every one of its four
  kinds on that frame, and so does the box `bash-execution.js` puts in the
  transcript. On 0.85.1 the working kind is handed to `custom-editor.js`, which
  draws it inside the composer's top border, and the other three keep their row
  above the box. The message after the frame is not part of the rule, which is
  why the kinds that do not say `Working` are claimed too — two of them driven,
  and the fourth is the same row with a fourth message on it.
- **`login`** wants `(escape/ctrl+c to` — the `to` is what tells it from the
  three hint rows that spell the same key `escape/ctrl+c cancel`, and the
  parenthesis from the two selectors whose 0.85.1 hint row ends `Escape/Ctrl+C
  to cancel` — twenty columns of rule, and no more than six rows between them.
  `login-dialog.js` is what reaches it.
- **`prompt`** wants twenty columns of rule and one of `↑`, `$0.` or `%/`
  exactly four rows from it: `within` and `apart` are both 4, so the span is
  the box, the two footer rows, and nothing wider or narrower. That is the
  composer and only the composer.

The last one is what most of the table below comes down to, and it is worth
saying why the window is shut that tight. A rule tries every row carrying an
anchor, so a widget's own **top** border is tried when it is inside the floor
and its bottom border is tried always. A tall widget's top border spans past 4
to the stats line, and its bottom border spans 2 — three rows of screen — which
`apart` refuses. Both are `unknown`, which is the same answer, and that is the
point: the window used to be 8 with no floor under it, so the same widget read
`idle` at one span and `unknown` at the other, and which one a pane gave you
depended on how much output happened to be above it rather than on the widget.

`/thinking` is the example the first pass wrote down. On a fresh boot it spans
17 and on a few turns of scrollback it spans 2, and both readings are
`unknown` today. The widget did not move.

## The blocking widgets

Nineteen components. `Reads` is the measured verdict at 100x30 on a fresh
`--offline` boot, with the span the rule computed. Read again on 2026-09-05,
against the eight rules the document carries today, and again on 2026-09-06
against 0.85.1: the rows that moved say so.

| Component | What raises it | The row it ends in | Reads |
| --- | --- | --- | --- |
| `extension-selector.js` | `ctx.ui.select`, `ctx.ui.confirm`, pi's own `/login` method chooser, and pi's own startup trust question | `↑↓ navigate  enter select  escape/ctrl+c cancel`, then a border | **`dialog`**, waiting (span 8; 7 on the `/login` chooser). On 0.85.1 a dialog raised mid-turn has no frame anywhere above it: the working indicator is in the editor the dialog replaced |
| `trust-selector.js` | `/trust` | `↑↓ navigate  enter save  escape/ctrl+c cancel`, then a border | **`project_trust`**, waiting (span 5). 0.85.1 draws the saved decision's `✓` in front of the label, so an unsaved folder's rows read `→   Trust`; the anchors are the title and the two rows under it, which did not move |
| `first-time-setup.js` | first interactive run, with `PI_EXPERIMENTAL=1` and no `settings.json` | `↑↓ navigate`, then `enter continue` on the theme step and `enter finish` on the next, then `escape/ctrl+c skip setup`, then a border | **`first_time_setup`**, waiting (span 7 on both steps) |
| `model-selector.js` | `/model`, or `ctrl+l` | `  Enter to select · Ctrl+S to set as default · Escape/Ctrl+C to cancel`, then a border — 0.85.1 builds the row from the person's keybindings; 0.84.4 wrote `Esc to cancel` | nothing, unknown (span 2). It read **`login`** on 0.85.1 until the login rule's anchor gained its parenthesis — see finding 2 of the 0.85.1 pass |
| `thinking-selector.js` | `/thinking` with no argument | the same hint row, then a border | nothing, unknown (span 17); the same false `login` on 0.85.1, closed the same way |
| `scoped-models-selector.js` | `/scoped-models` | `enter toggle · ctrl+a all · ctrl+x clear · ...`, wrapped, then a border | nothing, unknown (span 2) |
| `session-selector.js` | `/resume`, or `app.session.resume` | the session list, then a border | nothing, unknown (span 14) |
| `tree-selector.js` | `/tree`, or `app.session.tree` | the tree, then a border | nothing, unknown (span 2) |
| `user-message-selector.js` | `/fork`, or `app.session.fork` | the message list, then a border | nothing, unknown (span 8) |
| `settings-selector.js` | `/settings` | `  Type to search · Enter/Space to change · Esc to cancel`, then a border | nothing, unknown (span 20) |
| `settings-submenu.js` | Enter on a settings row that has one (Theme, per-model thinking) | `  Enter to select · Esc to go back`, then the selector's border | nothing, unknown (span 13) |
| `oauth-selector.js` | `/login` then either method; `/logout` | the provider list, then a border — no hint row at all | nothing, unknown (span 16 on the account list, 18 on the API-key one) |
| `login-dialog.js` | `/login` through to a provider that wants a key or a code | ` (escape/ctrl+c to cancel, enter to submit)`, then a border | **`login`**, waiting (span 5) |
| `extension-input.js` | `ctx.ui.input` | ` enter submit  escape/ctrl+c cancel`, then a border | **`input`**, waiting (span 6) |
| `extension-editor.js` | `ctx.ui.editor` | ` enter submit  shift+enter/ctrl+j newline  escape/ctrl+c cancel  ctrl+g external editor`, then a border | **`editor`**, waiting (span 8) |
| `theme-selector.js` | an extension only: exported from the package, shown through `ctx.ui.custom` | the theme list, then a border | nothing, unknown (span 6) |
| `show-images-selector.js` | an extension only, the same way | the two choices, then a border | nothing, unknown (span 5) |
| `custom-editor.js` | always — it is the composer; also `ctx.ui.setEditorComponent` | the box's bottom border, with the footer under it; on 0.85.1 its top border carries the working indicator while a turn runs | **`prompt`**, idle (span 4) |
| `config-selector.js` | the `pi config` command, in its own TUI | its bottom border, and nothing under it | nothing, unknown — the screen carries no `↑`, `$0.` or `%/` for any rule to find |

Four notes the table has no column for.

`ctx.ui.custom` is not in the list because it draws whatever the extension hands
it. Both rows above that say *an extension only* were measured through it, and
what they read is a property of the component, not of `custom`. The same call
with `overlay: true` paints over the pane instead of taking the editor slot,
footer included, and no screen amx knows survives that.

`custom-editor.js` earns its row twice. It is the screen the `prompt` rule was
written for, and it is also the one thing on a pi pane that says the program
drawing it is pi — twenty columns of its border is the anchor all eight rules
stand on. An extension that replaces it through `setEditorComponent` with
anything unbordered takes that anchor off the screen, and then every rule in the
document goes quiet at every width.

The composer has one other reading, and it is where it sits on the pane rather
than what is in it. Close a widget taller than the composer and pi redraws the
box where the widget's top was, leaving the rest of the pane blank underneath:
the box, the two footer rows, and then nothing. A capture is trimmed of its
trailing blank rows before it is read, so the floor begins at the box's
**bottom** border, the top border is out of reach, and the span is 2 rather
than 4 — `unknown`, on a pi sitting at an empty prompt. Measured at 100x30 on
2026-09-05, after `/model` and after `/settings`. The next thing drawn on the
pane puts it back.

`session-selector-search.js` is in the components directory and is not in this
table: it is the search and filter functions `session-selector.js` calls, and it
draws nothing.

## The status lines

| Component | What raises it | The row it ends in | Reads |
| --- | --- | --- | --- |
| `status-indicator.js` — working | every turn, for the whole of it | on 0.85.1, `── ⠼ Working ───…`: the frame and the word inside the composer's own top border, and no row above the box (`custom-editor.js` draws it; 0.85.0 moved it there and dropped the ellipsis). On 0.84.4, `⠦ Working...` two rows above that border | **`spinner`**, working (span 2 on both — border to border on 0.85.1, driven 2026-09-06 at 100, 54, 40, 30, 24 and 20 columns; at 20 the rule after the word is seven wide and the twenty the rule asks for are on the bottom border) |
| `status-indicator.js` — retry | a provider call that failed and will be retried | `Retrying (1/3) in 5s... (escape to cancel)`, still a row of its own above the box on 0.85.1 | **`spinner`**, working — driven 2026-09-05 against a server that answers 503, at six widths, and written down at the spinner rule; not re-driven, its class is byte-identical in 0.85.1 and `interactive-mode.js` embeds only the working kind |
| `status-indicator.js` — compaction | `/compact`, and automatic compaction | `Compacting context... (escape to cancel)`, or `Auto-compacting...`, a row of its own as above | **`spinner`**, working — driven the same day and the same way, against a request held open; the same source reading at 0.85.1 |
| `status-indicator.js` — branch summary | summarising a branch | `Summarizing branch... (escape to cancel)`, a row of its own as above | not driven; it is the fourth kind of the same row, so the frame the rule anchors on is on it |
| `status-indicator.js` — idle | between turns, on a pane whose `clearOnShrink` setting is on | two blank rows where the spinner was; with the setting off, which is the default, the rows simply go | whatever the rest of the screen says |
| `footer.js` | always | the working directory, then the stats line, then one row per extension status | not a screen — it is the furniture every rule reads through, and `assets/screen-rules-pi.toml` measures it |
| `bash-execution.js` | `!cmd` and `!!cmd` | `⠧ Running... (escape/ctrl+c to cancel)` inside its own box in the transcript, with the composer still under it | **`spinner`**, working (span 3) |
| `bordered-loader.js` | `/share`, while it uploads — it takes the editor slot and the focus | `escape/ctrl+c cancel`, then a border | not driven; it uploads a gist |

The gap those four rows used to share is closed, and it is worth naming what
closed it. `spinner` anchored on `working...` when this file was written, and
`Working...` is the message of one of four kinds: the other three take the row
over rather than sharing it, so the word was off the pane for the whole of a
compaction and the whole of a retry. The rule anchors on the braille frame now,
which all four kinds draw, and a compacting pi reads `working` instead of
reading nothing.

What it cost is one screen more than the status lines, and the table says so:
`!cmd` draws its own bordered box in the transcript with a frame in it, and
that box now reads `working` too. A shell command somebody ran in the pane is
not a turn the agent is taking, and `working` is still the better of the two
answers available — the pane is busy, and what it read before was `unknown`.

Two extension calls belong here rather than in a rule. `ctx.ui.setWorkingMessage`
rewrites `Working` to anything the extension likes — inside the border on
0.85.1, since it is the working kind's message — and
`ctx.ui.setWorkingVisible(false)` takes the indicator off the pane entirely,
leaving a plain border. The first was driven on 0.84.4 and is claimed, because
the frame is what the rule reads and the message is not; the second takes the
frame off the pane, and a running turn under it reads whatever else is on the
screen. Neither was driven against a live turn on a provider that answers, and
neither was re-driven on 0.85.1; both are one call away in any extension pi
loads.

## The update notice

Drawn by `interactive-mode.js` itself, not a component: a `DynamicBorder`, a
`Text` holding `Update Available` and the instruction, the changelog line, and
a border under it, added to the chat container above the composer whenever
the update check finds a newer pi — which is most of the time. Measured
2026-09-06 on 0.84.4 with 0.85.1 out, at 120x40, 100x30, 80x24 and 40x24, with
no `--offline` on the argv, the way `amx new` starts one:

    ────────────────────────────────────────────────────────────────────────────────
     Update Available
     New version 0.85.1 is available. Run pi update
     Changelog: https://pi.dev/changelog
    ────────────────────────────────────────────────────────────────────────────────

    ────────────────────────────────────────────────────────────────────────────────

    ────────────────────────────────────────────────────────────────────────────────
    ~/Sites/tries/2026-09-04-dogfood (main)
    0.0%/1.0M (auto)               (opencode) muse-spark-1.3-contributor-free • high

Five rows at every width but 40, where the instruction wraps and it is six; a
release that ships a note adds the note's rows with a blank row either side.
`showPackageUpdateNotification` draws the same box for extension packages,
with `Package updates are available. Run pi update --extensions` where the
version line is; it was not driven, and nothing below turns on the words.
The box's borders are the composer's own, so from the notice's top border the
stats line is 10 rows off (11 at 40), and mid-turn the spinner sat two rows
above the composer as it did on any other 0.84.4 screen. Not re-driven on
0.85.1: it is the newest pi, so there is no notice to draw. What 0.85.1 changed
is under the box rather than in it — the frame is in the composer's top border
— and `tests/mock_pi/pi` draws `notice-working` that way, 0.84.4's box over
0.85.1's composer.

| Component | What raises it | The row it ends in | Reads |
| --- | --- | --- | --- |
| `interactive-mode.js` — update notice, idle | a newer pi published, on every start without `--offline` | the box, a blank row, then the composer | **`prompt`**, idle (span 4 from the composer's own border; 10 from the notice's, which is the one a rule used to stand on) |
| `interactive-mode.js` — update notice, mid-turn | the same, with a turn running | `⠦ Working...` two rows above the composer, under the box, on 0.84.4 | **`spinner`**, working (span 2) |

Both read `unknown` until 2026-09-06. `row_of` found the topmost row carrying
a string, the notice's own top border was that row, and `prompt` (within 4),
`spinner` (4), `login` (6) and `project_trust` (7) all lost their windows at
once — on a spawn, a resume and a fork, at 80x24 and 100x30, for as long as
the box stayed inside the 24 rows a rule reads. `Rule::holds` tries every row
each anchor is on now and holds when some choice of rows fits the window; the
floor `apart` still refuses a lone bottom border, since no choice of rows on
that screen spans enough. `tests/mock_pi/pi` draws both screens as `notice`
and `notice-working`, and `tests/e2e_pi.rs` reads them.

## What is not a screen

The rest of the directory. None of these blocks, and none of them says anything
about what the agent is doing:

- **Transcript rendering** — `assistant-message.js`, `user-message.js`,
  `tool-execution.js`, `diff.js`, `mermaid.js`, `skill-invocation-message.js`,
  `branch-summary-message.js`, `compaction-summary-message.js`,
  `custom-message.js`, `custom-entry.js`. Rows in the document container, above
  everything a rule reads. Not a screen is not the same as not read: these are
  the rows amx writes down as a pi turn's answer, and they have a section of
  their own below.
- **Helpers with no rows of their own** — `dynamic-border.js` (the rule every
  box is drawn with), `keybinding-hints.js` (the words in every hint row above),
  `markdown-transform.js`, `visual-truncate.js`, `index.js`,
  `session-selector-search.js`.
- **`countdown-timer.js`** — no rows either: it retitles somebody else's dialog
  once a second, which is how `ctx.ui.select` with a timeout counts down. It is
  in this list because it is the reason a dialog screen changes while nobody
  touches it.
- **Easter eggs** — `armin.js` and `daxnuts.js` (`/arminsayshi`,
  `/dementedelves`, and a model id with kimi-k2.5 in it), plus
  `earendil-announcement.js`. The first two animate on a timer in the
  transcript, so a pane holding one never holds still and no quiescent rule can
  ever settle on it. Measured, and read again on 2026-09-05: `/arminsayshi`
  reads `idle` at a span of 4, since the composer is still underneath.

Nineteen widgets, four status lines and twenty of these: that is every file in
the directory, and the point of counting them is that the next person does not
have to.

## Where pi's tools stop and pi starts

Everything above this line is about the bottom of a pane. This section is about
the rest of it, and the reason it needs one is that the rest of it is now on the
record. pi reports through no hooks and keeps no conversation amx can read back,
so a reading that ends a pi turn writes the screen to `state.result` with
`screen` beside it as the source, and what lands on the record there is
**what the agent last said** — the pane with the vendor's own furniture cut off
the bottom by `src/furniture.rs`, the same walk a card and `amx logs` print
through. Everything left over is the transcript. So: every row shape a turn
leaves above the box, what draws each one, and which of those rows is the answer
a caller asked for.

### How this was read

One pi, one tmux pane 30 rows tall, `resize-window` between captures so that the
same live screen is what each width reads: 220, 100 and 40 columns. The run was
`pi --offline --approve --no-session --no-context-files --no-extensions
--no-skills --no-themes`, in a scratch git repository holding one four-line
`notes.md` and nothing else, on opencode's muse-spark-1.3-contributor-free. The
prompt asked for `wc -l notes.md` through the bash tool, then a read of the same
file, then one sentence about it and no other tools, which is the shortest turn
that puts all four shapes on one pane. Driven 2026-09-05.

pi repaints its whole transcript on a resize, which is why the 220-column
reading has scrolled one row further than the two narrower ones: it is the pane
as the turn left it, and they are the pane redrawn from pi's own buffer. The
rows themselves are the same rows.

Captures are taken the way `src/tmux.rs` takes one: `capture-pane -p -J`,
control and format characters replaced with a space, trailing whitespace off the
end of the whole capture. The rows below are those captures with trailing spaces
stripped and nothing else done to them. Where colour is the point, a second
capture was taken with `-e` on the same pane and the two were lined up row by
row — `-e` is exactly what amx does not pass, and a good deal of this section is
about what that costs.

The renderers were read off the tree beside the binary, and one correction
belongs before the captures rather than after them: three of the four shapes are
not drawn in `components/` at all. `components/tool-execution.js` is the
component a tool call is mounted in and it draws none of the words — it asks the
tool's own definition for a `renderCall` and a `renderResult`, and for pi's
built-in tools those live in `core/tools/`. Only the agent's prose is drawn
where this document has been looking, by `components/assistant-message.js`.
Nothing here is an anchor on the strength of being in either tree; the rows are
the measurement, and the renderers say which row is whose.

### The four shapes

The whole 100-column capture, thirty rows:

     pi v0.84.4
     escape interrupt · ctrl+c/ctrl+d clear/exit · / commands · ! bash · ctrl+o more
     Press ctrl+o to show full startup help and loaded resources.

     Pi can explain its own features and look up its docs. Ask it how to use or extend Pi.


     Run: wc -l notes.md using the bash tool. Then read notes.md. Then say in one short sentence what
     the file describes. Use no other tools.



     $ wc -l notes.md

     4 notes.md

     Took 0.0s



     read notes.md


     The file describes recent changes to caching and timeout configuration.

    ────────────────────────────────────────────────────────────────────────────────────────────────────

    ────────────────────────────────────────────────────────────────────────────────────────────────────
    ~/.claude/jobs/c9abca70/tmp/rig/work (main)
    ↑2.1k ↓386 R3.9k CH93.1% 0.2%/1.0M (auto)          (opencode) muse-spark-1.3-contributor-free • high

Rows 1 to 5 are pi's own startup banner. Rows 8 and 9 are the prompt somebody
typed. Rows 13, 15 and 17 are the bash call, its output and the line it ends on;
row 21 is the read; row 24 is the agent. Rows 26 to 30 are the furniture the
rest of this document measures.

The same screen at 40 columns, where the prompt and the answer both wrap:

     look up its docs. Ask it how to use or
     extend Pi.


     Run: wc -l notes.md using the bash
     tool. Then read notes.md. Then say in
     one short sentence what the file
     describes. Use no other tools.



     $ wc -l notes.md

     4 notes.md

     Took 0.0s



     read notes.md


     The file describes recent changes to
     caching and timeout configuration.

    ────────────────────────────────────────

    ────────────────────────────────────────
    ~/.claude/jobs/c9abca70/tmp/rig/work ...
    ↑2.1k ↓386 R3.9k CH93.1% 0.2%/1.0M (a...

At 220 columns the four shapes are the same four strings and nothing wraps: the
prompt is one row rather than two, the answer is one row rather than two, and
the box's two borders and the two footer rows run the full 220 columns, which is
why they are not reproduced here. The transcript rows are ` $ wc -l notes.md`,
` 4 notes.md`, ` Took 0.0s`, ` read notes.md` and ` The file describes recent
changes to caching and timeout configuration.` — byte for byte what the two
narrower captures carry, leading space included.

So the four shapes, at every width driven:

| Shape | The row | What draws it |
| --- | --- | --- |
| the bash tool's own row | ` $ wc -l notes.md` | `core/tools/bash.js`, `formatShellCall` |
| the line the call ends on | ` Took 0.0s` | `core/tools/bash.js`, `rebuildBashResultRenderComponent` |
| a read's row | ` read notes.md` | `core/tools/read.js`, `formatReadCall` |
| the agent's prose | ` The file describes recent changes to caching and timeout configuration.` | `components/assistant-message.js` |

And what each of those renderers says about the row beside it.

**The bash row** is `${prompt} ${command}`, and `prompt` is `"$"` — one
character, from the tool's own config. A `timeout` argument adds a muted
` (timeout Ns)` after the command, which the second turn of this session drew:
` $ sleep 25; echo finished (timeout 35s)`. There is a second shell tool with
the same code behind it and `PS>` for its prompt, `core/tools/powershell.js`,
not driven here. Every other built-in opens its call row with its own name in
lower case instead — `read`, `edit`, `write`, `grep`, `find`, `ls` — and so does
an extension tool that ships no `renderCall` of its own, because
`createCallFallback` in `tool-execution.js` draws the bare tool name.

**`Took`** is the shell tools' line and not something every call ends with,
which is worth saying plainly because the name suggests otherwise. It is one
`addChild` at the end of `rebuildBashResultRenderComponent`, the duration is
`(ms / 1000).toFixed(1)` and the label is the one word that changes while the
call is in flight: `Elapsed` where `isPartial`, `Took` where it is not. Driven
at 100 columns on a `sleep 25` — ` Elapsed 11.0s` mid-call, ` Took 25.0s` after.
No other tool in `core/tools/` draws either word.

**A read's row is the whole of a read.** `formatReadResult` returns an empty
string unless the block has been expanded or the call errored, so a successful
read leaves one row on the pane and none of the file it read. That is the
capture above: ` read notes.md`, and no line of `notes.md` anywhere on the
screen at any of the three widths. It is the one shape here that is shorter than
a reader would guess, and the guess is the dangerous direction: an answer read
off a pane is short of a read's output whether or not anybody knows it.

**The agent's prose** is a `Markdown` with a padding of one column, a padding of
zero rows, and a `Spacer(1)` in front of it. No glyph, no label, no border,
nothing at the head of the row. That is the shape, and the next two sections are
what it costs.

### Nothing on the pane says which of them is which

The four shapes share one leading space and carry nothing else at the head of a
row that a rule could stand on. What tells them apart is colour, and colour is
what `capture-pane -p -J` throws away. A second capture with `-e` on the same
pane, at the same width, lined up against the first row for row:

| Row | What it is | What the `-e` capture carries |
| --- | --- | --- |
| 7 to 10 | the box the typed prompt is drawn in | background `48;2;33;34;47`, opened on row 7 and still in force at row 10 |
| 8, 9 | the prompt itself | `38;2;205;214;244` |
| 12 to 18 | the bash block | background `48;2;39;47;49`, opened on row 12 |
| 13 | ` $ wc -l notes.md` | bold, `38;2;137;180;250` |
| 15 | ` 4 notes.md` | `38;2;205;214;244` |
| 17 | ` Took 0.0s` | `38;2;143;149;173` |
| 20 to 22 | the read block | the same background as the bash block |
| 21 | ` read notes.md` | `read` bold and `38;2;137;180;250`, the path the same colour unbolded |
| 24 | ` The file describes recent changes…` | `49`, and no foreground code at all |

Three things in that table are worth having. The agent's own sentence is the one
row shape on a pi pane with no colour of its own: it is drawn in the terminal's
default foreground, which is the one colour a theme has nothing to say about.
The row a tool's output lands on carries `38;2;205;214;244`, which is the same
paint the person's typed prompt carries — so even the capture amx does not take
would not tell those two apart. And a tool block's *extent* is a background and
nothing else: rows 12 and 18 are blank on the plain capture and are the vendor's
own padding, while rows 11 and 19 are blank and belong to nobody. Eleven of the
twenty-four rows on the record here have anything on them at all, and which of
the other thirteen are furniture is knowable only from the capture that was
thrown away.

The one column of indent is not a marker either, and it is a setting rather than
a fact about the vendor. `getOutputPad` in `core/settings-manager.js` gives 0
where `outputPad` is exactly 0 and 1 for everything else including a setting
nobody has written, and it reads the merge of the global settings with a
project-local `.pi/settings.json`. `tool-execution.js` does not consult it: the
box a tool block is drawn in is `Box(1, 1, …)` with the 1 written in. Driven at
100 columns with `{"outputPad": 0}` in the scratch repository's own
`.pi/settings.json`, so nothing of anybody's was moved. It is a fresh pi given
the same prompt rather than the screen above, so the sentence it ends on is the
model's own and not the setting's:

    Run: wc -l notes.md using the bash tool. Then read notes.md. Then say in one short sentence what the
    file describes. Use no other tools.



     $ wc -l notes.md

     4 notes.md

     Took 0.0s



     read notes.md


    The file describes release notes for cache write-through eviction and configurable timeouts.

The prose and the typed prompt lose the column; every tool row keeps it. Which
is the inversion worth writing down: on the default the indent is on all four
shapes and tells nobody anything, and on the one other setting the only indented
rows left are the vendor's.

Two more kinds of row have no marker and are not the agent answering. A tool row
too long for the pane wraps at its width less two, and the continuation carries
the same one space and none of the head that named it: the timeout row quoted
above, captured again at 40 columns, reads ` $ sleep 25; echo finished (timeout`
and then ` 35s)`. And pi draws a model's thinking through the same
`assistant-message.js` in the same column, told from an answer by italics and
`38;2;110;114;135` and by nothing else at all.

### The reading that would tell them apart

Every marker above is the agent's to write or the person's to switch off, so
what goes on the record is the whole cut pane, and the boundary is measured
rather than built: ruling #QQPQW2VZ, taken with the rows above in front of it.
The one thing that would carry it is the capture those rows were read with,
`capture-pane -e`, and nobody has taken it. What it would cost is written down
here so that the next person deciding weighs it against the measurement rather
than driving pi again for one.

The walk is not what is missing. `Server::capture_painted` already takes a `-e`
capture, for the card, and `src/ansi.rs` already turns one into runs of text
with the paint each was drawn in. Three other things are.

**Every reading would have to take it.** `derive` matches the rules against one
capture and writes the answer out of that same string, and a wall asks tmux for
every screen on it in one invocation (`Server::captures`). So a paint-carrying
reading is either the whole reader moved onto `-e`, with an escape walk in front
of every rule match and in front of the hash a quiescent rule's stillness is
kept as, or a second capture of every pane on every look.

**The anchor would be an absence.** The agent's prose is the one row shape here
with no foreground code on it at all, which is the one colour on the pane a
theme has nothing to say about — but that was read off one sentence of plain
prose, and `assistant-message.js` draws a `Markdown`. Whether the absence
survives an answer with a heading, a bold word or an inline code span in it is
not something this pass drove, and an anchor nobody measured is a guess wearing
the format of a fact.

**And a pane still holds one screen.** A boundary would separate the vendor's
rows from the agent's on whatever is up when somebody looks, which is not the
same as separating this turn's rows from the last turn's: the section below is
that problem, and no capture flag moves it.

### Which rows are the answer a caller asked for

`Ruleset::claim` reads all three captures as `prompt`, idle, which is the state
in which the screen is written down at all. The walk then cuts five rows off the
bottom of each — the box's top border, the row kept for what is staged in it,
the bottom border, the working directory, the stats line — and one blank row
comes off the end with them:

| Width | Rows on the pane | Cut | On the record | With anything on them | The answer asked for |
| --- | --- | --- | --- | --- | --- |
| 220 | 28 | 5 | 22 | 9 | row 22, one row |
| 100 | 30 | 5 | 24 | 11 | row 24, one row |
| 40 | 30 | 5 | 24 | 12 | rows 23 and 24 |

Five rows at every width, and not one of them from the transcript: the walk is
exact about where the furniture begins and says nothing at all about where the
answer does. So the answer a caller asked for is one row of twenty-two at 220
columns, one of twenty-four at 100, and two of twenty-four at 40. Taking the
middle one apart: thirteen rows blank, four rows of pi's startup banner, two
rows of the prompt somebody typed, four rows of the two tool blocks, and the
answer. A reader with only the record in front of them has no way to say which
was which.

The turn after it is where that bites, and it was driven rather than reasoned
about. A later turn on the same pane — `sleep 25` through the bash tool and an
instruction to say nothing else — read at 100 columns once it had ended. This is
the record's twenty-two rows rather than the pane's thirty: the five furniture
rows are already off the bottom.

     $ sleep 6; echo done

     done

     Took 6.0s



     Now run: sleep 25; echo finished  with the bash tool. Say nothing else.



     $ sleep 25; echo finished (timeout 35s)

     finished

     Took 25.0s


     Following the instruction to say nothing after the tool completes, with no further text.

     Analyzing the post-tool turn to confirm no user-visible output is required.

Twenty-eight rows on the pane, five cut, twenty-two on the record — and none of
them is an answer. The model wrote no assistant text at all: the last two rows
are the thinking the section above measures, and the first five rows are the
tail of the turn before it.

When this was driven, every reading that found a finished screen wrote it down,
so this picture landed where the answer of the turn that did say something had
been and `amx result` handed a caller two other turns. A screen reaches the
record now only where a reading watched the turn end on it, which is the one
thing that ever ties a screen to a turn: a look arriving at this pane with the
record already idle leaves the earlier answer where it is. What a reader that
does watch this turn end writes down is still these twenty-two rows. There is no
other account of that turn and there never will be, and what a `source` reading
`screen` is worth is exactly this much.

None of this is an argument for cutting more. The walk's own law is that a wrong
number costs furniture left on the screen and never a row of work taken off it,
and every marker measured above — the `$`, the tool's name, the indent, the
colour — is either the agent's to write or the person's to switch off. What the
measurement is for is knowing what the record means: a `source` reading `screen`
is amx saying it looked at a picture, and this section is how much of the
picture is in there.

## What the first pass found, and what answered it

Four findings, and three of them are closed. They are kept here because the
next reader wants to know which parts of the document were written to a
measurement and which were inherited.

1. **`↑↓ navigate` covers the extension dialogs and almost nothing else.** A
   permission gate written with `ctx.ui.input` instead of `ctx.ui.select` was
   invisible. Closed: `input` and `editor` are rules of their own, and both
   screens read `waiting` with the caller's own title as the question.
2. **Fourteen screens that need a person read `idle` or `unknown`,** and `idle`
   was the worse of the two. Closed: none of the fourteen reads `idle` now.
3. **The prompt rule's `within = 8` is doing work nobody measured it for.**
   Closed: `within` is 4 and `apart` is 4, which is the box and the two footer
   rows and nothing else, so a widget is not a prompt at any height.
4. **A boot screen with pi's own update notice on it reads `unknown`.** Closed
   2026-09-06 in the matcher rather than the document — see below, and the
   update notice's own section above.

## What this pass found

Driven on 2026-09-05 against real pi 0.84.4 through amx itself, spawning,
resuming, forking and adopting agents on a scratch repository. Two findings,
both about where a box sits on the pane rather than about what is in it.

1. **pi's own startup trust gate is not the screen `project_trust` was measured
   off.** `/trust` draws `trust-selector.js`: the title `Project trust`, the
   saved decision and the current session under it, three choices, and
   `enter save` in the hint row. A pi started in a folder it has no decision
   for draws something else — `core/project-trust.ts` builds the question as a
   string and hands it to `ctx.ui.select`, so what a spawned agent stops on is
   `extension-selector.js` with pi's own words in it. Measured on an agent
   `amx new` started in a worktree carrying a `.pi/settings.json`, at 80
   columns, where the folder wraps:

       ────────────────────────────────────────────────────────

        Trust project folder?
        /home/saiful/.claude/jobs/27a6b683/tmp/dogfood/scratch/.amx/worktrees/wait-her
        e-tco

        This allows pi to load .pi settings and resources, install missing project
        packages, and execute project extensions.

        → Trust
          Trust parent folder
       (/home/saiful/.claude/jobs/27a6b683/tmp/dogfood/scratch/.amx/worktrees)
          Trust (this session only)
          Do not trust
          Do not trust (this session only)

        ↑↓ navigate  enter select  escape/ctrl+c cancel

       ────────────────────────────────────────────────────────

   with the working directory and the stats line under that border, as under
   any other dialog pi draws.

   No `Project trust`, no `Saved decision:`, five choices rather than three,
   and `enter select` where the other screen says `enter save`. So
   `project_trust` walks past it and `dialog` claims it: `waiting`, and
   `question` where the kind should be `trust`. What that costs is everything
   keyed on the kind — `doctor` names a claude agent on `folder_trust` and says
   nothing about the pi sitting beside it, because `setup` is a flag on the
   rule and `dialog` does not carry it, and the offer of the trust key that
   goes with it never comes. The question the row shows is the sentence above
   the arrow, which on this screen is the consequence rather than the question:
   *This allows pi to load .pi settings and resources…*.
2. **The update box takes every windowed rule off the pane.** The first pass
   measured the idle half of this. The whole of it is worse: `row_of` finds the
   topmost row carrying a string, the box's own top border is that row, and
   every rule that says how far its anchor may sit from the border loses its
   window at once. `prompt` (4), `spinner` (4), `login` (6) and `project_trust`
   (7) all go quiet, so a pi with the notice up reads `unknown` idle *and*
   mid-turn, and its trust screen falls past two rules instead of one. amx
   spawns pi with no `--offline` on the argv, so this is what a real spawn
   looks like until the transcript pushes the box off — measured on a spawn, on
   a resume and on a fork, at 80x24 and at 100x30. `assets/screen-rules-pi.toml`
   is measured `--offline` throughout and the box is on no capture in it.

   Closed 2026-09-06, and not with a rule: the box's borders are pi's own and
   identical to the composer's, so nothing textual tells them apart. The
   matcher changed instead — `Rule::holds` tries every row each anchor is on
   and holds when some choice of rows fits the window, so the composer's own
   border is found four rows above the stats line whatever sits above it. The
   box is measured at four sizes in its own section above, the stand-in draws
   it, and a fresh pi under it reads `idle` and `working` by `prompt` and
   `spinner`, at every size.

## What the 0.85.1 pass found

Driven on 2026-09-06 against real pi 0.85.1, the day it went on this machine,
with the rig the *How this was read* section describes. Three findings about
pi and one about amx.

1. **The working indicator is in the composer's top border.** 0.85.0 moved it
   there and dropped the ellipsis: `── ⠼ Working ───…`, with no row above the
   box. The spinner rule held at every width without a change — the frame is
   on the border row and the bottom border is the twenty columns of rule, two
   rows under it — and the furniture walk takes the border by the rule it ends
   in, word and all. What the change reaches is everything measured on the old
   shape: the stand-in's `working`, `notice-working`, `renamed` and `dialog`
   screens, and the spinner and furniture comments in
   `assets/screen-rules-pi.toml`. Compaction, retry and branch summary keep a
   row of their own above the box, by 0.85.1's own source and not re-driven.
   A dialog raised mid-turn carries no frame at all, driven at 100 and 20
   columns: the indicator is in the editor the dialog replaced. Closed in the
   stand-in and the comments.
2. **`/model` and `/thinking` read as the login gate.** 0.85.1 builds those two
   selectors' hint row from the person's keybindings and spells the cancel key
   out in full, `Escape/Ctrl+C to cancel`, where 0.84.4 wrote `Esc to cancel`.
   That row carries `escape/ctrl+c to`, the login rule's anchor, so a person
   picking a model was an agent stopped at a setup gate: `doctor` named it and
   `send` refused. `/settings` and `/scoped-models` spell their keys some other
   way and read `unknown` as before. Closed in the anchor: the login dialog
   wraps its whole hint in parentheses on every shape it draws and no other
   hint row pi draws opens with one, so the rule stands on `(escape/ctrl+c to`,
   whole at all six widths. The 0.85.1 model selector and login dialog are
   captures in `src/rules.rs`.
3. **The trust selector's mark moved.** The saved decision's `✓` is drawn in
   front of the label rather than after it, so a folder with no decision reads
   `→   Trust`. The title and the two rows under it are the anchors, and none
   of them moved; `project_trust` still claims the screen at a span of 5.
4. **A hookless pi's later waiting screens do not replace the recorded
   question.** Every waiting screen after the startup gate — `/sel`, `/input`,
   `/editor`, `/trust`, `/login` — kept the gate's sentence in `amx status
   --json` while state and rule moved with each screen; a fresh record whose
   first waiting screen was the mid-turn dialog showed the right one.
   `derive::write_the_reading` writes a reading on a phase edge only, and
   waiting to waiting is not an edge. This is amx's and not pi's, 0.84.4
   behaved the same, and it is open as finding #SX6QK58A.
