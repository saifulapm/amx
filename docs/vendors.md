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

Three things stay outside the table anyway, and not because a vendor's word for
them was hardcoded somewhere it should not have been: each is the wire format
of one particular capability's implementation. The JSON keys a hook payload
arrives with — `session_id`, `hook_event_name`, `tool_name`, and the rest
`hook.rs` reads — are claude's, written there because claude is the only
vendor in the table with hooks to read. The lines inside a transcript are
claude's too: `logs::conversation` and `hook::transcript_answer` both walk one
by `type`, `user`, `assistant`, `message.content` and `text`, a shape measured
from a live claude 2.1.240 transcript and not re-driven since, and `logs` reads
`tool_use` blocks besides. `Transcript` is a capability only claude claims.
`trust`'s store, `$CLAUDE_CONFIG_DIR/.claude.json`, is a literal in `trust.rs`
for the same reason, with a test tying that file to whichever vendors the table
says can answer the screen — `[claude]`, today. Each waits on a second vendor
claiming the capability that reads it, and pi claims none of the three.

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
dates. Nor was `[furniture] spinner`, which walks over the two punctuation
fragments the `spinner` rule was moved off, and which is the last finding that
document leaves open.

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
rather than for want of a transcript — the only path that ever reaches
`meta.transcript` arrives on a hook payload, and pi sends none. pi does keep
the conversation on disk: a session jsonl at
`~/.pi/agent/sessions/<encoded-cwd>/<ts>_<id>.jsonl`, measured at 0.84.4.
Finding one means replicating pi's own encoding of the working directory, which
is a pass nobody has made. `logs` is the verb that puts that gap to the entry,
and it names pi in the sentence it prints when a pi pane has gone and nothing
was recorded off it. `Trust` is off too: `--help` shows `--approve, -a`
trusting project-local files for a run, but nothing measured yet says amx can
answer that screen unattended the way it answers claude's. Only `Hooks` is a
shape pi lacks; `Transcript` and `Trust` are doors amx has not built.

Reporting nothing costs pi none of the three it does claim. It resumes, forks
and is adopted with no hooks at all, because a session flag amx can hand it
directly stands in for the id a Started hook would otherwise have had to
report.

What a turn answered does reach the record, and by the one route left. A
reading that ends a turn on a vendor with neither `Hooks` nor `Transcript`
writes what was on the pane to `state.result` with `screen` beside it as the
source, so `amx result` has an answer to hand back and a pi row on the wall
carries a summary. It is a reading of a picture and the record says so, which
is the difference between it and the two the entry does not claim: pi has not
told amx what it said, amx has looked.

`screens` is where what amx can read off a pi pane is written down: eight
rules — the first-run setup gate, the folder-trust question, a dialog, an
editor, an input, the spinner, the login box and the prompt — plus the chrome
that comes off a capture before anybody reads it, driven live against 0.84.4
and checked in as `assets/screen-rules-pi.toml`. The entry declares it with
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

## What the dogfood saw

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

**What it found.** Two are about which screen amx is looking at and are written
down in `docs/pi-screens.md`: pi's startup trust gate is not the screen
`project_trust` was measured off, and pi's update notice takes every windowed
rule off the pane. The other four are the hooks gap showing up in places the
capability list does not obviously cover, and none of them is answered here:

- **`send` always says the message may not have arrived.** It waits five
  seconds for the vendor's `UserPromptSubmit` and pi sends none, so every send
  to a pi agent exits failure with *did not start working within 5s; the
  message may not have reached it*. Measured four times, and the message had
  landed and the turn had run every time.
- **`result` never returns to a pi agent that was sent a message.** It waits
  for a turn that ended after the last `send`, and only a `Stop` event says one
  did. With no `--timeout` it waits forever; with one it exits on the deadline
  and says nothing. On an agent nobody has sent anything to it said it had
  captured none, and that half has been answered since: such an agent gets back
  what the last reading saw on the pane, the way this file describes below.
  After a message it still exits on the deadline, with that same answer on the
  record it will not serve.
- **An adopted pi whose first reading was `working` stays there.** Nothing ever
  writes a pi record's phase again, and the `prompt` rule is quiescent: from a
  record that says a turn is running it may not decide until the screen has
  held still for thirty consecutive looks, and `still_looks` counts within one
  process. `amx ls` looks once per process, so the row said `working` at an
  empty composer for as long as it was watched, while `amx result` — which
  polls in one process — cleared the same pane in six seconds. `amx status` on
  it named *the vendor's hooks* as the evidence, on a vendor that has none.
- **The first question amx reads off a pi pane is the question every later one
  shows.** A record learns a question and overwrites nothing, because a hook is
  the vendor's own word and a screen is amx's reading of a picture. On claude
  the next hook clears it. On pi nothing does: an agent driven through a dozen
  screens was still offering *Run echo hi?* as its question when it was stopped
  on the login box, which is the first `ctx.ui.select` it had ever been read on.

One more, from the rig rather than from pi. `adopt` takes the first vendor in
the table whose session variable is in the environment, so a pi started from a
terminal that already had `CLAUDE_CODE_SESSION_ID` in it was adopted as claude,
with claude's session id on the record and claude's document reading the pane —
`unknown`, and no rule. Unset that variable and the same pane adopted as pi.

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
   vendor's dialect, and the suite driven against it. pi's stand-in is
   `tests/mock_pi/pi`, a shell script reached through the PATH, since the
   table is keyed by the program a command runs and that is the whole of what
   makes an agent pi's on a machine with no pi installed; `tests/e2e_pi.rs` is
   the suite driven against it. It replays what pi's entry claims and nothing
   else: the session flags, down to the five pi refuses `--session-id` beside,
   and the screens `assets/screen-rules-pi.toml` was measured off, each
   painted in one write. No payload and no transcript, because pi reports
   neither — what the vendor was asked for is on its pane, and that is where
   the suite reads it. Under that sit the table's laws, the unit tests in
   `pi.rs`, and the panes the screens were measured off, checked into
   `rules.rs`.
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
   vendor's own words. What the capability list leaves off is not a finding: a
   vendor reporting through no hooks lags its pane by `FRESH` seconds and
   leaves `result` nothing to print, which is a partial entry being honest. A
   finding is amx promising what the program does not honour, or a pane read
   with another vendor's anchors. Write down what you saw beside the
   measurements it tests, with the version on it. An entry is measured on a
   date, and so is a dogfood.

A vendor can also land partially, and honestly. An entry with dials, a session
vocabulary and screens but no hooks still resumes, forks and is adopted, and
carries everything the floor already carries. `logs` is what asks the table
whether such a vendor keeps a conversation to read back, and it names the gap
rather than opening a path that was never going to be one. `result` asks the
table nothing: it reads whatever is on the record, which is what a hook wrote
there, or failing that the transcript path a hook named.

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

What none of that does is get `result` to the end of a turn somebody sent,
which the dogfood measured and `tests/e2e_pi.rs` holds: the turn it waits for
is the one after the last message, only a hook says a turn ended, and a vendor
that sends none leaves the wait sitting on its deadline with the answer on the
record beside it. The capabilities list is what keeps a partial entry truthful:
nothing is promised that is not there, and where an unclaimed capability costs
a verb more than an empty answer, the place to write that down is the pass
above.
