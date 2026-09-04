# Every screen pi 0.84.4 draws

`assets/screen-rules-pi.toml` has three rules in it, and each one was written
from a screen somebody sat in front of. This file is the other half of that
sitting: the whole list of screens the vendor can put on a pane, and what each
one reads as today. It is an inventory and not a second ruleset — no anchor in
it, and nothing here changes what amx recognises. What it is for is knowing
which screens the three rules cover and which they walk past, so the next pass
at that document starts from a list rather than from whatever screen somebody
happened to hit.

The short version: nineteen of pi's components draw something that waits for a
keystroke. Three are drawn in a shape the `dialog` rule can see. One is the
composer, which is the screen `idle` was written for, and one is a separate
program. The remaining fourteen stop for a person and read `idle` or `unknown`,
which are both amx saying nobody is waiting.

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

**What each screen reads as** is measured. One fresh `pi --offline -e
screens.js` per screen in a detached tmux session at 100 columns and 30 rows on
2026-09-04, driven with `send-keys`, captured with `capture-pane -p -J` — the
call `src/tmux.rs` makes — and each capture put through the three rules the way
`Rule::holds` puts one through: the last 24 rows (`FLOOR_LINES`), case folded, the
topmost row carrying each `all` string, the topmost row carrying any `any`
string, and the span between the first and last of those against `within`. The
agent was opencode's muse-spark-1.3-contributor-free; the readings do not depend
on which model, only on the rows pi draws. `screens.js` is a seven-command
extension that does nothing but raise `ctx.ui.select`, `confirm`, `input`,
`editor` and `custom`, which is the only way to reach four of these screens.

`--offline` earns its place in that command line. With the update check on, pi
0.84.4 draws an Update Available box above the composer, and its border lands
inside the floor: the same idle screen reads `idle` at a span of 4 offline and
`unknown` at a span of 10 with the banner up. That is not a quirk of a test rig.
0.85.0 shipped, so every pi 0.84.4 booted on a machine with a network draws that
box today, and every one of them reads `unknown` until it scrolls off.

Five screens were not driven, and the table says so on each row. The first-run
setup gate refuses to run against any agent directory but the real one, so
reaching it means moving somebody's `settings.json`. `/share` uploads a gist.
The retry, compaction and branch summary status lines want a provider that
fails, a session big enough to compact and a branch worth summarising, and none
of the three was arranged.

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

- **`dialog`** wants `↑↓ navigate` and twenty columns of rule. Three components
  draw that hint row: `extension-selector.js`, `trust-selector.js` and
  `first-time-setup.js`. Everything else pi draws spells its keys some other
  way, and `enter submit`, `Enter to select · Esc to cancel` and `Enter to
  select · Esc to go back` are three separate other ways.
- **`spinner`** wants `working...`. One component draws it:
  `status-indicator.js`, and only in its `working` kind. Its other three kinds
  say `Retrying`, `Compacting context` and `Summarizing branch`.
- **`prompt`** wants twenty columns of rule and one of `↑`, `$0.` or `%/`
  within eight rows of it. This is where every other screen lands, and whether
  it holds is not a fact about the widget. `row_of` finds the topmost row
  carrying a string, so the span runs from the widget's own **top** border — if
  that border is inside the floor — down to the stats line. A tall widget puts
  its top border in reach and the span blows past 8: `unknown`. A short one, or
  a long transcript that pushes the top border out of the floor, leaves only the
  bottom border in reach: span 2, `idle`.

That last one is worth saying plainly, because it is what most of the table
below comes down to. The same widget reads both ways. `/thinking` on a fresh
boot reads `unknown` at a span of 17; the same selector opened after a few turns
of scrollback reads `idle` at a span of 2, because its top border has left the
floor. Neither reading is right — a person is being asked to pick something —
and which one a pane gives you depends on how much output is above it.

## The blocking widgets

Nineteen components. `Reads` is the measured verdict at 100x30 on a fresh
`--offline` boot, with the span the rule computed.

