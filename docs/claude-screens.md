# The six screens amx reads on a claude 2.1.259 pane

`assets/screen-rules.toml` holds six rules. Every string in them was read off a
live claude, and until this pass the newest of those readings was three bumps
old — 2.1.226, 2.1.237, 2.1.240. This file is those six rules driven against
2.1.259 at 220, 54, 40, 30 and 24 columns, with the capture beside each
verdict. It is a measurement and not a second ruleset: no anchor in it, and
nothing here changes what amx recognises.

Three rules were re-anchored off these captures afterwards, and the verdicts
below have been re-run against the document as it now stands, so a row in a
table here is what amx reads today. What each of the three read before it was
answered is kept beside it, because that reading is what the answer was for,
and a rule moved without the reading that moved it is a rule nobody can check.

The short version: all six hold at every width driven, and three of them did
not when the pass began. A claude stopped on its own folder-trust gate read
`unknown` at 24 columns, so nothing keyed on `setup` fired and the offer of the
trust key never came. A menu somebody was standing at read `unknown` there too,
because the box was taller than the rows a rule may see. And a claude with a
turn running read **`idle`** at 30 and at 24 — not `unknown`, which is the
answer a missing rule is supposed to give, but the confident wrong one, because
the mode footer is still on the screen and `idle_prompt` is the next rule in
the file.

Two more things moved, and both cost more than a rule. The folder-trust screen
stopped numbering its choices, which takes `❯ 1.` off it, put an answer where
the question should be on the row, and left every key `amx answer` was allowed
to type at that screen doing nothing or ending the agent. And the line claude
leaves behind when a turn is over now carries one of the two fragments the
spinner rule used to stand on. The rule has been moved off both; the walk in
`[furniture] spinner` still stands on the pair, and it was not driven this
round.

## How this was read

Driven on 2026-09-05 against the claude on this machine, `claude --version`
reporting `2.1.259 (Claude Code)`.

**The pane.** One tmux pane on a server of its own, 30 rows tall except where a
line below says otherwise, its width changed with `resize-window` between
captures so that the same live screen is what each width reads. Captures are taken the way `src/tmux.rs` takes
one: `capture-pane -p -J`, trailing whitespace off the end of the whole
capture, then every control and format character replaced with a space. The
rows below are those captures with trailing spaces stripped and nothing else
done to them.

**The verdict** is the crate's own reading and not a second implementation of
it. Each capture went through `Ruleset::claim` over `assets/screen-rules.toml`
as this branch carries it, run out of a copy of the tree, with `Phase::Unknown`
on the record and no still looks — so `idle_prompt`, the one quiescent rule,
decides at once, the way it does for an agent nothing is outstanding for. The
`span` column is that rule's own arithmetic: the topmost row carrying each
`all` string and the topmost row carrying any `any` string, the lowest of those
rows minus the highest, counted inside the floor — the last 24 rows
(`FLOOR_LINES`) of the trimmed capture.

That call was made twice: once against the document as 2.1.240 left it, which
is what the findings at the end of this file were found by, and again against
the document those findings were answered in. Four cells moved between the two
runs and the other twenty-six did not, because no rule lost an anchor that was
holding — `folder_trust` dropped a string the screen had stopped drawing,
`ask_menu` gained one below the two it had, and both keep matching the rows
they matched before. The captures the second run read are the five in this file
and the four checked in beside the rules they answer, in `src/rules.rs`.

**Raising the screens.** The trust gate by starting claude in a git repository
it had no decision for. The permission box with `--permission-mode manual` over
a `.claude/settings.json` carrying `{"permissions": {"ask": ["Bash"]}}`, then
asking for `sleep 300` — manual mode alone was not enough, a bare `sleep` ran
without a box. The menu by asking for `AskUserQuestion` with a description
under each choice. The plan box by cycling to plan mode with shift+tab and
asking for a one-paragraph plan. The spinner by asking for a long multiplication
with no tools at all, on a pane with nothing else on it, so the only row that
could carry its anchors was the spinner's own. The idle footer on a fresh boot,
after a turn, and in all six permission modes.

