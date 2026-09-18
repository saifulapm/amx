# The screens amx reads on a claude pane

`assets/screen-rules.toml` held six rules when this was written. Every string in
them was read off a live claude, and until this pass the newest of those
readings was three bumps old — 2.1.226, 2.1.237, 2.1.240. This file is those six
rules driven against 2.1.259 at 220, 54, 40, 30 and 24 columns, with the capture
beside each verdict. It is a measurement and not a second ruleset: no anchor in
it, and nothing here changes what amx recognises. The pass at the foot of the
file drove the same document against 2.1.270 nine days later, and that one added
the seventh rule.

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
spinner rule used to stand on. The rule has been moved off both, and on
2026-09-06 the walk in `[furniture] spinner` followed it, driven at 80 columns
with `--effort low` — see the spinner section.

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
came back empty on this pass — the options were the numbered choices, and this
screen numbers nothing — and that is what the 2.1.276 pass at the foot of the
file answered, by reading the rows off the cursor glyph.

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
this screen was offered the walk rather than `1-9` — `1-2` since the 2.1.276
pass put the rows on the record — and a key whose effect on
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

`[furniture] spinner`, which is what `src/furniture.rs` walks over to find the
rows an agent earned, stood on the two old fragments until 2026-09-06, when it
was driven at 80 columns with `--effort low`. claude spun `● Actioning…` with
nothing after it for the 65 seconds before the first token — no parenthesis at
all, a shape the table above never reached — and the walk printed that row at
the foot of the view's card as the agent's own output. The walk now stands on
`ing…` as the rule does, and that row is cut.

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
   vendor bump. *Answered for the rule on 2026-09-05 and for the walk on
   2026-09-06:* both stand on `ing…`.

One thing was not driven and should not be read as measured here: the
review-answers screen that both multi-part shapes of `AskUserQuestion` end on,
which the document already records as claimed by nothing. `[furniture]
spinner` was the other, and was driven on 2026-09-06 — see the spinner section.

And one thing the re-run turned up that nothing answers yet. At 24 columns
`ask_menu` is claimed off the bottom of the box while the question above it is
out of the floor, so the row says an agent is waiting on a question and carries
no question and no options. Claiming the screen is what a person standing at it
needs and is worth having on its own; reading a box taller than `FLOOR_LINES`
is a separate measurement and nobody has made it.

## What the 2.1.270 pass found

Driven on 2026-09-14 against the claude on this machine, `claude --version`
reporting `2.1.270 (Claude Code)`. The pass above drove the six screens the
document already knew; this one went after screens it does not — the viewer and
the overlay a person opens in front of a session, and the shape a pane takes
when the turn is over and the work is not. It carried a question from outside
with it. herdr's claude manifest reads the pane title before it reads anything
on the screen, and amx reads no title at all; the first finding below is the
answer to that, and `docs/vendors.md` carries the rest of the comparison.

The short version: two screens amx read as `idle` are claimed by nobody now,
one screen nobody claimed is claimed as `working`, the pane title is no signal
on this version, and the walk that cuts the vendor's chrome off a card has
stopped cutting it.

**The pane.** One tmux pane on a server of its own, 100 columns by 30 rows, its
width changed with `resize-window` between captures so that the same live screen
is what each width reads: 100, 54, 40, 30 and 24. Captures are taken the way
`src/tmux.rs` takes one, and the rows below are those captures with trailing
spaces stripped and nothing else done to them. Fifty-nine of them, off one
scratch folder that was nobody's work.

**The verdict** is `Ruleset::claim` over `assets/screen-rules.toml`, run out of
a copy of the tree with `Phase::Unknown` on the record and no still looks — the
same call the pass above made, and for the same reason: that is the reading amx
gives an agent nothing is outstanding for. It was made twice over every capture,
once against the document as 2.1.259 left it and once against the document this
branch carries, which gains a `not` list on `idle_prompt` and a seventh rule
between `spinner` and it. Both readings are in the tables, because the reading a
rule was written for is the only way to check the rule.