| Component | What raises it | The row it ends in | Reads |
| --- | --- | --- | --- |
| `extension-selector.js` | `ctx.ui.select`, `ctx.ui.confirm`, and pi's own `/login` method chooser | `↑↓ navigate  enter select  escape/ctrl+c cancel`, then a border | **`dialog`**, waiting (span 8) |
| `trust-selector.js` | `/trust` | `↑↓ navigate  enter save  escape/ctrl+c cancel`, then a border | **`dialog`**, waiting (span 12) |
| `first-time-setup.js` | first interactive run, with `PI_EXPERIMENTAL=1` and no `settings.json` | `↑↓ navigate`, then `enter continue` on the theme step and `enter finish` on the next, then `escape/ctrl+c skip setup`, then a border | not driven; carries both `dialog` anchors |
| `model-selector.js` | `/model`, or `ctrl+l` | `  Enter to select · Ctrl+S to set as default · Esc to cancel`, then a border | `prompt`, idle (span 2) |
| `thinking-selector.js` | `/thinking` with no argument | the same hint row, then a border | nothing, unknown (span 17) |
| `scoped-models-selector.js` | `/scoped-models` | `enter toggle · ctrl+a all · ctrl+x clear · ...`, wrapped, then a border | `prompt`, idle (span 2) |
| `session-selector.js` | `/resume`, or `app.session.resume` | the session list, then a border | nothing, unknown (span 12) |
| `tree-selector.js` | `/tree`, or `app.session.tree` | the tree, then a border | `prompt`, idle (span 2) |
| `user-message-selector.js` | `/fork`, or `app.session.fork` | the message list, then a border | `prompt`, idle (span 8) |
| `settings-selector.js` | `/settings` | `  Type to search · Enter/Space to change · Esc to cancel`, then a border | nothing, unknown (span 20) |
| `settings-submenu.js` | Enter on a settings row that has one (Theme, per-model thinking) | `  Enter to select · Esc to go back`, then the selector's border | nothing, unknown (span 13) |
| `oauth-selector.js` | `/login` then Sign in with an account; `/logout` | the provider list, then a border — no hint row at all | nothing, unknown (span 16) |
| `login-dialog.js` | `/login` through to a provider that wants a key or a code | ` (escape/ctrl+c to cancel, enter to submit)`, then a border | `prompt`, idle (span 8) |
| `extension-input.js` | `ctx.ui.input` | ` enter submit  escape/ctrl+c cancel`, then a border | nothing, unknown (span 10) |
| `extension-editor.js` | `ctx.ui.editor` | ` enter submit  shift+enter/ctrl+j newline  escape/ctrl+c cancel  ctrl+g external editor`, then a border | nothing, unknown (span 12) |
| `theme-selector.js` | an extension only: exported from the package, shown through `ctx.ui.custom` | the theme list, then a border | `prompt`, idle (span 6) |
| `show-images-selector.js` | an extension only, the same way | the two choices, then a border | `prompt`, idle (span 5) |
| `custom-editor.js` | always — it is the composer; also `ctx.ui.setEditorComponent` | the box's bottom border, with the footer under it | `prompt`, idle (span 4), which is the one reading here that is correct |
| `config-selector.js` | the `pi config` command, in its own TUI | its bottom border, and nothing under it | nothing, unknown — the screen carries no `↑`, `$0.` or `%/` for any rule to find |

Three notes the table has no column for.

`ctx.ui.custom` is not in the list because it draws whatever the extension hands
it. Both rows above that say *an extension only* were measured through it, and
what they read is a property of the component, not of `custom`. The same call
with `overlay: true` paints over the pane instead of taking the editor slot,
footer included, and no screen amx knows survives that.

`custom-editor.js` earns its row twice. It is the screen the `prompt` rule was
written for, and it is also the one thing on a pi pane that says the program
drawing it is pi — twenty columns of its border is the anchor all three rules
stand on. An extension that replaces it through `setEditorComponent` with
anything unbordered takes that anchor off the screen, and then every rule in the
document goes quiet at every width.

`session-selector-search.js` is in the components directory and is not in this
table: it is the search and filter functions `session-selector.js` calls, and it
draws nothing.

## The status lines