Two scratch folders, both made for this and neither anybody's work: one
trusted, to get past the gate and drive the other five screens, and one left
untrusted so the gate could be raised again. The trust screen was measured on
both, and the two agree to the row at every width.

## The six screens at five widths

`span` is the rows the rule's own matches covered. The rules are the ones this
branch carries; the four cells the re-anchoring moved say what they read before
it in brackets, and a bracketed rule name is the rule that claimed the screen
instead.

| Screen | 220 | 54 | 40 | 30 | 24 |
| --- | --- | --- | --- | --- | --- |
| `folder_trust` | waiting, span 9 | waiting, span 13 | waiting, span 14 | waiting, span 16 | waiting, span 19 (was **unknown**) |
| `permission_prompt` | waiting, span 1 | waiting, span 1 | waiting, span 1 | waiting, span 1 | waiting, span 2 |
| `ask_menu` | waiting, span 8 | waiting, span 12 | waiting, span 14 | waiting, span 18 | waiting, span 2 (was **unknown**) |
| `plan_approval` | waiting, span 2 | waiting, span 3 | waiting, span 3 | waiting, span 4 | waiting, span 4 |
| `spinner` | working, span 0 | working, span 0 | working, span 0 | working, span 0 (was **idle** [`idle_prompt`]) | working, span 0 (was **idle** [`idle_prompt`]) |
| `idle_prompt` | idle | idle | idle | idle | idle |

And the same six rules read as their anchors. A row marked *was* is a string
the rule no longer carries, kept here because it is what the reading beside it
paid for.

| Rule | What it stands on | 2.1.259 |
| --- | --- | --- |
| `folder_trust` | `trust` | holds at all five widths |
| | `enter to confirm` | holds at all five, and is the whole of the affordance now |
| | `within = 19` | holds at all five; 19 is the span at 24 columns, the widest driven |
| | `asks = { sentence = "quick" }` | holds: the question whole, with an empty options list under it |
| | *was* `❯ 1.` | **gone** at all five: the screen has no numbered choices any more |
| | *was* `within = 16` | **short by three rows at 24 columns**, where the screen spans 19 |
| | *was* `asks = { sentence = "trust" }` | **drifted**: it read back a choice, not the question |
| `permission_prompt` | `do you want to` | holds at all five |
| | `❯ 1.` | holds at all five |
| | `esc to cancel` | holds at all five |
| `ask_menu` | `enter to select` | holds at all five, still the fragment its row opens with |
| | `chat about this` | inside the floor at 24, the one width where neither of the other two holds |
| | `❯ 1.` | holds at 220 to 30; **out of the floor** at 24 |
| | `esc to cancel` | broken by the wrap at 40 and at 24; whole at 220, 54, 30 |
| `plan_approval` | `ready`, `execute` | hold, on one row, at all five |
| | `❯ 1.` | holds at all five |
| `spinner` | `ing…` | holds at all five, and is on no line a finished turn leaves |
| | *was* `… (` | held at 220, 54, 40, 24; **gone at 30** |
| | *was* `s · ` | held at 220, 54, 40; **gone at 30 and 24**, and on the finished line at all five |
| `idle_prompt` | `⏵⏵`, `⏸` | hold at all five, in all six permission modes |
| | `shift+tab to cycle` | gone by 30 columns, and absent in manual mode at every width |

`not_below` was never wrong on any capture in this pass: not one of the four
blocking screens draws a mode footer under itself, at any of the five widths.

## `folder_trust`

At 54 columns, and the same screen at 220 but for where the sentence wraps:

    ──────────────────────────────────────────────────────
     Accessing workspace:

     /home/saiful/.claude/jobs/dfc82656/tmp/scratch2

     Quick safety check: Is this a project you created or
     one you trust? (Like your own code, a well-known
     open source project, or work from your team). If
     not, take a moment to review what's in this folder
     first.

     Claude Code'll be able to read, edit, and execute
     files here.

     Security guide

     ❯ No, exit
       Yes, I trust this folder

     Enter to confirm · Esc to cancel

