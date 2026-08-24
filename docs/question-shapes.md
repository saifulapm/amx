# The vendor's question screens

`AskUserQuestion` is the one blocking prompt whose contents the agent wrote
itself, and it is not one screen. The menu in `assets/screen-rules.toml` — a
question, numbered choices, a rule, `N. Chat about this`, a footer — is only its
plainest shape. Four more are in the tool's own schema and all four reach a
pane: a checkbox list, several question tabs in one call, a note beside a
choice, and the free-text `Other` row. A fifth, the Submit tab, is not in the
schema at all; it is where the vendor sends you when the first two shapes are
answered, and no rule amx ships claims it.

This file is the measurement those shapes are owed before any code depends on
them, in the form `assets/screen-rules.toml`'s anchor law asks for: the capture,
where it was taken, the payload beside it, and the keys that drive it. Nothing
here was read off the vendor's source or inferred from a screen amx drew for
itself.

Sections 6 to 8 are a second sitting, on 2026-08-25, and they are about the
parts amx got wrong rather than the parts the vendor draws: which events say a
menu is up, what the vendor does with keys sent faster than it redraws, and what
happens when the two rows the payload does not name are pressed.

## How these were measured

One `claude 2.1.240` in a detached tmux session on 2026-08-24, driven with
`send-keys` and read with `capture-pane -p -J` — the same call `tmux.rs`'s
`capture` makes, so the rows below are the rows a reader would match against.
Widths were changed with `resize-window` and the vendor redraws on the resize,
so a 24-column capture is a real render at 24 columns rather than a reflow of a
wider one. Trailing spaces are stripped in this file, and the two full-width
rules that bracket a box are cut short to fit the page; on the pane they run its
whole width. Nothing else is touched, and where rows above or below a box are
left out it says so, because on two of these screens what is *not* drawn is the
finding.

The payloads are the vendor's own, not transcriptions: a `PreToolUse` hook
matching `AskUserQuestion` in the probe project appended the hook's stdin to a
file, and a `PostToolUse` hook did the same for the answer. Every `questions`
block below is that hook's `tool_input`, and every `answers`/`annotations` block
is its `tool_response`.

The 2026-08-25 sitting was driven through amx itself rather than by hand: `amx
new` on the same probe project, one agent per shape, at 220 columns. Its hook
log is the whole event stream and not the two `AskUserQuestion` events, and the
answers below are `amx answer` typing at the pane rather than a person. One
thing that took an hour to find is worth saying here: this machine's own
`~/.claude/settings.json` already wires amx's hooks, so a probe project that
wires them again gets every payload folded twice, once by each binary. The
second fold is by whatever `amx` is installed, which is not the one under test.

That response carries three keys and not two. `questions` — the whole call, as
it went in — sits beside `answers` and `annotations`, on both a one-question and
a three-question call measured on 2026-08-24 against v2.1.240. So what was asked
can be read off the answer coming back, not only off the question going out.

## What every question screen shares

A full-width rule, a header strip, the question, the numbered choices, a second
rule, `Chat about this`, and a footer. The footer is the anchor `ask_menu`
keys on and it survived every shape measured, but its wording is not one string:

    Enter to select · ↑/↓ to navigate · Esc to cancel
    Enter to select · Tab/Arrow keys to navigate · Esc to cancel   (>1 question)
    Enter to select · ↑/↓ to navigate · n to add notes · Esc to cancel
                                                        (an option has a preview)

and where the cursor is on a text row — the `Other` row or the notes field — it
gains one more term:

    Enter to select · ↑/↓ to navigate · ctrl+g to edit in Kak · Esc to cancel

That term belongs to two rows rather than one. Stepped down a checkbox screen a
row at a time on 2026-08-24 against v2.1.240, with the footer read at each stop:

| cursor on | footer |
| --- | --- |
| `3. [ ] Tests` | `Enter to select · ↑/↓ to navigate · Esc to cancel` |
| `4. [ ] Type something` | the same, with `ctrl+g to edit in Kak` before `Esc to cancel` |
| `Submit` | the same again, the term still there |

and walking back up drops it at the choice rows again. So it is not evidence
that the cursor is in text: it is drawn on the field's row and on the
`Submit`/`Next` row under it, which is the row that ends the answer.

`Kak` there is this machine's `$EDITOR`, so that term names a local setting and
can never be an anchor. What every one of them opens with is `Enter to select`,
which is why the rule anchors there and not on `esc to cancel`: at 24 columns
the footer wraps and only the opening fragment is guaranteed contiguous.

The header strip has two forms. One question with a single answer draws just the
checkbox and the header:

     ☐ License