**Raising the screens.** The viewer with ctrl+o on a fresh session. The overlay
by typing `/btw` and a question mid-session. The background line by asking for
one subagent, started in the background, to run `sleep 45` and report: claude
ends the turn when the answer stops streaming and goes on running the subagent
after it. The trust gate, the menu, the permission box and the plan box as
before, except that the box this time was raised by the subagent and not by the
agent on the pane.

### The pane title is no signal on this version

`#{pane_title}` was sampled twice a second across both turns of the pass, 71
samples in all:

| What was on the pane | The title |
| --- | --- |
| the trust gate, before claude had drawn a title | `macbook-m2` |
| a fresh session | `✳ Claude Code` |
| a turn running, tool call and all | `✳ Sleep 15` |
| a menu up, a permission box up, and idle after both | `✳ Confirmation and sleep command` |

Seventy of the seventy-one are claude's own and every one of them opens with
`✳`. The glyph did not move while the state did, and no braille frame or
half-circle ever reached the title. herdr's claude manifest ranks two rules on
it first of all — `osc_title_working` and `osc_title_idle`, measured at 2.1.227
and 2.1.228 — and on this version both would be reading the same character in
every state there is.

Reading the title was the one item on the 2026-09-09 herdr list that would have
bought amx a width-independent signal, since a title does not wrap. This is the
measurement that says there is nothing on it to read.

### The transcript viewer read `idle` over a running turn

ctrl+o takes the whole pane and ends in a footer of its own. At 100 columns:

    ────────────────────────────────────────────────────────────────────────────────────────────────────
      Showing detailed transcript · ctrl+o to toggle · ? for shortcuts                          verbose

No mode row under it and no mode row anywhere else on the screen. What
`idle_prompt` found there was `? for shortcuts`, the anchor this document keeps
for a claude older than the glyphs, so a person who opened the viewer to watch a
long tool call got a row saying the turn was over as soon as the hooks went
stale and the screen had held still for `SETTLED_LOOKS`.

The footer truncates from the middle rather than from the right, with `verbose`
pinned to the far side of the row. At 54, 40, 30 and 24:

      Showing detailed transcript · ctrl+o to togg…verbose

      Showing detailed transcript · …verbose

      Showing detailed tran…verbos
                            e

      Showing detaile…verbos
                      e

The last letter wraps onto a row of its own at the two narrowest. `? for
shortcuts` is off the screen below 100 columns, so the wrong claim needed the
wide pane; below it the screen was unclaimed for want of any anchor at all,
which is the right answer arrived at by accident. What no width touches is the
fragment the row opens with, and that is the string the rule refuses itself on
now.

| The viewer | 100 | 54 | 40 | 30 | 24 |
| --- | --- | --- | --- | --- | --- |
| before | **idle** | unclaimed | unclaimed | unclaimed | unclaimed |
| after | unclaimed | unclaimed | unclaimed | unclaimed | unclaimed |

Unclaimed is what this screen wants, rather than a rule of its own. A screen
nobody claims leaves an idle or a waiting record its own word and reads
`unknown` over a record mid-turn, which is herdr's `skip_state_update` reached
from the other end: amx has no way to say *this screen means nothing about the
agent*, and needs none, because that is what an unclaimed screen already says.

### The /btw overlay

`/btw` puts a side question in the slot the composer had. While it works:

    ▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔

        /btw how many files are in this folder

          · Answering…

        Esc to close

`· Answering…` carries `ing…`, so the spinner rule has it, and that is the
answer a person wants: a side question being answered is a turn running. Once it
has answered:

    ▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔

        /btw how many files are in this folder

          I can't tell you — that needs an actual directory listing, and I have no tools available in
          a side question.

          Nothing in the conversation so far has enumerated the contents of /tmp/measure-270. Ask in
          the main conversation and it can run ls there.

        ↑/↓ to scroll · c to copy · f to fork · Esc to close