The wording is the 2.1.240 wording to the letter. What changed is under it: the
choices have lost their numbers, and they have swapped places. `❯ 1. Yes, I
trust this folder` / `2. No, exit` is now `❯ No, exit` / `Yes, I trust this
folder`, with the cursor on the exit.

The rule survived that: `any` was a pair, and `enter to confirm` is the half
the screen kept. `❯ 1.` has come out of the rule since — an anchor the vendor
has stopped drawing holds nothing up, and all it can still match is a numbered
list somewhere else on the pane. Three things around the rule did not survive,
and each was answered where it broke.

**The question on the row read back as a choice.** `asks = { sentence =
"trust" }` reads the sentence the lowest row carrying `trust` belongs to, above
the first numbered choice. With no numbered choice on the screen there is no
ceiling, and the lowest row carrying `trust` is no longer the safety-check
sentence — it is `Yes, I trust this folder`. So the question read back as
**"Yes, I trust this folder"** and the options list came back **empty**, at
220, 54, 40 and 30 columns alike. Whoever read that row was handed an answer in
the place where the thing being asked should be.

`asks` anchors on `quick` now. That word is on the question's own first row and
nowhere else on the screen, and it is one word, so no width can break it: the
question reads back whole at 54 and at 24 columns, *Quick safety check: Is this
a project you created or one you trust? …* to the full stop. The options list
is still empty, and that is the reading rather than a gap — the options are the
numbered choices, and this screen numbers nothing.

**Every key the answer grammar allowed was wrong.** `amx answer` took `y`, `n`,
`1`–`9`, `enter` and `esc` at a screen of this kind. Driven one key per fresh
boot of the screen:

| Key | What 2.1.259 does with it |
| --- | --- |
| `1` | nothing; the cursor stays on `No, exit` |
| `2` | nothing |
| `y` | nothing |
| `n` | **claude exits**, status 0 |
| `enter` | **claude exits**, status 0 — the cursor is on `No, exit` |

The two keys that do something end the agent, and the three that would have
meant yes do nothing at all. Nothing in the grammar reached `Yes, I trust this
folder`; the only way to it is `Down` and then `Enter`.

The grammar has both of those now, and takes them as one answer: `amx answer
<id> "down enter"` walks the cursor to the row the caller means and takes what
it lands on. A walk with no take on the end of it is refused, since it moves
the cursor and answers nothing while the record would say the question was
answered. A bare `enter` is refused too, at a screen whose rows the record
carries no numbers for: it takes whichever row the vendor opened on, and here
that is the exit. What a waiting row prints is read off the same record, so
this screen is offered the walk rather than `1-9`, and a key whose effect on
the screen amx cannot check leaves the record saying `waiting` — so the screen
can be answered again rather than refused with *nothing to answer* while it is
still on the pane.

None of this was a hole amx fell into on the usual day — `src/trust.rs`
answers claude's gate by writing `hasTrustDialogAccepted` into the vendor's own
store before the pane exists, and a tree that already has the entry never draws
the screen. It is what is left for an agent that meets the screen anyway: on a
repository the person has never trusted, or with `trust` off in the config.

**At 24 columns the rule walked past its own gate.** The whole capture but for
its blank first row — 29 rows after the trim, floor from row 6 down:

     /home/saiful/.claude/j
     obs/dfc82656/tmp/scrat
     ch2

     Quick safety check: Is
     this a project you
     created or one you
     trust? (Like your own
     code, a well-known
     open source project,
     or work from your
     team). If not, take a
     moment to review
     what's in this folder
     first.

     Claude Code'll be able
     to read, edit, and
     execute files here.

     Security guide

     ❯ No, exit
       Yes, I trust this
       folder

     Enter to confirm · Esc
     to cancel