Anything that needs an explicit submit — more than one question, or one
multi-select question — draws a tab strip with arrows and a Submit tab:

    ←  ☐ Runtime  ☐ Storage  ☐ Rollout  ✔ Submit  →

`☐` is an unanswered tab and `☒` an answered one.

## 1. The checkbox screen

`multiSelect: true`. Measured at 220, 54 and 24 columns on 2026-08-24 against
v2.1.240.

### The capture

At 220 columns, the moment it went up. Everything above the first rule is the
transcript; what is below the footer is the finding, and it is in the next
section.

    ────────────────────────────────────────────────────────────────────────
    ←  ☐ Features  ✔ Submit  →

    Which features should be enabled?

    ❯ 1. [ ] Logging
      Write a log file
      2. [ ] Metrics
      Export counters
      3. [ ] Tracing
      Emit spans
      4. [ ] Type something
         Submit
    ────────────────────────────────────────────────────────────────────────
      5. Chat about this

    Enter to select · ↑/↓ to navigate · Esc to cancel

The same screen at 24 columns, part-answered. The choices do not reflow, they
keep their `[ ]`, and the tab strip comes to exactly 24 columns with a header
this short and survives whole. Only the question and the footer wrap.

    ────────────────────────
    ←  ☒ Checks  ✔ Submit  →

    Which checks should the
    hook run?

    ❯ 1. [✔] Format
      Run rustfmt
      2. [ ] Clippy
      Run the linter
      3. [✔] Tests
      Run cargo test
      4. [ ] Type something
         Submit
    ────────────────────────
      5. Chat about this

    Enter to select · ↑/↓ to
    navigate · Esc to
    cancel

Four things this shape does that the plain menu does not:

* the box is `[ ]` or `[✔]` between the number and the label, so the label a
  reader takes off the row is `[ ] Logging` and not `Logging`;
* the `Other` row reads `Type something` with **no** trailing full stop, where
  the plain menu's reads `Type something.` with one;
* an unnumbered `Submit` row sits under the `Other` row, inside the box, at the
  indent a description would use;
* a description under a checkbox choice is indented two columns, against five
  under a plain one.

### The payload

    {
      "questions": [
        {
          "question": "Which features should be enabled?",
          "header": "Features",
          "options": [
            { "label": "Logging", "description": "Write a log file" },
            { "label": "Metrics", "description": "Export counters" },
            { "label": "Tracing", "description": "Emit spans" }
          ],
          "multiSelect": true
        }
      ]
    }

Neither the `Other` row nor `Chat about this` nor `Submit` is in it. They are
the vendor's furniture, added to every menu it draws, so the screen carries two
more numbered rows than the record has options, and on this shape a third,
unnumbered one under them.

### The keystrokes

| key | what it does |
| --- | --- |
| `↑` `↓` | move the cursor: choices, then the `Other` row, then `Submit`, wrapping |
| `Enter` | toggle the choice under the cursor. It does **not** submit |
| `Space` | the same toggle. Not named in the footer |
| `1`–`9` | toggle that choice without moving the cursor, unless the cursor is in the `Other` row, where it is a character |
| `→` | leave the choices for the Submit tab |
| `Enter` on `Submit` | the same |

So two choices and a submit is `1`, `3`, `→`, `Enter` — the `Enter` being the
one on the review screen in section 5.

The `Other` row is a text field, and on a checkbox screen it behaves unlike the
rest of the list:

* typing into it **checks it by itself** — `❯ 4. [✔] Audit`, and the tab goes
  `☐` → `☒` — so the text alone is the answer;
* `Enter` on it then **unchecks it again**, because `Enter` is still the toggle;
* while it has the cursor, `←` and `→` are text-editing keys, not tab keys.

`Up`, `paste`, `Enter` — what `answer.rs`'s `say` sends today — therefore types
the words, checks the row, and unchecks it, leaving the prompt up and
unanswered. The sequence that works is `Up`, paste, `↓`, `Enter`, `Enter`.

### What answering it produced

    "answers": { "Which features should be enabled?": "Logging, Tracing" }

One string, the labels joined with `, `. Typed text comes back the same way:
`"Which checks should run?": "Audit"`.

**The labels are in the order the boxes were checked, not the order the payload
offers them.** Measured on 2026-08-24 against v2.1.240, on the same `Format`,
`Clippy`, `Tests` question as the 24-column capture above and driven at 220:
`Enter` on `3. Tests` and then the digit `1` for `Format` drew
`→ Tests, Format` on the review tab and recorded

    "answers": { "Which checks should the hook run?": "Tests, Format" }