No mode row under this one either, and the footer it opens — `↑/↓ to scroll · c
to copy · f to fork · Esc to close` — is whole at all five widths. Nothing in
the document claimed the overlay before and nothing claims it now. `↑/↓ to
scroll` went into the rule's `not` list all the same, and on these fifty-nine
captures it moves no verdict: the rows `idle_prompt` stands on are the
composer's and the composer is not on the screen. It is written down because a
screen drawn in the composer's slot is not the composer, and the next thing the
vendor draws there may keep chrome this one leaves off.

The overlay is not the whole pane, though, and the pane is what amx captures.
Directly above the overlay's top border, on the pane it came off, the turn's own
`✻ Waiting for 1 background agent to finish` was still drawn. Where that row
falls is what the width decides: twelve rows up from the bottom at 100 columns,
sixteen at 54 and nineteen at 40, all inside the floor — and twenty-six and
twenty-nine rows up at 30 and 24, where the taller overlay pushes it out:

| The answered overlay | 100 | 54 | 40 | 30 | 24 |
| --- | --- | --- | --- | --- | --- |
| the overlay alone, before | unclaimed | unclaimed | unclaimed | unclaimed | unclaimed |
| the overlay alone, after | unclaimed | unclaimed | unclaimed | unclaimed | unclaimed |
| the whole pane, before | unclaimed | unclaimed | unclaimed | unclaimed | unclaimed |
| the whole pane, after | working | working | working | unclaimed | unclaimed |

The three that say `working` say it on `background_agents`, off a row the
overlay is drawn over, and the word is true of the agent: a subagent was running
the whole time the side question was open. Whether a screen a person has opened
in front of a session should outrank the turn behind it is a question this pass
raises and nobody has answered.

### The line a background subagent leaves up

A turn that has answered, with a subagent still running under it, at 100
columns:

    ✻ Waiting for 1 background agent to finish
                                                                                      ● high · /effort
    ────────────────────────────────────────────────────────────────────────────────────────────────────
    ❯
    ────────────────────────────────────────────────────────────────────────────────────────────────────
      Opus 5 (1M context) (1M context) │ ◈ 2% │ measure-270 │ ◖ high
      ⏵⏵ auto mode on (shift+tab to cycle) · ← 2 agents

      ● main
      ◯ general-purpose  Preparing to run `sleep 45`                              48s · ↓ 10.2k tokens

The Stop hook has fired and the record says the turn is over. The line above the
composer says it is not, the mode row is under the line as it is under
everything this vendor draws, and there is a panel below that with a row per
agent. Before there was a rule for it this screen read `idle` — the mode row is
what `idle_prompt` stands on, and the line above it was nobody's anchor.