| Component | What raises it | The row it ends in | Reads |
| --- | --- | --- | --- |
| `status-indicator.js` — working | every turn, for the whole of it | `⠦ Working...`, two rows above the composer's top border | **`spinner`**, working (span 2) |
| `status-indicator.js` — retry | a provider call that failed and will be retried | `Retrying (1/3) in 5s... (escape to cancel)` | not driven; carries no `working...`, so `spinner` cannot claim it |
| `status-indicator.js` — compaction | `/compact`, and automatic compaction | `Compacting context... (escape to cancel)`, or `Auto-compacting...` | not driven; same gap |
| `status-indicator.js` — branch summary | summarising a branch | `Summarizing branch... (escape to cancel)` | not driven; same gap |
| `status-indicator.js` — idle | between turns, on a pane whose `clearOnShrink` setting is on | two blank rows where the spinner was; with the setting off, which is the default, the rows simply go | whatever the rest of the screen says |
| `footer.js` | always | the working directory, then the stats line, then one row per extension status | not a screen — it is the furniture every rule reads through, and `assets/screen-rules-pi.toml` measures it |
| `bash-execution.js` | `!cmd` and `!!cmd` | `⠧ Running... (escape/ctrl+c to cancel)` inside its own box in the transcript, with the composer still under it | nothing, unknown (span 10) |
| `bordered-loader.js` | `/share`, while it uploads — it takes the editor slot and the focus | `escape/ctrl+c cancel`, then a border | not driven; carries no `working...` and no `↑↓ navigate` |

The three undriven `status-indicator` kinds are the same gap stated three times,
and it is worth naming what it costs. Each replaces the working indicator, so
`Working...` is off the pane while they are up. A compacting pi is doing work
nobody can interrupt usefully, and it reads `idle` from a record with nothing
outstanding. From a record that says a turn is running it reads nothing at all,
because these indicators tick — a screen that ticks never holds still, so the
quiescent gate on the `prompt` rule never opens and the turn is not ended by
mistake. Wrong in the quiet direction, which is the direction that document is
built to be wrong in.

Two extension calls belong here rather than in a rule. `ctx.ui.setWorkingMessage`
rewrites `Working...` to anything the extension likes, and
`ctx.ui.setWorkingVisible(false)` takes the row off the pane entirely. Either one
turns a running turn into a screen with no spinner on it. Neither was driven
against a live turn; both are one call away in any extension pi loads.

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
  ever settle on it. Measured: `/arminsayshi` reads `idle` at a span of 4, since
  the composer is still underneath.

Nineteen widgets, four status lines and twenty of these: that is every file in
the directory, and the point of counting them is that the next person does not
have to.

## What the inventory says about the document

Nothing here argues for a new anchor — an anchor is measured against a purpose,
and this pass was measuring coverage. What it does establish, for whoever writes
the next version of `assets/screen-rules-pi.toml`:

1. **`↑↓ navigate` covers the extension dialogs and almost nothing else.** It is
   the right anchor for what that document set out to claim — the screens a
   tool call raises, which is where an agent stops in the middle of work — and
   `ctx.ui.select` and `ctx.ui.confirm` both go through
   `extension-selector.js`. But `ctx.ui.input` and `ctx.ui.editor` are the same
   kind of stop, raised the same way, and they read `unknown`. A permission gate
   written with `ctx.ui.input` instead of `ctx.ui.select` is invisible.
2. **Fourteen screens that need a person read `idle` or `unknown`,** and the two
   readings differ by nothing but how much scrollback is above the box. `idle`
   is the worse of the two: `send` will type into whatever widget is up, and a
   card that says idle is a card saying there is nothing to look at.
3. **The prompt rule's `within = 8` is doing work nobody measured it for.** It
   was sized for an empty composer plus four staged rows. What it decides in
   practice is whether a selector is short enough to be mistaken for a prompt,
   and it decides that differently for the same widget depending on the
   transcript.
4. **A boot screen with pi's own update notice on it reads `unknown`.** Measured
   twice today, and it is the state every pi 0.84.4 on a network is in now that
   0.85.0 exists.