with `Format` still option one in the payload. So the string carries the
keystrokes that produced it, and two callers who chose the same boxes in
different orders hand the agent two different strings.

## 2. The multi-tab screen

Three questions in one call. Measured at 220, 80, 54, 40 and 24 columns on
2026-08-24 against v2.1.240.

### The capture

    ────────────────────────────────────────────────────────────────────────
    ←  ☐ Runtime  ☐ Storage  ☐ Rollout  ✔ Submit  →

    Which runtime should the service target?

    ❯ 1. Node
         Widest library support
      2. Deno
         Batteries included
      3. Type something.
    ────────────────────────────────────────────────────────────────────────
      4. Chat about this

    Enter to select · Tab/Arrow keys to navigate · Esc to cancel

Only the tab strip and the footer tell it from a one-question menu. The strip is
the first thing to go as the pane narrows, and it goes by eliding the headers
rather than by wrapping. At 40 columns:

    ←  ☐ Runtime  ☐ St…  ☐ Ro…  ✔ Submit  →

and at 24, where the strip finally takes two rows and the *current* tab's own
header is elided to nothing but the ellipsis:

    ────────────────────────
    ←  ☐  ☐ S… ☐ R… ✔      →
      …            Submit

    Which runtime should the
    service target?

    ❯ 1. Node
         Widest library
         support
      2. Deno
         Batteries included
      3. Type something.
    ────────────────────────
      4. Chat about this

    Enter to select ·
    Tab/Arrow keys to
    navigate · Esc to cancel

**No reader can count or name the tabs from a narrow pane.** At 24 columns three
headers are `☐`, `S…` and `R…`, and at every width the strip says nothing about
which tab of how many is showing beyond the position of the marker. How many
tabs there are, what each is called and which are answered is in the payload and
only there.

A tab already answered marks its chosen row with a trailing `✔` when you come
back to it:

    ←  ☒ Runtime  ☐ Storage  ☐ Rollout  ✔ Submit  →

    Which runtime should the service target?

    ❯ 1. Node ✔
         Widest library support
      2. Deno

so a reader that takes labels off the screen gets `Node ✔` for the answered one.

### The payload

    {
      "questions": [
        {
          "question": "Which runtime should the service target?",
          "header": "Runtime",
          "options": [
            { "label": "Node", "description": "Widest library support" },
            { "label": "Deno", "description": "Batteries included" }
          ],
          "multiSelect": false
        },
        {
          "question": "Which store should hold sessions?",
          "header": "Storage",
          "options": [
            { "label": "Redis", "description": "Fast, volatile" },
            { "label": "Postgres", "description": "Durable, already deployed" }
          ],
          "multiSelect": false
        },
        {
          "question": "Which rollout steps should run?",
          "header": "Rollout",
          "options": [
            { "label": "Canary", "description": "Five percent first" },
            { "label": "Migrate", "description": "Run the schema change" },
            { "label": "Announce", "description": "Post to the channel" }
          ],
          "multiSelect": true
        }
      ]
    }

One call, three questions, and `multiSelect` per question rather than per call:
a checkbox tab and two plain ones sit in the same prompt, and the keys that
drive one tab are not the keys that drive the next. `hook.rs` keeps
`questions[0]`, so on this payload amx holds one third of what it was told and
none of the shape of the rest.

### The keystrokes

| key | what it does |
| --- | --- |
| `Enter` on a choice of a plain tab | records it and **advances to the next tab** |
| `Enter` on a choice of the last plain tab | advances to the Submit tab |
| `→` / `Tab` | next tab |
| `←` | previous tab, answer kept |
| `1`–`9` on a plain tab | selects that choice and submits the tab at once |

Answering never returns to the composer until the Submit tab is confirmed, which
is what "answering one tab leaves the next pending" has to mean on the record: a
prompt that is still up.

#### A checkbox tab with a question behind it

`multiSelect` is per question, so one call holds both kinds of tab and the keys
that finish one do not finish the next. Measured on 2026-08-24 against v2.1.240
at 220 columns, on the three questions above with `Rollout` moved from third to
second so that a question follows the checkbox tab; options, descriptions and
flags otherwise unchanged. `Enter` on `Node` answered the first tab and left the
second showing:

    ←  ☒ Runtime  ☐ Rollout  ☐ Storage  ✔ Submit  →

    Which rollout steps should run?

    ❯ 1. [ ] Canary
      Five percent first
      2. [ ] Migrate
      Run the schema change
      3. [ ] Announce
      Post to the channel
      4. [ ] Type something
         Next
    ────────────────────────────────────────────────────────────────────────
      5. Chat about this

    Enter to select · Tab/Arrow keys to navigate · Esc to cancel

