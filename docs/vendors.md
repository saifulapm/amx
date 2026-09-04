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

## The shape: a descriptor, not a trait

A vendor is a `Vendor` value in a static table — no dynamic dispatch, no
second implementation of anything, and no place for a vendor to hide
behaviour. What one declares is data a person can read in one sitting and
diff against the vendor's own `--help`. Where a field would want a function
is where to think again, and not before.

Two real entries exist today: `claude`, in `src/vendor/claude.rs`, and `pi`,
in `src/vendor/pi.rs` — see [pi](#pi) for what it can do and what it cannot. A
test-only vendor, `src/vendor/second.rs`, is nobody's agent and is not in the
table; it exists so every law about the table is proved against a shape that
is neither claude's nor pi's. It answers most questions the other way —
resumable but not forkable, no hooks, no transcript, different flags for the
same dials — which is what keeps the machinery honest against a shape neither
real entry happens to take.

## What an entry carries

- **`name`** — the program, which is what the table is keyed by. `agent` in
  the config is a command line; the entry is found by the program it runs, so
  `agent = "claude --add-dir .."` still resolves.
- **The dials** — `model`, `permission`, `effort`, each an optional
  `DialSpec`: the values a cycle key offers, whether values off the cycle are
  legal, and the flag the vendor spells it with. The one place a dial becomes
  vendor argv is `inject`, and a flag the caller already wrote wins by the
  dial standing down.
- **`session`** — a `SessionSpec`, the flags that decide which session a
  process opens, or `None` from a vendor amx has measured no session
  vocabulary for. `start` is the flag that opens a session under an id amx
  minted, and is `None` from a vendor whose own report already names the
  session it opened, the way claude's Started hook does. `resume` is the flag
  that carries one on, and `joined` says whether its value rides onto it with
  `=` rather than standing as a word of its own. `conflicts` lists every other
  flag that also claims to say which session is open, so a resume or a fork
  replaces it instead of leaving two words that disagree. `fork` says how this
  vendor branches a session into a copy, for a vendor that claims `Fork`: a
  bare marker written beside `resume`, or a flag naming the origin. `new`,
  `resume` and `fork` build their session argv out of this and nothing else.
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

- `spawn` — which variables to strip, which flags the dials become, which
  flag opens a session under the id amx just minted.
- `install` / `uninstall` / `doctor` — which settings file, which events.
- `hook` — which moment a payload's event name is, which tool is the
  question tool, how the permission sentence reads.
- `rules` / `derive` / the card — which screens document, whose chrome
  anchors cut the furniture. Through `rules::bundled()` today, which is the
  first entry's document whatever vendor the pane runs: see [pi](#pi).
- `fork`, `resume`, `adopt`, `trust` — may I, before anything is spawned.
  The refusal is immediate and says which capability is missing.
- `logs` — whether this vendor keeps a conversation to read back, before it
  opens a transcript path a record carries, and which gap to name when the
  pane has gone and nothing was recorded either. The chrome it cuts off a
  fallback capture is not one of its questions: that comes off `bundled()`
  like everybody else's.

Three things stay outside the table anyway, and not because a vendor's word for
them was hardcoded somewhere it should not have been: each is the wire format
of one particular capability's implementation. The JSON keys a hook payload
arrives with — `session_id`, `hook_event_name`, `tool_name`, and the rest
`hook.rs` reads — are claude's, written there because claude is the only
vendor in the table with hooks to read. The lines inside a transcript are
claude's too: `logs::conversation` and `hook::transcript_answer` both walk one
by `type`, `user`, `assistant`, `message.content` and `text`, a shape measured
from a live claude 2.1.240 transcript, and `logs` reads `tool_use` blocks
besides. `Transcript` is a capability only claude claims. `trust`'s store,
`$CLAUDE_CONFIG_DIR/.claude.json`, is a literal in `trust.rs` for the same
reason, with a test tying that file to whichever vendors the table says can
answer the screen — `[claude]`, today. Each waits on a second vendor claiming
the capability that reads it, and pi claims none of the three.

## pi

pi is the second real entry, in `src/vendor/pi.rs`, every value in it measured
against 0.84.4 on the date it carries.

It can be resumed, forked and adopted. `--session-id <id>` is mint-or-open —
it opens the session already under that id, or creates one if none exists —
so the same flag serves as both `start` and `resume`; `pi --fork <origin>
--session-id <new>` branches into an id amx chose, which is `ForkSpec::Origin`
rather than claude's marker; and `PI_SESSION_ID`, which pi puts in the
environment of every command its bash tool runs, is what `adopt` reads to find
its way home, the same way claude's `CLAUDE_CODE_SESSION_ID` does.

It cannot report through hooks, and amx cannot read its conversation back or
answer a trust screen for it. `hooks` is `None` because pi's extension events
are JS callbacks inside its own process, not command entries a settings file
can name: there is nothing for `install` to write and nothing for `hook` to
read, so `Hooks` is off. `Transcript` is off with it, and for that plumbing
rather than for want of a transcript — the only path that ever reaches a record
arrives on a hook payload, and pi sends none. pi does keep the conversation on
disk: a session jsonl at `~/.pi/agent/sessions/<encoded-cwd>/<ts>_<id>.jsonl`,
measured at 0.84.4. Finding one means replicating pi's own encoding of the
working directory, which is a pass nobody has made. `logs` is the verb that
puts that gap to the entry, and it names pi in the sentence it prints when a pi
pane has gone and nothing was recorded off it. `Trust` is off too: `--help`
shows `--approve, -a` trusting project-local files for a run, but nothing
measured yet says amx can answer that screen unattended the way it answers
claude's. Only `Hooks` is a shape pi lacks; `Transcript` and `Trust` are doors
amx has not built.

Reporting nothing costs pi none of the three it does claim. It resumes, forks
and is adopted with no hooks at all, because a session flag amx can hand it
directly stands in for the id a Started hook would otherwise have had to
report.

`screens` is where what amx can read off a pi pane is written down: three
rules — dialog, spinner, prompt — plus the chrome that comes off a capture
before anybody reads it, driven live against 0.84.4 and checked in as
`assets/screen-rules-pi.toml`. The entry declares it with `include_str!`, so
it is in the binary; `rules::of("pi")` finds it, parses it and hands it back,
and `rules.rs`'s own tests read pi's screens exactly that way.

No reader asks for them yet. `furniture`, `derive`, `send`, `doctor`, `status`
and the view all take `rules::bundled()`, which is `rules::of` on
`registry::entries().first()` — claude, by where the table lists it, and there
is a law holding that order. So a pi pane is cut and claimed with claude's
anchors today, the same as an unmeasured vendor's would be. The gap to close
is which door a reader goes through, not a measurement nobody made.

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
   vendor that reports names every moment exactly once, an adoptable vendor
   names its session variable, a session variable never travels, a session's
   own flags are flags too and none of them lists `resume` among what
   conflicts with it, a vendor that can fork says how — and one that forks
   through no hooks declares a start flag, since there is no report coming to
   name the copy's session — and declared screens parse. A new entry inherits
   every one.
4. **Prove the conformance.** `tests/mock_claude/` is a stand-in that replays
   scenarios — hook payloads, transcripts, screens — against the real tmux.
   A second vendor's harness takes the same shape: a fake that speaks the
   vendor's dialect, and the suite driven against it. pi has no such harness.
   What proves its entry is the table's laws, the unit tests in `pi.rs`, and
   the panes its screens were measured off, checked into `rules.rs` so the
   suite runs on a machine with no pi installed. Nothing yet drives amx
   end to end against something answering as pi.

A vendor can also land partially, and honestly. An entry with dials, a session
vocabulary and screens but no hooks still resumes, forks and is adopted, and
carries everything the floor already carries. `logs` is what asks the table
whether such a vendor keeps a conversation to read back, and it names the gap
rather than opening a path that was never going to be one. `result` asks the
table nothing: it reads whatever a hook already wrote to the record, or
failing that the transcript path a hook already named, and a vendor with
neither leaves both empty, so `result` says plainly that it captured no answer
instead of naming a capability. The capabilities list is what keeps a partial
entry truthful: nothing is promised that is not there.