The words wrap, and not at one place: after `to` at 40 columns, after
`background` at 30, after `1` at 24.

    ✻ Waiting for 1 background agent to
      finish
                          ● high · /effort
    ────────────────────────────────────────
    ❯
    ────────────────────────────────────────
      Opus 5 (1M context) (1M context) │ …
      ⏵⏵ auto mode on (shift+tab to cycle)

    ✻ Waiting for 1 background
      agent to finish
                ● high · /effort
    ──────────────────────────────
    ❯
    ──────────────────────────────
      Opus 5 (1M context) (1M c…
      ⏵⏵ auto mode on (shift+tab

    ✻ Waiting for 1
      background agent to
      finish
          ● high · /effort
    ────────────────────────
    ❯
    ────────────────────────
      Opus 5 (1M context)…
      ⏵⏵ auto mode on

So `waiting for` and `background` end up on different rows and no one anchor can
want both. One row apart is the widest of the five, and `within = 3` is sized
over it.

| The background line | 100 | 54 | 40 | 30 | 24 |
| --- | --- | --- | --- | --- | --- |
| before | **idle** | working (`spinner`) | **idle** | **idle** | **idle** |
| after | working | working (`spinner`) | working | working | working |

At 54 columns the agents panel elides its subagent's label to `Preparing…`,
which is the whole of the spinner rule's anchor, drawn on a row
under the mode footer. Both rules hold on that screen and both say `working`;
which of them names it is the document order's business, and `spinner` comes
first. The coincidence is why the rule under it was measured at the other four
widths as well.

The glyph is not what the rule reads. `✻` is one of the six the spinner cycles
and it is also what the line a finished turn leaves behind opens with — `✻
Cooked for 18s · done 2:17 PM`, on the same pane an hour earlier — so it is in
the anchor only to put the words on the row the vendor drew them on. It held at
`✻` in every capture over ten seconds rather than cycling, which is a fact about
this version and nothing the rule leans on; the next bump re-measures it.

A `working` claim over a record the hooks left idle is a reading and never
something written down. claude reports, and what it reported stands on the
record; this only puts the right word on the row for as long as the line is up.

### The walk that cuts the chrome had stopped cutting it

`Furniture::cut` is what `src/furniture.rs` walks over to take the vendor's
chrome off a capture before the view floats it in a card, and it reads from the
bottom: the mode footer on the last drawn row is the anchor every later step
hangs off. 2.1.270 draws two rows the walk had never been measured against, one
below the footer and one above the composer, and each of them cost a card
something.

The one below. On `c4-bgline` the last drawn row is the agents panel, so there
is no footer at the bottom, no anchor, and nothing is cut at all: the card over
that agent carried the banner, the whole transcript, the composer box, the
statusline, the mode row and both rows of the panel. That is the walk's own law
working as written — what a shape it was not measured against costs is furniture
left on the screen and never a row of work taken off it — and it is still a card
of chrome.

The one above is `● high · /effort`, right-aligned directly on the composer's
top border with no blank row between the two. It is drawn on a fresh pane, over
a running turn and on an idle pane after one — though not on every idle screen,
since `4-idle` and `c9-workflow` have none. The walk's last step looks for the
spinner row above the box, and where this row is drawn it is what that step
finds instead, so on a pane with a turn running it cost two rows and not one:

    ● Wibbling… (20s)
                                                                                      ● high · /effort
    ────────────────────────────────────────────────────────────────────────────────────────────────────
    ❯
    ────────────────────────────────────────────────────────────────────────────────────────────────────
      Opus 5 (1M context) (1M context) │ ◈ 3% │ measure-270 │ ◖ high
      ⏸ plan mode on (shift+tab to cycle) · /tasks to see subagents · ← 2 agents

The spinner row the walk was moved onto `ing…` for on 2026-09-06 came through
onto the card with the hint row under it, because it was no longer the row above
the box.

**What the walk does now.** Three fields in `[furniture]`, every number read on
2026-09-14 off this pass's own captures at 100, 54, 40, 30 and 24 columns:

- `beneath = ["● ", "◯ "]`, what a row the vendor draws under its mode footer
  opens with after the indent. `c4-bgline` has three of them at all five widths:
  a blank row, `  ● main`, and one row an agent, `  ◯ general-purpose` and its
  elapsed time and tokens.
- `panel = 8`, how many such rows, blank ones counted, the walk steps over from
  the bottom before it must meet the footer. Three were measured, for two
  agents; eight is the statusline's cap and the same kind of margin. The step is
  taken by position, so a taller panel leaves the walk standing on a row that is
  no footer, which is the whole screen kept.
- `hint = ["/effort"]`, the fragment the row on the composer's top border
  carries at all five widths. It is read as a fragment rather than as what the
  row opens with because the row is right-aligned: where it starts is the pane's
  width. Where the walk finds it the cut moves above it, and the blank-skip and
  the spinner check run from there — which is how the spinner row over
  `c8-plan-after` now goes with it.

Both are steps the walk takes where it finds the row and neither is a row it
requires, so a screen that draws neither is cut where it always was:

| What the walk was given | Rows in | Kept before | Kept now |
| --- | --- | --- | --- |
| `c4-bgline` at 100, 54, 40, 30 and 24 | 30 | 30 | 21 |
| `c6-after-bg` and `c7-planmode`, the same shape | 30 | 30 | 21 |
| `c8-plan-after`, the hint row with a spinner over it | 30 | 25 | 23 |
| `2-fresh`, `c0-fresh`, `b6-idle`, `c2-btw-after` | 30 | 25 | 24 |
| `4-idle` and `c9-workflow`, neither row drawn | 30 | 25 | 25 |

The five `c4-bgline` widths keep the same twenty-one rows and end on the row the
turn left up — `✻ Waiting for 1 background agent to finish`, wherever the width
broke it — with the panel, the footer, the statusline, the box and the hint row
off the bottom. `c8-plan-after` keeps through the blank row under `● Agent
"Sleep 45 then report" finished · 1m 34s`. Those five widths, `c8-plan-after`,
`2-fresh`, `4-idle` and `b6-idle` are held verbatim as tests in
`src/furniture.rs`, and so is a pane with nine rows under its footer, which is
the cap being met and the screen kept whole.

### The six screens, re-measured

Everything the document already knew was driven again, and nothing in it moved.

| Screen | Capture | Widths | before and after |
| --- | --- | --- | --- |
| `folder_trust` | `1-gate` | 100 | waiting |
| `ask_menu` | `b2-menu` | 100 | waiting |
| `permission_prompt` | `b4-box`, `c3-background` | 100, 54, 40, 30, 24 | waiting |
| `plan_approval` | `c8-plan` | 100, 54, 40, 30, 24 | waiting |
| `spinner` | `b5-after-box`, `c8-plan-after` | 100 | working |
| `idle_prompt` | `4-idle`, `2-fresh`, `b6-idle`, `c9-workflow` | 100, 54, 40, 30, 24 | idle |

The trust gate draws `❯ No, exit` over `Yes, I trust this folder` exactly as
2.1.259 did, which is the shape `answer`'s walk was written for. The menu keeps
its numbers, its separator and `4. Chat about this`:

     ☐ Proceed?

    Proceed?

    ❯ 1. Yes
         Go ahead.
      2. No
         Do not go ahead.
      3. Type something.
    ────────────────────────────────────────────────────────────────────────────────────────────────────
      4. Chat about this

    Enter to select · ↑/↓ to navigate · Esc to cancel

The permission box is the same box with a new first row on it when a subagent
raised it. Here it is over the background line, raised by the subagent the turn
had started:

    ✻ Waiting for 1 background agent to finish

    ────────────────────────────────────────────────────────────────────────────────────────────────────
     Bash command · from the general-purpose agent

       sleep 45
       Sleep for 45 seconds

     Ask rule Bash overrides auto mode for this command.
     /permissions to let auto mode decide

     Do you want to proceed?
     ❯ 1. Yes
       2. No

     Esc to cancel · Tab to amend

`Bash command · from the general-purpose agent` says whose question it is, which
is a row worth having and not a row any rule reads: `do you want to`, `❯ 1.` and
`esc to cancel` are all on the screen and all in their old places, and the footer
still ends at `Esc to cancel · Tab to amend`. The sentence above it is the
vendor's wording for an `ask` rule under auto mode — `Ask rule Bash overrides
auto mode for this command.` — where the pass above read `Permission rule Bash
requires confirmation for this command.` under manual mode. Neither is an anchor.

The plan box, at 100 columns, the same rows in the same order as 2.1.259:

       Claude has written up a plan and is ready to execute. Would you like to proceed?

       ❯ 1. Yes, and use auto mode
         2. Yes, manually approve edits
         3. Tell Claude what to change
            shift+tab to approve with this feedback

       ctrl+g to edit in Kak · ~/.claude/plans/plan-a-change-add-hazy-origami.md

The spinner spins `● Synthesizing… (4s · ↓ 223 tokens)` and `● Wibbling… (20s)`
on this version, the glyph `●` rather than the `✻` of a finished turn, and
`ing…` is on both. The finished line reads `✻ Cooked for 18s · done 2:17 PM`.

The idle footer gained two things. `· ← 2 agents` is where 2.1.259 read `← for
agents`, the count coming back after the 2.1.240 bump had taken it away:

    ────────────────────────────────────────────────────────────────────────────────────────────────────
    ❯
    ────────────────────────────────────────────────────────────────────────────────────────────────────
      Opus 5 (1M context) (1M context) │ ◈ 2% │ measure-270 │ ◖ high
      ⏵⏵ auto mode on (shift+tab to cycle) · ← 2 agents

And the agents panel under the mode row, the two rows in the background capture
above. Neither is an anchor either — the rule stands on the glyph the mode row
opens with, and both of these are drawn after it.

### What was not raised, and so has no rule

Under the anchor law a string nobody has read off a live screen is not an
anchor, so none of these got one:

- **The plan-mode interview.** shift+tab into plan mode and a request for a plan
  went straight to the approval box the document already matches. Whatever
  interview this version can draw was never on the pane.
- **The dynamic workflow prompt.** Asked for and not drawn: the model listed the
  folder's files instead. The captures from that turn are an ordinary idle pane
  and read as one.
- **MCP elicitation, the connection prompt and the MCP-tasks line.** No MCP
  server was configured for the pass, so none of the three could come up.

The `/model` picker was raised and is claimed by nothing, which is the answer it
should have: it is a vendor dialog amx has no verb for and no rule stands on it.

       Select model
       Switch between Claude models. Your pick becomes the default for new sessions. For
       other/previous model names, specify with --model.

       ❯ 1. Default (recommended) ✔  Opus 5 with 1M context · Best for everyday, complex tasks
         2. Opus (1M context)        Opus 5 with 1M context · Best for everyday, complex tasks
         3. Sonnet                   Sonnet 5 · Efficient for routine tasks
         4. Haiku                    Haiku 4.5 · Fastest for quick answers

       ● High effort (default) ←/→ to adjust

       Enter to set as default · s to use this session only · Esc to cancel

## What the 2.1.276 pass found

Driven on 2026-09-18 against the claude on this machine, `claude --version`
reporting `2.1.276 (Claude Code)`. One screen, the folder-trust gate, raised the
way the passes above raised it: claude started in a folder its own store held no
decision for, in a tmux pane of 30 rows at 220, 54 and 24 columns.

The screen has not moved since 2.1.259. The 54-column and 24-column captures
came back row for row the two in `## folder_trust` above, and 220 columns is the
same box with the sentence on one row and the rest of it unchanged, down to
where the cursor opens:

     Security guide

     ❯ No, exit
       Yes, I trust this folder

     Enter to confirm · Esc to cancel

The 220-column capture is held whole in `src/rules.rs` as
`TRUST_SCREEN_276_220`.

What moved is the reading. The rule carries `marks = "❯"` now, which is what
pi's four selectors have carried since 2026-09-14: the run of rows the cursor
glyph is in is the list, and amx numbers it itself. The gate reads back two
choices in the order the vendor draws them — `No, exit`, then `Yes, I trust this
folder` — and the record says they are amx's numbers rather than the vendor's.
So the row a person reads offers `1-2` instead of a walk typed blind, `amx
answer <id> 2` is the `Up Down Enter` that reaches the row that trusts the
folder, and `enter`, `y` and `n` are refused with the two digits offered in
their place. `esc` still cancels. The walk is still an answer: `down enter` is
what it always was, for a caller reading the rows rather than their numbers.

Two things the mark does not do. A run the vendor numbered itself is read off
its numbers, so the 2.1.226 captures in `src/rules.rs` — `❯ 1. Yes, I trust this
folder` over `2. No, exit` — read back exactly as they did, unwalked, with the
digits the vendor drew. And the question is unmoved: `asks` still anchors on
`quick`, and the sentence reads whole at every width.

The 24-column capture is where the wrap had to be read. claude hangs the rest of
a label under the label, both at the same column — `Yes, I trust this` over
`folder` — where pi wraps to the left of it and the indent is what tells a wrap
from a choice. A row opening in lower case is the rest of the row above it, the
way the prose reader already treats one, and that is what keeps this gate at two
choices rather than three at the narrowest width driven.