The boxes, the descriptions indented two columns and the `Type something` with
no full stop are section 1's checkbox screen to the letter. **The unnumbered
row under the `Other` row is not.** It reads `Next` here and `Submit` there, and
what differs between the two is position: `Submit` when no question follows,
`Next` when one does. So that row names where the tab leads, never what kind of
screen is drawing it, and neither word tells a reader it is looking at
checkboxes.

| key | what it does on a checkbox tab |
| --- | --- |
| `Enter` on a choice | toggles it, and does **not** advance |
| `1`–`9` | toggles that choice, cursor unmoved, tab unadvanced |
| `→` / `Tab` | the next question's tab, not the Submit tab |
| `←` | back a tab, the boxes kept |
| `Enter` on `Next` | the next question's tab |

The first two rows are the plain tab's own keys inverted. There `Enter` records
and advances and a digit selects and submits the tab at once; here both toggle
and neither moves on. The tab's mark flips `☐` to `☒` on the first box checked
rather than on leaving the tab, so the strip says a tab has been touched and not
that it is finished.

Driven end to end the call took `Enter` on `Node`, then `2` and `Enter` on the
`Rollout` tab, then the tab-moving keys in the table above — `→`, `←`, four `↓`
and `Enter` on `Next` — which left both answers alone, then `Enter` on `Redis`,
then `Enter` on the review tab. What came back is at the end of this section.

### What answering it produced

    "answers": {
      "Which runtime should the service target?": "Node",
      "Which store should hold sessions?": "Redis",
      "Which rollout steps should run?": "Canary, Announce"
    }

Keyed by the question text, verbatim — not by header, not by index. Two
questions with the same text would collide in the vendor's own answer map.

The reordered call driven above came back the same way, one entry per tab, keyed
by its question's text:

    "answers": {
      "Which runtime should the service target?": "Node",
      "Which rollout steps should run?": "Migrate, Canary",
      "Which store should hold sessions?": "Redis"
    }

`Migrate, Canary` is the order the two boxes were checked and not the order the
payload lists them in — section 1's finding again, on a tab this time rather
than on a screen of its own.

## 3. The `Other` row

Every menu the tool draws carries one free-text row as its last choice, above
the rule. Measured at 220, 80, 54 and 24 columns on 2026-08-24 against v2.1.240.

### The capture

At rest it is a numbered choice like any other, and nothing in the payload
accounts for it — or for `Chat about this` under the rule:

     ☐ License

    Which license should the LICENSE file contain?

    ❯ 1. MIT
         Short and permissive
      2. Apache-2.0
         Permissive with a patent grant
      3. Type something.
    ────────────────────────────────────────────────────────────────────────
      4. Chat about this

    Enter to select · ↑/↓ to navigate · Esc to cancel

Typed into, it is the text, in place. The label is gone — there is no row on the
screen that still says `Type something.` — and the footer has gained the editor
term:

      1. MIT
         Short and permissive
      2. Apache-2.0
         Permissive with a patent grant
    ❯ 3. BSD-3-Clause
    ────────────────────────────────────────────────────────────────────────
      4. Chat about this

    Enter to select · ↑/↓ to navigate · ctrl+g to edit in Kak · Esc to cancel

### The payload

    {
      "questions": [
        {
          "question": "Which license should the LICENSE file contain?",
          "header": "License",
          "options": [
            { "label": "MIT", "description": "Short and permissive" },
            { "label": "Apache-2.0", "description": "Permissive with a patent grant" }
          ],
          "multiSelect": false
        }
      ]
    }

Two options; three rows above the rule. Nothing in the payload says the row is
there, so its position can only be counted from the end.

### The keystrokes

`Up` from the first row wraps onto it — reconfirmed against v2.1.240 on
2026-08-24 on both a plain menu and a checkbox one, which is what
`answer.rs`'s `TO_THE_FIELD` rests on. `Chat about this` sits below the rule and
is not in that ring, so the wrap lands on the field and not past it.

Then type, then `Enter` on a plain menu. On a checkbox menu, `↓` `Enter`
instead — see section 1.

**A digit is not the same key twice.** On a plain menu a digit at a choice
selects it and submits at once, but once the cursor is on the `Other` row every
key including a digit is a character in the field:

    ❯ 3. Type something.     press `2`
    ❯ 3. 2                   and `Enter` answers the literal "2"

which is what the record showed: `"Which editor should the docs mention?": "2"`.
Any answer that walks the cursor onto the field has spent its digits.

### What answering it produced

    "answers": { "Which license should the LICENSE file contain?": "BSD-3-Clause" }