`trust` is first found on ` trust? (Like your own`, where the wrap put it, and
`enter to confirm` is 19 rows below that. `within` was 16, so the rule walked
past, and no other rule claimed the screen: `unknown`. The reading is a fact
about the width and not about the pane's height — the same 19 came back at pane
heights 40, 30 and 24. At 30 columns the span is 16 exactly, so the rule held
there by one row.

`within` is 19 now, counted off this capture, which is the widest this box gets
at any width driven. The gate is claimed at all five, span 19 at the narrowest.

What the silence cost is the `setup` flag. `folder_trust` is the only rule in
the document that carries it, `doctor` reads the flag rather than the rule's
name, and a screen nothing claims is not a gate anybody is standing at. So at
24 columns `amx doctor` said "no agent is stopped at the vendor's own setup"
about an agent that was stopped at exactly that, and the remedy that goes with
it — attach and answer it, or set `trust = true` and let amx answer it — was
never printed.

## `permission_prompt`

Unchanged where it counts. At 54 columns:

    ──────────────────────────────────────────────────────
     Bash command

       sleep 300
       Sleep for 300 seconds

     Permission rule Bash requires confirmation for this
     command.
     /permissions to update rules

     Do you want to proceed?
     ❯ 1. Yes
       2. No

     Esc to cancel · Tab to amend

`do you want to`, `❯ 1.` and `esc to cancel` are all present at every width,
the span never opens past 2, and the question reads back as
`Do you want to proceed?` with `["Yes", "No"]` under it at all five. The box
keeps its numbers, which is worth saying beside the trust screen that lost
them: this is one vendor screen changing, not the vendor's dialogs changing.

One drift with no cost: the footer used to read `Esc to cancel · Tab to amend ·
ctrl+e to explain` and now stops after `Tab to amend` at 220 columns, where
nothing is truncating it. The rule anchors on the fragment the row opens with,
so it did not notice.

At 24 columns the footer wraps as `Esc to cancel · Tab to` / `amend`, which
leaves `esc to cancel` whole.

## `ask_menu`

At 54 columns, the plainest shape the tool draws:

    ──────────────────────────────────────────────────────
     ☐ Indentation

    Should this project be indented with spaces or tabs?

    ❯ 1. Spaces
         Fixed-width indentation that renders identically
         everywhere; the common default for most language
         style guides and formatters.
      2. Tabs
         One tab character per level, so each reader's
         editor controls the visible width; better for
         accessibility and smaller files.
      3. Type something.
    ──────────────────────────────────────────────────────
      4. Chat about this

    Enter to select · ↑/↓ to navigate · Esc to cancel

Checkbox, question, numbered options, the separator, `N. Chat about this`, and
a footer that still opens with `Enter to select`: the same shape 2.1.229 and
2.1.240 drew. The question and all four options read back correctly at 220, 54,
40 and 30.

At 24 columns the rule went quiet, and not because a string changed. The box is
taller than the floor. Here is the whole capture below its blank first row — 30
rows in all, so the floor is the last 24 and begins on the seventh:

    Should this project be
    indented with spaces or
    tabs?

    ❯ 1. Spaces
         Fixed-width
         indentation that
         renders identically
         everywhere; the
         common default for
         most language style
         guides and
         formatters.
      2. Tabs
         One tab character
         per level, so each
         reader's editor
         controls the
         visible width;
         better for
         accessibility and
         smaller files.
      3. Type something.
    ────────────────────────
      4. Chat about this

    Enter to select · ↑/↓ to
    navigate · Esc to
    cancel

`❯ 1. Spaces` is the sixth row, one above the floor's top edge, and the footer
wraps as `Enter to select · ↑/↓ to` / `navigate · Esc to` / `cancel`, which
breaks the other half of `any`. `enter to select` is on the screen and matched;
with both entries in `any` gone at once there was no affordance left inside the
rows a rule may see, and the screen read `unknown`.

That is the hazard the rule's own comment names — "the box's height is the
agent's own choice" — arriving at a width. Two one-sentence descriptions were
enough to do it. The `within = 24` this rule carries is not what failed: the
marker was outside the floor, not too far from the footer.

