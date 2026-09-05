# The six screens amx reads on a claude 2.1.259 pane

`assets/screen-rules.toml` holds six rules, and every string in them was read
off a live claude at a version that is now three bumps old — 2.1.226, 2.1.237,
2.1.240. This file is what those six rules read as against 2.1.259, driven at
220, 54, 40, 30 and 24 columns, with the capture beside each verdict. It is a
measurement and not a second ruleset: no anchor in it, and nothing here changes
what amx recognises. What it is for is knowing which of the six still hold,
which hold on a fragment that has drifted under them, and which are gone — so
the next pass at that document starts from a list rather than from whatever
screen somebody happened to hit.

The short version: four rules hold at every width driven and two do not, and
both of them fail narrow. A claude stopped on its own folder-trust gate reads
`unknown` at 24 columns, so nothing keyed on `setup` fires and the offer of the
trust key never comes. A claude with a turn running reads **`idle`** at 30 and
at 24 — not `unknown`, which is the answer a missing rule is supposed to give,
but the confident wrong one, because the mode footer is still on the screen and
`idle_prompt` is the next rule in the file.

Two more things moved without taking a rule down, and both cost something. The
folder-trust screen stopped numbering its choices, which takes `❯ 1.` off it,
puts an answer where the question should be on the row, and leaves every key
`amx answer` is allowed to type at that screen doing nothing or ending the
agent. And the line claude leaves behind when a turn is over now carries one of
the spinner rule's two fragments, which the rule's own comment says it does not.

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

`span` is the rows the rule's own matches covered; a bracketed name is the rule
that claimed the screen instead.

| Screen | 220 | 54 | 40 | 30 | 24 |
| --- | --- | --- | --- | --- | --- |
| `folder_trust` | waiting, span 9 | waiting, span 13 | waiting, span 14 | waiting, span 16 | **unknown** (span 19 > `within`) |
| `permission_prompt` | waiting, span 1 | waiting, span 1 | waiting, span 1 | waiting, span 1 | waiting, span 2 |
| `ask_menu` | waiting, span 8 | waiting, span 12 | waiting, span 14 | waiting, span 18 | **unknown** (no affordance in the floor) |
| `plan_approval` | waiting, span 2 | waiting, span 3 | waiting, span 3 | waiting, span 4 | waiting, span 4 |
| `spinner` | working, span 0 | working, span 0 | working, span 0 | **idle** [`idle_prompt`] | **idle** [`idle_prompt`] |
| `idle_prompt` | idle | idle | idle | idle | idle |

And the same six rules read as their anchors:

| Rule | What it stands on | 2.1.259 |
| --- | --- | --- |
| `folder_trust` | `trust` | holds at all five widths |
| | `❯ 1.` | **gone** at all five: the screen has no numbered choices any more |
| | `enter to confirm` | holds at all five, and is what keeps the rule standing |
| | `within = 16` | **gone at 24 columns**, where the screen spans 19 |
| | `asks = { sentence = "trust" }` | **drifted**: it reads back a choice, not the question |
| `permission_prompt` | `do you want to` | holds at all five |
| | `❯ 1.` | holds at all five |
| | `esc to cancel` | holds at all five |
| `ask_menu` | `enter to select` | holds at all five, still the fragment its row opens with |
| | `❯ 1.` | holds at 220 to 30; **out of the floor** at 24 |
| | `esc to cancel` | broken by the wrap at 40 and at 24; whole at 220, 54, 30 |
| `plan_approval` | `ready`, `execute` | hold, on one row, at all five |
| | `❯ 1.` | holds at all five |
| `spinner` | `… (` | holds at 220, 54, 40, 24; **gone at 30** |
| | `s · ` | holds at 220, 54, 40; **gone at 30 and 24** |
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

The rule survives that: `any` is a pair, and `enter to confirm` is the other
half of it. Three things around the rule do not.

**The question on the row is a choice.** `asks = { sentence = "trust" }` reads
the sentence the lowest row carrying `trust` belongs to, above the first
numbered choice. With no numbered choice on the screen there is no ceiling, and
the lowest row carrying `trust` is no longer the safety-check sentence — it is
`Yes, I trust this folder`. So the question reads back as **"Yes, I trust this
folder"** and the options list comes back **empty**, at 220, 54, 40 and 30
columns alike. Whoever reads that row is handed an answer in the place where
the thing being asked should be.

**Every key the answer grammar allows is wrong.** `amx answer` takes `y`, `n`,
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
meant yes do nothing at all. Nothing in the grammar reaches `Yes, I trust this
folder`; the only way to it is `Down` and then `Enter`. This is not a hole amx
falls into on the usual day — `src/trust.rs` answers claude's gate by writing
`hasTrustDialogAccepted` into the vendor's own store before the pane exists,
and a tree that already has the entry never draws the screen. It is what is
left for an agent that meets the screen anyway: on a repository the person has
never trusted, or with `trust` off in the config.

**At 24 columns the rule is gone.** The whole capture but for its blank first
row — 29 rows after the trim, floor from row 6 down:

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
`enter to confirm` is 19 rows below that. `within` is 16, so the rule walks
past, and no other rule claims the screen: `unknown`. The reading is a fact
about the width and not about the pane's height — the same 19 came back at pane
heights 40, 30 and 24. At 30 columns the span is 16 exactly, so the rule holds
there by one row.