Typed text arrives in the same slot as a label, with nothing marking it as the
caller's own words rather than one of the offered choices.

## 4. Notes

A note is not a shape of its own: it is a field the vendor adds when an option
carries a `preview`, and it comes with a layout that breaks every other reading
on this page. Measured at 220, 80, 54 and 24 columns on 2026-08-24 against
v2.1.240.

### The capture

At 220 columns, before the note is opened:

     ☐ Layout

    Which header layout should the page use?

    ❯ 1. Stacked                      ┌──────────────────────────────────────────┐
      2. Inline                       │ +----------+                             │
                                      │ | TITLE    |                             │
                                      │ | subtitle |                             │
                                      │ +----------+                             │
                                      └──────────────────────────────────────────┘

                                      Notes: press n to add notes

    ────────────────────────────────────────────────────────────────────────
      Chat about this

    Enter to select · ↑/↓ to navigate · n to add notes · Esc to cancel

Three departures, all of them costly:

* **the choices share their rows with the preview box**, so the label on row
  `❯ 1.` is `Stacked` followed by spaces and then `┌───…┐`;
* **there is no `Other` row.** A previewed question has no free text;
* **`Chat about this` loses its number.** It is `  Chat about this`, not
  `  3. Chat about this`, so a reader counting numbered rows does not see it.

After `n`, the field takes the cursor and shows its placeholder, and the footer
gains the editor term:

                                      Notes: Add notes on this design…

    ────────────────────────────────────────────────────────────────────────
      Chat about this

    Enter to select · ↑/↓ to navigate · n to add notes · ctrl+g to edit in Kak · Esc to cancel

Typed, and after `Escape` has put the cursor back on the choices:

    ❯ 1. Stacked                      ┌──────────────────────────────────────────┐
      2. Inline                       │ +----------+                             │
                                      │ | TITLE    |                             │
                                      │ | subtitle |                             │
                                      │ +----------+                             │
                                      └──────────────────────────────────────────┘

                                      Notes: prefer the stacked one

At 54 columns the label itself breaks, the colon landing on the row below, and
the note is drawn truncated:

    ❯ 1. Stacked               ┌──────────────────┐
      2. Inline                │ +----------+     │
                               │ | TITLE    |     │
                               │ | subtitle |     │
                               │ +----------+     │
                               └──────────────────┘

                               Notes keep the subtitle on…
                               :

So `notes:` is not an anchor at any width worth having, and **the note's own
text is not on the screen** once it is longer than the column the preview left
for it: `keep the subtitle on…` is all that is drawn of a note the record holds
whole.

At 24 columns the layout comes apart. The box takes the whole 50-row pane — rows
4 to 50 — because the preview column is two characters wide and the box stretches
to hold what will not fit, with the middle cut out and marked:

     4|────────────────────────
     5| ☐ Layout
     6|
     7|Which header layout
     8|should the page use?
     9|
    10|❯ 1.           ┌──┐
    11|    Stacked    │  │
    12|  2. Inline    │  │
    13|               │  │
      …               (22 more rows of it)
    35|               ├─── ✂
    36|               ─── 24
    37|               lines
    38|               hidden ┤
    39|               └──┘
    40|
    41|               Not keep…
    42|               es:
    43|
    44|────────────────────────
    45|  Chat about this
    46|
    47|Enter to select · ↑/↓ to
    48|navigate · n to add
    49|notes · ctrl+g to edit
    50|in Kak · Esc to cancel

The choice's own label has left the row its number is on — `❯ 1.` on row 10 and
`Stacked` on row 11 — and both choices sit fifteen rows above where the floor's
last 24 rows begin. The rule still holds, on the footer; there is nothing left
for it to read.

### The payload

    {
      "questions": [
        {
          "question": "Which header layout should the page use?",
          "header": "Layout",
          "options": [
            {
              "label": "Stacked",
              "description": "Title over subtitle",
              "preview": "+----------+\n| TITLE    |\n| subtitle |\n+----------+"
            },
            {
              "label": "Inline",
              "description": "Title beside subtitle",
              "preview": "+---------------------+\n| TITLE - subtitle    |\n+---------------------+"
            }
          ],
          "multiSelect": false
        }
      ]
    }

`preview` on any option is what turns the note field on. The descriptions are in
the payload but the previewed layout does not draw them at all.

### The keystrokes

| key | what it does |
| --- | --- |
| `n` | put the cursor in the notes field. A no-op on a menu with no preview |
| typing | the note, in place |
| `Escape` | leave the field, keeping the note. It does **not** cancel the prompt |
| `Enter` *from the field* | submit the note **with no choice at all** |
| `Enter` on a choice, after `Escape` | submit the choice with the note attached |

