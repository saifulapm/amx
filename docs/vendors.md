# Vendors

amx runs coding agents it did not write, and everything it knows about one is
an entry in a table: `src/vendor/`. This file says what an entry carries, what
the rest of amx asks of it, and what adding one takes. It is written for the
day a second real vendor lands — opencode, pi, whichever arrives first — so
that landing is an afternoon of measurement and one new file, not a
re-architecture.

## The shape: a descriptor, not a trait

A vendor is a `Vendor` value in a static table — no dynamic dispatch, no
second implementation of anything, and no place for a vendor to hide
behaviour. What one declares is data a person can read in one sitting and
diff against the vendor's own `--help`. Where a field would want a function
is where to think again, and not before.

One real entry exists today: `claude`, in `src/vendor/claude.rs`. A test-only
second vendor, `src/vendor/second.rs`, is nobody's agent and is not in the
table; it exists so every law about the table is proved against a shape that
is not claude's. It answers most questions the other way — resumable but not
forkable, no hooks, no transcript, different flags for the same dials — which
is what keeps the machinery honest while the table has one row.

## What an entry carries

- **`name`** — the program, which is what the table is keyed by. `agent` in
  the config is a command line; the entry is found by the program it runs, so
  `agent = "claude --add-dir .."` still resolves.
- **The dials** — `model`, `permission`, `effort`, each an optional
  `DialSpec`: the values a cycle key offers, whether values off the cycle are
  legal, and the flag the vendor spells it with. The one place a dial becomes
  vendor argv is `inject`, and a flag the caller already wrote wins by the
  dial standing down.
- **`session_env`** — the variable the vendor puts in the environment of
  every process it starts, naming the conversation. It is what lets `adopt`
  know which claude it was typed inside, and it is first on the
  `not_inherited` list, because an agent that kept its spawner's session id
  would file its events under somebody else.
- **`not_inherited`** — the vendor's own variables a fresh pane must not
  inherit. The vendor's alone: what belongs to any pane is the caller's
  business.
- **`capabilities`** — what amx may ask of this vendor. The whole list:
  `Hooks`, `Transcript`, `Resume`, `Fork`, `Adopt`, `Trust`. A verb asks
  before it acts and refuses naming the gap. A vendor with no entry has none
  of these, which is the floor every unregistered command stands on: a pane
  to watch, and nothing amx pretends to know about what is in it.
- **`hooks`** — where the vendor's settings file is, the vendor's own name
  for each of the seven moments amx listens for (started, prompted, calling,
  asked, refused, notified, ended), its tool matcher, its question tool, its
  two notification types, and the sentence it writes on a permission box.
  `install` writes from this; `hook` reads by it. `None` from a vendor that
  reports nothing, and then install has nothing to wire and leaves the
  machine alone.
- **`screens`** — the document naming what this vendor's screens look like,
  in the format of `assets/screen-rules.toml`: ordered rules, each built from
  measured anchors, with the capture, the version and the date each anchor
  was read at. `None` from a vendor nobody has sat in front of yet, and then
  its pane is watched and never named. Screens are measured against a running
  program; a document written from anywhere else is a transcription.

## What the rest of amx asks

Nothing outside the table spells a vendor's flags, events, sentences or
variables. The questions the rest of amx puts to an entry:

- `spawn` — which variables to strip, which flags the dials become.
- `install` / `uninstall` / `doctor` — which settings file, which events.
- `hook` — which moment a payload's event name is, which tool is the
  question tool, how the permission sentence reads.
- `rules` / `derive` / the card and `logs` — which screens document, whose
  chrome anchors cut the furniture.
- `fork`, `resume`, `adopt`, `trust`, `logs` — may I, before anything is
  spawned. The refusal is immediate and says which capability is missing.

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
   cycles start at the sentinel, flags are distinct and are flags, a vendor
   that reports names every moment exactly once, an adoptable vendor names
   its session variable, a session variable never travels, declared screens
   parse. A new entry inherits every one.
4. **Prove the conformance.** `tests/mock_claude/` is a stand-in that replays
   scenarios — hook payloads, transcripts, screens — against the real tmux.
   A second vendor's harness takes the same shape: a fake that speaks the
   vendor's dialect, and the suite driven against it.

A vendor can also land partially, and honestly. An entry with dials and
screens but no hooks gives its agents real rows read from the pane, refusals
for `fork` and `result`'s transcript path, and everything the floor already
carries. The capabilities list is what keeps a partial entry truthful: nothing
is promised that is not there.