The affordance comes off the bottom of the box now. `  4. Chat about this` is
the row the vendor draws between the last choice and the footer on every screen
its question tool puts up, so it is inside the floor whenever the footer this
rule already stands on is, whatever the agent wrote above it. The screen reads
`waiting`, span 2: `chat about this`, the blank row under it, and the first row
of the footer.

Being claimed is not the same as being read. The question is the sentence above
the first numbered choice, and `❯ 1. Spaces` is exactly the row that fell out
of the floor, so at this width there is no first option to read above and the
reading comes back with nothing on it — no question and no options. The row
says an agent is waiting on a question and carries none of the words it is
being asked, where the four wider widths carry all of them. That is the same
box being taller than the rows a rule may see, arriving one step further in:
the claim survives a box that tall and the reading does not.

`esc to cancel` also breaks at 40 columns, where the footer wraps as
`Enter to select · ↑/↓ to navigate · Esc` / `to cancel`. The rule holds there
on `❯ 1.`, which is why `any` had two entries before it had three.

## `plan_approval`

The strongest of the four. At 24 columns, the narrowest driven:

       Claude has written
       up a plan and is
       ready to execute.
       Would you like to
       proceed?

       ❯ 1. Yes, and use
            auto mode
         2. Yes, manually
            approve edits
         3. Tell Claude
            what to change
            shift+tab to
            approve with
            this feedback

       ctrl+g to edit in
       Kak · ~/.claude/pl
       ans/write-a-one-pa
       ragraph-harmonic-v
       aliant.md

`ready` and `execute` land on one row at every width driven, the span never
opens past 4, and the question reads back whole — `Claude has written up a plan
and is ready to execute. Would you like to proceed?` — at all five, because the
reading joins the rows the vendor wrapped it out of. The options are read as
far as their own rows go, so at 24 columns they come back as `Yes, and use`,
`Yes, manually`, `Tell Claude`, which is the documented behaviour and not a
drift.

## `spinner`

This is the rule that broke, and the way it broke is worse than going quiet.

claude composes the spinner row as a glyph, a rotating gerund, `…`, and then a
parenthesis holding the elapsed time and a detail after a `·`. It drops that
tail from the right as the pane narrows, and both of the rule's two anchors
were in the part it drops. One turn, one pane, five widths, the row as it read
at each:

| Width | The spinner row | `… (` | `s · ` |
| --- | --- | --- | --- |
| 220 | `● Finagling… (6s · thinking with xhigh effort)` | yes | yes |
| 54 | `● Finagling… (5s · thinking with xhigh effort)` | yes | yes |
| 40 | `● Finagling… (4s · thinking)` | yes | yes |
| 30 | `● Finagling… thinking` | **no** | **no** |
| 24 | `● Finagling… (2s)` | yes | **no** |

The rule wanted both, so at 30 and at 24 it did not hold. What claimed the
screen instead was `idle_prompt`, because the mode footer is under the spinner
the whole time a turn runs — which the idle rule's own comment says in capitals
and is exactly the trap it was written for. Sampled eight times a second apart
at each of three widths, over one live turn:

| Width | working | idle |
| --- | --- | --- |
| 40 | 7 | 1 (the turn had ended) |
| 30 | 0 | 8 |
| 24 | 0 | 8 |

The 30-column row is not stable across a turn either: the same turn read
`● Finagling… thinking` for four samples and `● Finagling… (33s)` for the next
three. What survives at a given width depends on how long the gerund is, and
the gerund changes while the turn runs, so at 30 columns the same running
agent is sometimes missing one anchor and sometimes both.

Two things stood between that and a wrong row in practice, and neither was
much. `quiescent` gates `idle_prompt` from ending a turn that is on the record
as running until the screen has held still for `SETTLED_LOOKS`; but the
narrower the pane, the less of that row there is to move — at 24 columns it is
the elapsed second and nothing else, and the four 30-column samples that read
`● Finagling… thinking` carry nothing that moves at all. And an agent whose
hooks are flowing is read from its hooks and not from its screen. The reading
is what is left when the hooks stop — an agent interrupted with Escape, or one
nobody has heard from — and on a narrow pane what was left said a working agent
had finished.