The fourth row is the trap. Pressing `Enter` while the field still has the
cursor is a complete answer to the vendor and the record shows it:

    "answers":     { "Which header layout should the page use?": "(notes only)" },
    "annotations": { "Which header layout should the page use?": {
                       "notes": "keep the subtitle one line" } }

`(notes only)` is a literal the vendor writes into the answer slot. Escaping out
of the field first and then choosing gives the shape a caller means:

    "answers":     { "Which header layout should the page use?": "Stacked" },
    "annotations": { "Which header layout should the page use?": {
                       "preview": "+----------+\n| TITLE    |\n| subtitle |\n+----------+",
                       "notes":   "prefer the stacked one" } }

The chosen option's own `preview` rides back in `annotations` beside the note,
keyed — like `answers` — by the question's text.

`n` was pressed on a checkbox menu with no previews and nothing happened, which
is the refusal `amx answer --note` owes a question that offers no such thing.

## 5. The Submit tab, which no rule claims

Both shapes that need an explicit submit end here. It is not in the tool's
schema; it is the vendor's own confirm step.

    ────────────────────────────────────────────────────────────────────────
    ←  ☒ Runtime  ☒ Storage  ☒ Rollout  ✔ Submit  →

    Review your answers

     ● Which runtime should the service target?
       → Node
     ● Which store should hold sessions?
       → Redis
     ● Which rollout steps should run?
       → Canary, Announce

    Ready to submit your answers?

    ❯ 1. Submit answers
      2. Cancel

That is the whole screen at 220 columns and there is nothing under it. **It
draws no footer** — not `Enter to select`, not `Esc to cancel`, and not the mode
footer either, since it is a blocking prompt. Measured at 220, 54 and 24
columns; at 24 the tab strip elides and `Ready to submit your` / `answers?`
wraps, while `Review your answers` and `❯ 1. Submit answers` both stay whole:

    ────────────────────────
    ←  ☒   ☒ S… ☒   ✔
      R…       R…  Submit  →

    Review your answers

     ● Which runtime should
       the service target?
       → Node
      …                        (the other two, wrapped the same way)
    Ready to submit your
    answers?

    ❯ 1. Submit answers
      2. Cancel

`Enter` on `1. Submit answers` sends every tab's answer at once. `←` goes back
to the last question tab with its answer intact.

No rule in `assets/screen-rules.toml` claims this screen. `ask_menu` needs
`enter to select` and there is no footer to carry it; `plan_approval` needs
`ready` *and* `execute` and only `ready` is here; the trust and permission
anchors are absent; `idle_prompt` needs the mode footer, which a blocking prompt
never draws. An agent parked one keystroke from finishing an answer reads
`unknown`, with no question on its row.

If a rule is written for it, the anchor these captures support is
`review your answers`: 19 columns wide, the fragment its row opens with, and
whole at every width measured. `❯ 1.` or `submit answers` is the affordance, and
`not_below` carries the same weight as it does on its neighbours — this box ends
at `2. Cancel` with nothing beneath. Adding it means adding its name to
`rules_the_bundled_file_is_the_ruleset` in `src/rules.rs`, which this task does
not own; it belongs with the work that does.

## 6. The three events behind one menu

The screen above is one screen. The vendor fires three events about it, and two
of them are about a permission box that does not exist. Measured on 2026-08-25
against v2.1.240, once in manual permission mode and once in auto, with a hook
on every event appending its stdin to a file:

| when | event | what it carries |
| --- | --- | --- |
| `20:38:53.622` | `PreToolUse` | `tool_name` `AskUserQuestion`, and `tool_input` holding every question the menu will ask |
| `20:38:53.632` | `PermissionRequest` | `tool_name` `AskUserQuestion`, and the same `tool_input` |
| `20:38:59.650` | `Notification` | `notification_type` `permission_prompt`, and the message `Claude needs your permission` |

The gap between the first two was 10 ms in manual mode and 27 ms in auto. The
notification is six seconds behind, which is this vendor's own timer and not a
mode. **The permission mode makes no difference at all**: the vendor asks itself
for leave to use its own question tool either way, and `PermissionDenied` never
fires — the box is never shown, because there is no box.

So the event that knows what is being asked arrives first, and the two that know
least arrive last. amx folded each one over the last, and what an agent standing
at a menu read was this:

    "kind": "permission",
    "multi": false,
    "questions": 0,
    "options": ["[ ] Logging", "[ ] Metrics", "[ ] Tracing",
                "[ ] Type something", "Chat about this"]