What the silence costs is the `setup` flag. `folder_trust` is the only rule in
the document that carries it, `doctor` reads the flag rather than the rule's
name, and a screen nothing claims is not a gate anybody is standing at. So at
24 columns `amx doctor` says "no agent is stopped at the vendor's own setup"
about an agent that is stopped at exactly that, and the remedy that goes with
it — attach and answer it, or set `trust = true` and let amx answer it — is
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

At 24 columns the rule is gone, and not because a string changed. The box is
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
there is simply no affordance left inside the rows a rule may see, so the
screen reads `unknown`.

That is the hazard the rule's own comment names — "the box's height is the
agent's own choice" — arriving at a width. Two one-sentence descriptions were
enough to do it. The `within = 24` this rule carries is not what failed: the
marker was outside the floor, not too far from the footer.

`esc to cancel` also breaks at 40 columns, where the footer wraps as
`Enter to select · ↑/↓ to navigate · Esc` / `to cancel`. The rule holds there
on `❯ 1.`, which is why `any` has two entries.

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
tail from the right as the pane narrows, and both of the rule's anchors are in
the part it drops. One turn, one pane, five widths, the row as it read at each:

| Width | The spinner row | `… (` | `s · ` |
| --- | --- | --- | --- |
| 220 | `● Finagling… (6s · thinking with xhigh effort)` | yes | yes |
| 54 | `● Finagling… (5s · thinking with xhigh effort)` | yes | yes |
| 40 | `● Finagling… (4s · thinking)` | yes | yes |
| 30 | `● Finagling… thinking` | **no** | **no** |
| 24 | `● Finagling… (2s)` | yes | **no** |

The rule wants both, so at 30 and at 24 it does not hold. What claims the
screen instead is `idle_prompt`, because the mode footer is under the spinner
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

Two things stand between that and a wrong row in practice, and neither is much.
`quiescent` gates `idle_prompt` from ending a turn that is on the record as
running until the screen has held still for `SETTLED_LOOKS`; but the narrower
the pane, the less of that row there is to move — at 24 columns it is the
elapsed second and nothing else, and the four 30-column samples that read
`● Finagling… thinking` carry nothing that moves at all. And an agent whose
hooks are flowing is read from its hooks and not from its screen. The reading
is what is left when the hooks stop — an agent interrupted with Escape, or one
nobody has heard from — and on a narrow pane what is left now says a working
agent has finished.

A transcript can also put `s · ` back by accident. Driven at 24 columns with a
tool call above the box, `● Sleeping for 300 seconds · 42s` — claude's own
tool header with the elapsed time on it — carried the fragment the spinner row
had dropped, and the rule held with a span of 8 rows between two rows that have
nothing to do with each other. That is the same rule holding for the wrong
reason, which is why the clean measurement above was driven on a pane with
nothing but the spinner on it.

**The other half: `s · ` is now on the idle screen.** When a turn is over
2.1.259 leaves this behind:

    ✻ Cogitated for 2m 6s · done 10:09 AM

The rule's comment says of this line: "same glyph, no ellipsis and no
parenthesis", and that neither fragment is on it. One of them is on it now —
`6s · done` carries `s · ` — measured at all five widths. The rule still walks
past, because `all` wants both and `… (` is genuinely absent. So the finished
line has gone from carrying neither anchor to carrying one, and the whole of
what keeps a finished agent from reading `working` is a single punctuation
fragment.

The same two fragments are `[furniture] spinner`, which is what
`src/furniture.rs` walks over to find the rows an agent earned. That was not
driven this round and is the obvious next thing to drive: a spinner row the
walk cannot recognise is a row of the vendor's chrome printed as the agent's
own output.

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

## What this pass found

Five findings, in the order of what they cost.

1. **A running turn reads `idle` below 40 columns.** The spinner row drops the
   parenthesis and its `·` as the pane narrows, both of the rule's anchors go
   with it, and `idle_prompt` claims the screen: 16 of 16 samples across a live
   turn at 30 and 24 columns. Four agents on a 160-column terminal is 40
   columns each, and a fifth takes them under it. Naming a screen non-blocking
   also clears any pending question off the row, so this is the failure the
   spinner rule's own comment calls the worst one available.
2. **A claude on its folder-trust gate reads `unknown` at 24 columns.** The
   screen spans 19 rows where `within` allows 16. `setup` is a flag on the rule
   rather than on the record, so an unclaimed screen is not a gate: `doctor`
   reports nothing wrong and the trust-key remedy is never offered, about an
   agent that will sit there until somebody attaches.
3. **The trust screen's choices lost their numbers, and its question reads back
   as an answer.** No `❯ 1.` anywhere on it, so `asks` finds `trust` on
   `Yes, I trust this folder` instead of on the safety-check sentence, and hands
   that back as the question with no options beside it.
4. **Every key the answer grammar allows at that screen is wrong.** `1`, `2`
   and `y` do nothing; `n` and `enter` exit the agent, because the cursor now
   opens on `No, exit`. The way to yes is an arrow key, which is not in the
   grammar. The store write in `src/trust.rs` is what keeps this off the usual
   day, and it is unaffected — it never touches the screen.
5. **The line a finished turn leaves behind now carries `s · `.** `✻ Cogitated
   for 2m 6s · done 10:09 AM`, at all five widths. The rule wants both of its
   fragments and `… (` is still absent, so nothing reads wrong today; what is
   gone is the margin, on the rule its own comment calls the first one to
   re-measure at a vendor bump.

Two things were not driven and should not be read as measured here: the
review-answers screen that both multi-part shapes of `AskUserQuestion` end on,
which the document already records as claimed by nothing; and
`[furniture] spinner`, which shares the two fragments finding 1 is about.
