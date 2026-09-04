# Every screen pi 0.84.4 draws

`assets/screen-rules-pi.toml` has eight rules in it, and each one was written
from a screen somebody sat in front of. This file is the other half of that
sitting: the whole list of screens the vendor can put on a pane, and what each
one reads as today. It is an inventory and not a second ruleset — no anchor in
it, and nothing here changes what amx recognises. What it is for is knowing
which screens the eight rules cover and which they walk past, so the next pass
at that document starts from a list rather than from whatever screen somebody
happened to hit.

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
trimmed off, case folded, the topmost row carrying each `all` string, the
topmost row carrying any `any` string, and the span between the first and last
of those against `within` and `apart`. The two agreed on every screen in this
table, which is what the second one is for: a span in the Reads column is a
number somebody can check, and the rule beside it is what amx really said. The
agent was opencode's muse-spark-1.3-contributor-free; the readings do not depend
on which model, only on the rows pi draws. `screens.js` is a six-command
extension that does nothing but raise `ctx.ui.select`, `confirm`, `input`,
`editor` and `custom`, which is the only way to reach four of these screens.

Trailing blank rows are worth one sentence, because they move every span in the
table. `Server::run` ends on `trim_end`, so the rows a pane has left empty
under the box are not rows a rule may see, and the floor is counted up from the
last row with anything on it. Read a capture without trimming it and pi's own
trust screen reads `dialog` instead of `project_trust`, which is how this pass
found the trim in the first place.

`--offline` earns its place in that command line. With the update check on, pi
0.84.4 draws an Update Available box above the composer, and its border lands
inside the floor. That is not a quirk of a test rig, and what it costs is worse
than the first pass recorded — see the last section.

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
  `Enter to select · Esc to cancel` and `Enter to select · Esc to go back` are
  three separate other ways.
- **`editor`** wants `enter submit`, `shift+enter/ctrl+j` and twenty columns of
  rule. One component draws both terms: `extension-editor.js`.
- **`input`** wants `enter submit` and twenty columns of rule, which is
  everything the rule above stands on minus the term that tells them apart, so
  it sits under it. `extension-input.js` is what reaches it.
- **`spinner`** wants twenty columns of rule and one of the ten braille frames
  within four rows of it. `status-indicator.js` opens every one of its four
  kinds on that frame, and so does the box `bash-execution.js` puts in the
  transcript. The message after it is not part of the rule, which is why the
  kinds that do not say `Working...` are claimed too — two of them driven, and
  the fourth is the same row with a fourth message on it.
- **`login`** wants `escape/ctrl+c to` — the `to` is what tells it from the
  three hint rows that spell the same key `escape/ctrl+c cancel` — twenty
  columns of rule, and no more than six rows between them. `login-dialog.js` is
  what reaches it.
- **`prompt`** wants twenty columns of rule and one of `↑`, `$0.` or `%/`
  exactly four rows from it: `within` and `apart` are both 4, so the span is
  the box, the two footer rows, and nothing wider or narrower. That is the
  composer and only the composer.

The last one is what most of the table below comes down to, and it is worth
saying why the window is shut that tight. `row_of` finds the topmost row
carrying a string, so the span runs from the widget's own **top** border — if
that border is inside the floor — down to the stats line. A tall widget puts
its top border in reach and the span blows past 4. A widget taller than the
floor has that border out of reach entirely, and then the topmost border left
to find is the bottom one: three rows of screen and a span of 2. Both are now
`unknown`, which is the same answer, and that is the point: the window used to
be 8 with no floor under it, so the same widget read `idle` at one span and
`unknown` at the other, and which one a pane gave you depended on how much
output happened to be above it rather than on the widget.

`/thinking` is the example the first pass wrote down. On a fresh boot it spans
17 and on a few turns of scrollback it spans 2, and both readings are
`unknown` today. The widget did not move.

## The blocking widgets

Nineteen components. `Reads` is the measured verdict at 100x30 on a fresh
`--offline` boot, with the span the rule computed. Read again on 2026-09-05,
against the eight rules the document carries today.

| Component | What raises it | The row it ends in | Reads |
| --- | --- | --- | --- |
| `extension-selector.js` | `ctx.ui.select`, `ctx.ui.confirm`, pi's own `/login` method chooser, and pi's own startup trust question | `↑↓ navigate  enter select  escape/ctrl+c cancel`, then a border | **`dialog`**, waiting (span 8; 7 on the `/login` chooser) |
| `trust-selector.js` | `/trust` | `↑↓ navigate  enter save  escape/ctrl+c cancel`, then a border | **`project_trust`**, waiting (span 5) |
| `first-time-setup.js` | first interactive run, with `PI_EXPERIMENTAL=1` and no `settings.json` | `↑↓ navigate`, then `enter continue` on the theme step and `enter finish` on the next, then `escape/ctrl+c skip setup`, then a border | **`first_time_setup`**, waiting (span 7 on both steps) |
| `model-selector.js` | `/model`, or `ctrl+l` | `  Enter to select · Ctrl+S to set as default · Esc to cancel`, then a border | nothing, unknown (span 2) |
| `thinking-selector.js` | `/thinking` with no argument | the same hint row, then a border | nothing, unknown (span 17) |
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
| `custom-editor.js` | always — it is the composer; also `ctx.ui.setEditorComponent` | the box's bottom border, with the footer under it | **`prompt`**, idle (span 4) |
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
| `status-indicator.js` — working | every turn, for the whole of it | `⠦ Working...`, two rows above the composer's top border | **`spinner`**, working (span 2) |
| `status-indicator.js` — retry | a provider call that failed and will be retried | `Retrying (1/3) in 5s... (escape to cancel)` | **`spinner`**, working — driven 2026-09-05 against a server that answers 503, at six widths, and written down at the spinner rule |
| `status-indicator.js` — compaction | `/compact`, and automatic compaction | `Compacting context... (escape to cancel)`, or `Auto-compacting...` | **`spinner`**, working — driven the same day and the same way, against a request held open |
| `status-indicator.js` — branch summary | summarising a branch | `Summarizing branch... (escape to cancel)` | not driven; it is the fourth kind of the same row, so the frame the rule anchors on is on it |
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
rewrites `Working...` to anything the extension likes, and
`ctx.ui.setWorkingVisible(false)` takes the row off the pane entirely. The first
was driven and is claimed, because the frame is what the rule reads and the
message is not; the second takes the frame off the pane with the row, and a
running turn under it reads whatever else is on the screen. Neither was driven
against a live turn on a provider that answers; both are one call away in any
extension pi loads.

## What is not a screen

The rest of the directory. None of these blocks, and none of them says anything
about what the agent is doing:

- **Transcript rendering** — `assistant-message.js`, `user-message.js`,
  `tool-execution.js`, `diff.js`, `mermaid.js`, `skill-invocation-message.js`,
  `branch-summary-message.js`, `compaction-summary-message.js`,
  `custom-message.js`, `custom-entry.js`. Rows in the document container, above
  everything a rule reads.
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
4. **A boot screen with pi's own update notice on it reads `unknown`.** Open,
   and worse than it was written — see below.

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