`permission` because the notification's own type said so; `questions: 0` because
writing the notification's words retired the call; and the five options because
the words it wrote were `Claude needs your permission`, which is a placeholder a
reader forgets, and forgetting it sent the reader to the pane for the rows.
That is 02BQ6442 whole: the grammar amx offered was a permission box's, and the
numbering was the screen's five rows rather than the question's three.

Folding the same three payloads through the fixed hook leaves:

    "kind": "question",
    "multi": true,
    "questions": [ { "header": "Features", "multi": true, "options": [ … ] } ],
    "options": ["Logging", "Metrics", "Tracing"]

## 7. What the vendor does with keys sent faster than it draws

This is the one that cost an answer without saying so, and no stand-in can find
it: a mock claude reads its pty and never redraws, so every sequence amx has
ever sent passes against one and some of them do nothing against the real thing.

Driven on the checkbox menu of section 1 at 220 columns on 2026-08-25. Each
round starts with both boxes clear, sends `1`, `3`, `→`, `←`, and reads back how
many of the two boxes survived the trip to the Submit tab and home again:

| how the keys were sent | rounds that kept both boxes |
| --- | --- |
| one `send-keys` call carrying both digits | 0 of 3 |
| a `send-keys` call each, no pause | 4 of 6 |
| a `send-keys` call each, 50 ms apart | 16 of 16 |

Two separate findings sit in that table.

**Several keys in one call are not several keypresses.** `tmux send-keys -t %3
1 3` writes both into the pty in one go, and the menu took neither: three rounds
of three, the tab still `☐`, and the Submit tab drawing `⚠ You have not answered
all questions`. `tmux send-keys -t %3 Right Enter` was worse than losing a key —
the `Right` moved to the Submit tab and the `Enter` was answered against the tab
it had just left, unchecking a box that was already checked, so the review tab
showed `→ Tracing` where two boxes had been ticked.

**A call each is necessary and not sufficient.** Back to back with nothing
between them, two rounds of six lost both digits. The gap that a process start
provides is not reliably a gap the vendor gets to draw in. Fifty milliseconds
between calls was clean over sixteen rounds, which is what `answer.rs` now
leaves: an answer is at most six keys, so a third of a second at the pane
against an answer that silently does not take.

None of this is reported by anything. The prompt stays up, amx writes down that
the question was answered, the record says the agent is back at work, and
whoever asked stops watching a row that will never move again.

### What it looks like driven properly

`amx answer <id> 1,3` at the checkbox menu, and the vendor's own transcript:

    ● User answered Claude's questions:
      ⎿  · Which features should be enabled? → Logging, Tracing

with `"answers": { "Which features should be enabled?": "Logging, Tracing" }` in
the `PostToolUse` payload. `amx answer <id> --text BSD-3-Clause` at the plain
menu of section 3 gave `→ BSD-3-Clause`. The three-question call of section 2,
answered `1`, then `1`, then `1,3`, advanced a tab at a time — the record showing
`Runtime: Node`, then `Storage: Redis`, with the prompt still up between them —
and the last answer pressed the vendor's own Submit tab:

    ● User answered Claude's questions:
      ⎿  · Which runtime should the service target? → Node
         · Which store should hold sessions? → Redis
         · Which rollout steps should run? → Canary, Announce

## 8. Pressing the two rows the payload does not name

Section 3 records that a digit on the `Other` row is a character in the field.
What a digit *at* that row does, from a choice, is a different question, and it
is the one a caller counting rows off the pane asks. Measured on 2026-08-25 at
220 columns.

On a plain menu of two choices, `3` is `Type something.`, and pressing it moves
the cursor onto the field and stops:

    ❯ 3. Type something.
    Enter to select · ↑/↓ to navigate · ctrl+g to edit in Kak · Esc to cancel

Nothing is submitted and the prompt is exactly where it was. So an `amx answer
<id> 3` that pressed the digit would type a key, write `3` down as the answer,
report the agent back at work, and leave it standing at an empty text field.

On a checkbox menu of three choices, `4` is `Type something` and pressing it
**checks the empty field**:

      3. [ ] Tracing
      4. [✔] Type something

with the cursor still on row 1 and the tab flipping `☐` → `☒`. Submitting from
there sends an empty string as the answer.

`Chat about this` was not pressed. Its own label says where it goes, and it is
not a row an answer to the question can land on.

Neither row is an answer amx can make, so `amx answer` now names them instead of
pressing them: the free-text row points at `--text`, and anything past it is
refused against the count the payload carries.

## What the shipped ruleset makes of these