**Where the rule stands now.** `ing…`: the end of the gerund and the ellipsis
after it, which is the left of the row and the part the truncation never
reaches. One fragment, not a list of the tails the vendor sometimes keeps — a
list goes quiet at whatever width drops the next of them, and this row is not
even steady at one width. It holds on all five rows of the table above:
`working`, span 0, at 220, 54, 40, 30 and 24.

The ellipsis alone would not do. It is how claude elides anything too long for
the room it has, and what it elides is under every idle pane — the statusline
reads `Opus 5 (1M context) (1M context) │ …` at 40 columns — so an agent parked
at its prompt on a narrow pane would read `working` for as long as it sat
there. What makes `ing…` a spinner rather than an elision is the vendor's own
grammar: the word it spins is a present participle in every sample this file
and `assets/screen-rules.toml` record across three vendor versions, and the
word on the line a turn leaves behind is a past one.

A transcript could also put `s · ` back by accident. Driven at 24 columns with
a tool call above the box, `● Sleeping for 300 seconds · 42s` — claude's own
tool header with the elapsed time on it — carried the fragment the spinner row
had dropped, and the rule held with a span of 8 rows between two rows that have
nothing to do with each other. That is the same rule holding for the wrong
reason, which is why the clean measurement above was driven on a pane with
nothing but the spinner on it. `ing…` is on none of those rows, and it takes
one row rather than two, so there is no second row for it to hold across.

**The other half: `s · ` is on the idle screen.** When a turn is over 2.1.259
leaves this behind:

    ✻ Cogitated for 2m 6s · done 10:09 AM

The rule's old comment said of this line: "same glyph, no ellipsis and no
parenthesis", and that neither fragment was on it. One of them is on it —
`6s · done` carries `s · ` — measured at all five widths. It walked past
anyway, because `all` wanted both and `… (` is genuinely absent, so the whole
of what kept a finished agent from reading `working` had come down to one
punctuation fragment. `ing…` is not on that line either, and it is not a margin
that can be spent the same way: `Cogitated` is the past tense of the word the
spinner spins, which is the difference the anchor is now reading. That screen
reads `idle` at 40 columns, on `idle_prompt`, with the finished line above the
box.

The two old fragments are still `[furniture] spinner`, which is what
`src/furniture.rs` walks over to find the rows an agent earned, and that walk
wants both of them on the row. It was not driven this round and is the obvious
next thing to drive: a spinner row the walk cannot recognise is a row of the
vendor's chrome printed as the agent's own output, and the 30- and 24-column
rows in the table above are two of them.

## `idle_prompt`

Holds everywhere, on the anchor it was moved to. `⏵⏵` and `⏸` were on the
screen in all six permission modes at 220 columns and at 24:

    ⏵⏵ auto mode on (shift+tab to cycle) · ← for agents
    ⏵⏵ accept edits on (shift+tab to cycle) · ← for agents
    ⏸ plan mode on (shift+tab to cycle) · ← for agents
    ⏸ manual mode on · ← for agents
    ⏵⏵ bypass permissions on (shift+tab to cycle) · ← for agents
    ⏵⏵ don't ask on (shift+tab to cycle) · ← for agents

Six lines, word for word what the document recorded at 2.1.237 and 2.1.240,
including the tail that counts — the same pane read `← 5 agents` earlier in
this pass and `← for agents` later, which is the 2.1.240 change still in place.
Four of the six modes are reachable with shift+tab, and the other two were
driven with `--permission-mode` on the argv.

At 24 columns the footer truncates to `⏵⏵ auto mode on`, `⏸ manual mode on · …`
and `⏵⏵ bypass`, so the glyph is all that is left — which is the whole reason
it is the anchor.