Every capture above was run through the bundled ruleset's own matcher. It was
run outside the crate's tests, because this task owns
`assets/screen-rules.toml` and not `src/rules.rs`; the matcher was checked first
against the 22 captures already in `rules.rs`'s tests, where it reproduced the
rule each one matches and the question read off it.

| screen | rule | phase | question read off the pane |
| --- | --- | --- | --- |
| plain menu, 220/80/54/24 | `ask_menu` | waiting | correct, all four widths |
| checkbox, 220/54/24 | `ask_menu` | waiting | text correct; labels come back `[ ] Format`, `[✔] Tests` |
| checkbox, box high on the pane | `ask_menu` | waiting | **none** |
| multi-tab, 220/80/54/40/24 | `ask_menu` | waiting | tab one's text and choices only; nothing about the other tabs |
| `Other` row typed | `ask_menu` | waiting | correct, the typed text read as a label |
| previewed, 220/80 | `ask_menu` | waiting | text correct; labels carry the preview box's rows |
| previewed, 54 | `ask_menu` | waiting | text correct; labels carry the box |
| previewed, 24 | `ask_menu` | waiting | **none** |
| Submit tab, 220/54/24 | — | **unclaimed** | — |

The rule itself came through the vendor bump and all four shapes without a
change: `enter to select` is on every question screen at every width measured,
and `not_below` was never wrong. What the rule cannot do is read the screen. Two
rows of that table come out intact, both of them the plain menu; three read back
labels with the vendor's own drawing stuck to them; three read back nothing at
all; and the multi-tab row reads its first tab correctly and is silent about the
two behind it, which is the worst of the nine, because it looks right.

### The floor is above the question when the transcript is short

`checkbox, box high on the pane` is not a narrow-pane case. It is the ordinary
one: the very first capture of the session, a 50-row pane, the box drawn at rows
17 to 33 and **17 blank rows under the footer**, because the vendor pads the
screen it drew rather than sitting at the bottom of it. `FLOOR_LINES` is 24, so
the floor began at row 27 — seven rows below the question and five below
`❯ 1.`. `ask_menu` still ruled `waiting`, on `esc to cancel` in the footer, but
`first_option` found no row numbered 1 and the question came back empty.

Later captures in the same session, at 54 and 24 columns, had enough transcript
above them to push the box to the bottom of the pane, and read correctly. So
whether a caller gets the question depends on how much the agent had said before
it asked, which is not a property of the question.

### What the next tasks take from this

Five of these are now spent. The kind precedence, the numbering and the three
keystroke findings are in `hook.rs`, `derive.rs` and `verbs/answer.rs`, driven
against a live 2.1.240 and covered by `cargo test answer`. What is left is
marked below.

* the tabs, the descriptions, the `multiSelect` flag and the note are all in the
  payload and none of them can be read off a narrow pane. The record should hold
  the payload's version;
* the row count on the screen is the option count plus the `Other` row plus
  `Chat about this`, plus `Submit` on a checkbox screen, except on a previewed
  question where there is no `Other` row and `Chat about this` has no number.
  Nothing on the screen distinguishes the vendor's rows from the agent's;
* `Up`, paste, `Enter` answers a plain menu's `Other` row and unchecks a
  checkbox one. `↓`, `Enter`, `Enter` finishes the checkbox case — and every one
  of those keys needs a call and a pause of its own, per section 7;
* a digit answers a plain menu, toggles a checkbox menu, and types a character
  once the cursor is on the free-text row. Past the question's own choices it
  answers nothing at all, per section 8;
* the `PostToolUse` payload echoes `questions` beside the answers, so a record
  that missed the call going out can still take the tabs and their options off
  the answer coming back;
* a multi-select answer is one string in the order the boxes were checked, so
  the record cannot recover which options they were by matching the payload's
  order, and an answer that reproduces a recorded one has to toggle in the
  recorded order to get the same string back;
* no one sequence drives a whole call. On a plain tab `Enter` records and
  advances and a digit submits the tab outright; on a checkbox tab in the same
  call both only toggle, `→` reaches the next question rather than the Submit
  tab, and the row under the `Other` row reads `Next` where a last tab's reads
  `Submit`. Which of the two a tab is is `multiSelect`, which is in the payload
  and not on the strip;
* the Submit tab needs a rule, and the question read at the moment `waiting` is
  first concluded needs to come from somewhere other than the last 24 rows of a
  half-empty pane. **Still open**: both belong to whatever owns `src/rules.rs`;
* a menu fires a permission event and a permission notification of its own, so
  nothing that reads either of them alone can tell a menu from a box. What tells
  them apart is the tool call in front, per section 6.