The screen itself, at 40 columns — box, statusline elided from the right,
footer:

    ───────────────────── execute t1 brief ─
    ❯
    ────────────────────────────────────────
      Opus 5 (1M context) (1M context) │ …
      ⏵⏵ auto mode on (shift+tab to cycle)

## What this pass found, and what it cost

Six findings, in the order of what they cost, each with what was done about it.
The verdict tables above are the document after all six; the readings here are
what it was before, which is the only way to check the answers.

1. **A running turn read `idle` below 40 columns.** The spinner row drops the
   parenthesis and its `·` as the pane narrows, both of the rule's anchors went
   with it, and `idle_prompt` claimed the screen: 16 of 16 samples across one
   live turn at 30 and 24 columns. Four agents on a 160-column terminal is 40
   columns each, and a fifth takes them under it. Naming a screen non-blocking
   also clears any pending question off the row, so this is the failure the
   spinner rule's own comment calls the worst one available. *Answered in the
   rule:* it stands on `ing…` now, the part of the row the truncation never
   reaches, and reads `working` at all five widths.
2. **A claude on its folder-trust gate read `unknown` at 24 columns.** The
   screen spans 19 rows where `within` allowed 16. `setup` is a flag on the
   rule rather than on the record, so an unclaimed screen is not a gate:
   `doctor` reported nothing wrong and the trust-key remedy was never offered,
   about an agent that would sit there until somebody attached. *Answered in
   the rule:* `within = 19`, counted off the capture above.
3. **A menu somebody was standing at read `unknown` at 24 columns.** Two
   one-sentence descriptions put `❯ 1.` one row above the floor, and the wrap
   that did it broke `esc to cancel` on the same screen, so both entries in
   `any` were gone at once. *Answered in the rule:* `chat about this`, the row
   the vendor draws under the last choice, which is inside the floor whenever
   the footer the rule already stands on is.
4. **The trust screen's choices lost their numbers, and its question read back
   as an answer.** No `❯ 1.` anywhere on it, so `asks` found `trust` on
   `Yes, I trust this folder` instead of on the safety-check sentence, and
   handed that back as the question with no options beside it. *Answered in the
   rule:* `asks` anchors on `quick`, which is on the question's own first row
   and nowhere else, and `❯ 1.` is out of `any` because the vendor has stopped
   drawing it.
5. **Every key the answer grammar allowed at that screen was wrong.** `1`, `2`
   and `y` do nothing; `n` and `enter` exit the agent, because the cursor now
   opens on `No, exit`. The way to yes is an arrow key, which was not in the
   grammar. The store write in `src/trust.rs` is what keeps this off the usual
   day, and it was unaffected — it never touches the screen. *Answered in the
   verbs:* `answer` takes a walk and a take as one line, refuses a bare take at
   a screen whose rows carry no numbers, and leaves the record `waiting` after
   a key whose effect it cannot check; the offer a waiting row prints is read
   off the same record, so it no longer names keys the screen will not take.
6. **The line a finished turn leaves behind carries `s · `.** `✻ Cogitated for
   2m 6s · done 10:09 AM`, at all five widths. The rule wanted both of its
   fragments and `… (` was absent, so nothing read wrong; what was gone was the
   margin, on the rule its own comment calls the first one to re-measure at a
   vendor bump. *Answered for the rule and not for the walk:* the rule is off
   both fragments, and `[furniture] spinner` still stands on the pair.

Two things were not driven and should not be read as measured here: the
review-answers screen that both multi-part shapes of `AskUserQuestion` end on,
which the document already records as claimed by nothing; and
`[furniture] spinner`, which still carries the two fragments findings 1 and 6
are about, so both of those findings hold against the walk unchanged.

And one thing the re-run turned up that nothing answers yet. At 24 columns
`ask_menu` is claimed off the bottom of the box while the question above it is
out of the floor, so the row says an agent is waiting on a question and carries
no question and no options. Claiming the screen is what a person standing at it
needs and is worth having on its own; reading a box taller than `FLOOR_LINES`
is a separate measurement and nobody has made it.
