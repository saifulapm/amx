# amx

Run coding agents as tmux panes.

An agent started by amx is an ordinary program in an ordinary tmux pane. amx
starts it, watches it from the outside, and then gets out of the way. It is
never between the agent and your terminal, and nothing it starts stays
resident. Attaching to an agent is tmux attaching. Killing amx kills nothing.

That buys two things at once. A person gets a list of agents they can look in
on, answer and stop. A program gets four commands that say what happened in an
exit code: start one, ask after it, take its answer, end it.

```
$ amx new "port the importer"
port-the-importer-k3f
$ amx ls
working  port-the-importer-k3f     4s  Running Bash
waiting  fix-the-login-bug-a1b    12s  Claude needs your permission to use Bash
idle     tidy-the-imports-d4e      2m  the imports are sorted
```

## Requirements

- tmux 3.2 or newer. Earlier versions cannot address panes by id.
- A coding agent CLI. amx runs `claude` unless told otherwise; what it knows
  about a vendor is an entry in a table, and a command without one still gets
  a pane and a row — see [Vendors](#vendors).
- git, for the worktrees `new` cuts. Only needed if you use them.
- `gh` or `glab`, to put a pull request number on a row. Only that, and only
  for agents with a branch of their own.

## Installing

```sh
cargo install --path .
amx doctor --fix
```

`doctor` checks the eight things that have to be true before an agent can run:
tmux, the agent command, the config file, the hooks, one amx on the PATH and
the one running the check, a state directory amx can keep records in, no
handoff still carrying the spawner's environment from before that moved to a
file of its own, and no agent already stopped at a screen the vendor puts in
front of the work. Every check that fails says what to do about it.

Where a tmux server is already running, it checks a ninth: that the directory
that server is standing in still exists. A server keeps the directory it was
started in for as long as it lives, and once that directory is deleted every
pane it starts lands somewhere that is not there and dies immediately — which
from the outside looks like agents failing in under a second having said
nothing. Restarting the server is the fix, and the check prints the command.

`--fix` makes two repairs. It wires the configured agent's hooks, after asking
once: for claude, amx's seven hooks into `~/.claude/settings.json`, beside
whatever is already there, with the file backed up first; for pi, amx's own
extension into `~/.pi/agent/extensions/amx.ts`, where pi loads one from, and
a file of somebody else's standing there is copied aside. `amx uninstall` takes
both back out, puts the backed-up bytes back, and removes amx's records. It
refuses while any agent is still running: those records are the only place
their answers are kept. It also rewrites any handoff still carrying the
environment inline, which needs no asking — amx wrote every one of those files
itself. Upgrading amx wants `doctor --fix` again: the extension it ships for pi
changes with it, and `doctor` says so until the file is rewritten.

Without the hooks amx falls back to reading panes, which is enough to say what
an agent is doing but not enough to hand you what it said. Answers come from
the events. `amx doctor` says when they are missing.

`amx completion` writes a shell's completion script to stdout, for bash,
elvish, fish, powershell or zsh, off the verbs this build has. Where it lands
is your shell's business:

```sh
amx completion fish > ~/.config/fish/completions/amx.fish
amx completion zsh > ~/.zfunc/_amx   # with ~/.zfunc on the fpath compinit reads
```

fish reads that directory itself and needs nothing else. zsh reads the
directories on its `fpath`, so the file goes in one of those and `compinit`
runs after it.

## Starting an agent

```sh
amx new "fix the login bug"            # in a repository: its own worktree
amx new --no-worktree "run the tests"  # in this directory, as it is
amx new --name importer "port it"      # an id you chose
amx new --dir /srv/app "tail the log"  # somewhere other than here
amx rename importer auth               # what the wall calls it, afterwards
```

Every agent is one detached tmux session called `amx-<id>`, on the server you
are already in or on your default one. Nothing is tiled and nothing is
switched to: starting an agent from inside tmux leaves the window you were
looking at exactly where it was. The server is yours, so it reads the
`~/.tmux.conf` you wrote for it and amx brings no config of its own.

`new` prints the agent's id and nothing else. Everything after this takes that
id.

`--name` and `amx rename` name different things. `--name` is the id itself,
chosen rather than cut out of the task, and it is what the pane, the branch and
the worktree are named after. `amx rename` is only the word the name column
carries, so it can be said again whenever the work turns out to be about
something else, and it is 24 characters at most because a name a column cuts in
half is not a name. The id under it does not move, and `amx rename <id> <id>`
hands the row back to whatever named it without you.

Four dials say what is being launched and how it should behave:

```sh
amx new --agent claude --model opus --permission plan --effort high "port it"
```

Each falls back to the config, and the config falls back to the vendor: amx
passes no flag at all for a dial nobody turned. Which dials exist is the
vendor's answer, so a dial pi does not have, or a value claude would refuse, is
refused here instead, while the command is still on your screen.

A model names the harness that runs it, so `--model` on its own is enough:

```sh
amx new --model opus "port it"        # claude, which is what offers opus
amx new --model gpt-5-mini "port it"  # pi, whose listing holds it
```

Where you name no agent, amx asks each harness what models it offers, the one
your config names first, and starts the one whose list holds the word. A word
several offer stays with the configured harness. A word none of them offers is
refused while the command is still on your screen, naming what each takes.
`--agent` settles the question before it is asked: the harness you named is the
one that runs, and the model is a dial on it.

Anything after `--` is handed to the agent command untouched, and the task is
added after it, where a prompt goes:

```sh
amx new "port the importer" -- --model opus --session-id "$uuid"
```

`--model` before the separator turns amx's dial; the same word after it is the
vendor's own flag, and amx stands its dial down rather than send the flag
twice.

## A shell command as a row

Not everything worth keeping an eye on is an agent. `--exec` runs a plain
command in a pane, and it gets a row beside the rest:

```sh
amx new --exec 'cargo test --all'
amx new --exec 'ssh build01 make release && curl -fsS "$HOOK"'
```

In the view the same row is a line led with `!`: typing `!cargo test --all` on
the task line starts this and nothing else.

The whole command goes to `sh -c`, so a pipeline, an `&&` or a redirect is one
row and one exit code. It runs where you typed it rather than in a worktree of
its own: a command has no conversation to keep apart from the next one, and a
fresh checkout is not where `cargo test` was meant to run. There is no vendor
either, so the four dials and anything after `--` are refused beside `--exec`
rather than quietly dropped.

The row ends `done` or `failed` by what the command exited with, and the code
itself is `exit` in the JSON. `amx result` has nothing to hand back — a command
answers nothing, it exits — but what it printed is kept. The pane is piped into
`output` beside the record before the command starts, so the first line is in
the file as well as the last, and neither goes when the pane does. `amx logs`
reads the pane while the command is in it and that file afterwards, whole.
Space on the row reads the last quarter megabyte of the same file: the end of
it while the command runs, the top of that once the command has ended, in the
colours it was printed in. A quarter megabyte is more rows than a card is ever
paged through, and the card is taken again every second it is open — so a
build that has printed for an hour costs the view what a short command does.

Like every pane amx starts, it is told where its own scratch directory is in
`$AMX_AGENT_DIR`. While it runs there are no hook events to hear from and no
rules to hold against the screen, because the screen belongs to somebody else's
program. What there is is the pane, so the row reads `working` for as long as
the command is in it, and the line beside it is the last one the command
printed.

## The view

Typing `amx` on its own opens the list of agents on the terminal you typed it
at, inside tmux or outside one. It builds nothing to draw in: the list is a
program on a screen, and a screen is all it needs. Down a pipe it prints the
table and exits, so `amx | grep waiting` is a reasonable thing to write.

`amx --dir /srv/app` opens the same list about that directory alone, drawn on
the terminal or printed down the pipe by the same rule. It is the reading
`amx ls --dir` takes, described under
[Looking, and answering](#looking-and-answering).

The screen is four things: two rows above the list, the list, the line you are
typing when you are typing one, and a row of keys at the foot.

```
AMX          1 pinned   1 review   1 working   27 done   3 running    1 WAITING
└ next  claude   model  default   permission  default   worktree  new

 Pinned
  ✻ bump-deps-e5f          Running Bash                                      12s

 Ready for review
  ∙ fix-login-a1b     #12  the login bug is fixed                             4m

 Needs input
  ✻ port-import-b2c        Which fixture should the port keep?               29s

 Working
  ✻ ship-docs-a7d          Running cargo test                                 3s

 Completed 27

space card   enter attach   ctrl+x stop   ctrl+t pin   ctrl+s axis   ? keys
```

The first row is what there is: `AMX`, the directory the view was opened on
where one was named, a count per group, how many are running — beside the cap
they are counted against, where the view is about a fleet that has one — and at
the far end the number the view is opened to read: how many agents are waiting
on you, in reverse video, or `nothing waiting` in the same place when none are.
The second row hangs off it on a `└` and holds the dials, which are about the
agent that does not exist yet, so nothing on it can be read as a fact about the
fleet. On a terminal under ten rows the second row goes and the first stays.
The row at the foot is whatever keys the line under the cursor makes true, cut
to what the terminal holds, with `?` pinned to the end of it.

The list answers one question, so it is gathered under the answer: whatever you
pinned there yourself comes first, then the work standing in front of a
reviewer, then the agents stopped on a question, then the ones mid-turn, then
the turns that are over. Pinned is `ctrl+t` and nothing else. Ready for review
is a turn that is over whose branch still has a pull request asking somebody
for something, which is work that has left the machine and is waiting on a
reader rather than on amx. Completed takes every ending alike — an agent
sitting at its prompt, one whose command exited, one amx has lost track of —
because a turn at a prompt has ended as surely as a turn that exited, and
reading down two headings to find that out is one heading too many.

A heading is a line as quiet as the rows under it: a blank row, then the
group's name at the left margin, capitalised the way a sentence is rather than
shouted in caps, and nothing drawn across the rest of the line. There is no
count while the group is open, because the rows it would count are under it to
be counted. Shutting the heading — `enter` on it — puts those rows away and the
count where they were: `Completed 27`. A group holding a failure says so either
way, after the label and after the count where there is one, because a failed
agent is why somebody came to the screen: `Completed 27 · 2 failed` shut,
`Completed · 2 failed` open. Two things up here take a colour, and each of them
wants a person: those failures, and the words `Needs input`. The cursor stops
on a heading like any other line.

A row is one line, always, on columns the screen fixes rather than the fleet:
a cell of indent, the state glyph, the name, what the agent last said, and
the seconds at the end. They stand where they stood when the last agent ended,
so the row you learned wide is the row you get narrow — under 100 columns the
name column is the one that gives way. A wall with nothing on it keeps
everything above the list and offers, where a name would be, the only two keys
that lead anywhere from there: `n` and `?`.

The name in that column is the row's own word for the agent, and on a claude
agent it is the title claude gave the session. The vendor names the
conversation out of the work in it and writes a new one as that work moves, so
the column says what the agents are doing rather than repeating a column of
ids. `ctrl+r` puts a word of yours there instead, and from then on yours is the
one that stands. The id goes nowhere: the `ls` table prints it, every verb
takes it, and `/` finds a row by it as readily as by the title.

The glyph is two shapes, and the shape answers what a colour cannot: whether
there is still an agent there. `✻` is a process you can attach to, answer or
stop; `∙` is the dot it leaves behind, which is a record to read. While a turn
is running the `✻` breathes — the vendor's own mark, growing and shrinking a
frame at a time — so the rows in motion are the rows mid-turn, and a row that
stops settles onto the shape it was already breathing through. The dot stands
for one thing more: an agent amx let go. One left idle for `park_after` with
nobody attached loses its pane and keeps its record, so the row turns from `✻`
to the dot in the colour it was already wearing, and `enter` brings it back.

The colour on it says which state that process is in: amber for an agent
stopped on a question, green for one that finished, red for one that failed,
grey for one you stopped by hand, dim for one sitting idle at its prompt, and
your terminal's own colour while it is coming up or working, or when amx cannot
account for it. The name takes that amber or that red as well, so a row that is
asking and a row that failed can be found down a column of names without the
glyph being read; every other name is your terminal's own, dimmed like the
heading over it. What those colours are is the theme's to say, under
[Themes](#themes).

The wall says nothing at all with weight. Every name on it is as quiet as the
summary beside it and the heading over it, and what marks the row you are
working with is strength instead: the name under the cursor comes up out of
that dim, and so does the name under the pointer, each in whatever colour it
already had. A screenful of names half of which are shouting is a screenful
nobody reads down.

The name of the agent you last went into is in the accent until you go into
another. Coming back out of a session lands you on a screenful of rows that
look alike, and the one you were just inside is the one you are about to look
for. A name a state has already coloured keeps that colour, because what an
agent wants of you is worth more than where you have been, and the mark is the
view's rather than the record's: it says where you have been at this screen,
and it goes when the screen does.

The seconds at the end of a row are the time the agent has worked: ticking
while it works, standing still while it waits or sits idle — an agent left at
a question all afternoon has not worked an afternoon — and stopped for good
when the run ends. The `amx ls` table prints the same number. How long a
question has been standing is on the card, in its title.

`ctrl+s` gathers the same agents under the directory each one runs in, and the
screen changes twice for it. A heading is a path rather than a word, and it is
drawn the way a group's heading is: dim end to end, no weight on the last
segment, no count while the rows are on the screen, and a path too long for the
line loses its middle rather than its end — the end is the segment that says
which worktree of a project this is. And every row grows a state word between
its name and what the agent said, because the heading over it no longer says
what state the row is in:

```
 ~/code/amx
  ✻ port-import-b2c   waiting   Which fixture should the port keep?          29s
  ∙ tidy-imports-d4e  done      did what it was asked                         2m

 /srv/app
  ✻ fix-login-a1b     idle      the login bug is fixed                        4m
```

Eight cells, which is what `starting` needs and what the shorter words are
padded out to: a state word cut down would be a lie. The summary column pays
for all of it, so the name and the seconds sit exactly where they sat on the
other axis — switching the axis moves one boundary rather than the whole
table.

| Key | What it does |
| --- | ------------ |
| `↑` `↓` `j` `k` | walk the agents |
| `gg` `G` | the top of the list, and the foot |
| `space` `l` | the card, and the line on it for an answer or a message |
| `enter` `→` | put its session in front of you, or shut the group under the cursor |
| `esc` | put the card away, or leave a line alone |
| `n` | start an agent, and the cursor goes to its row |
| `alt+n` | start the line and go to the agent it started |
| `!` | leading the task line, run it as a command rather than give it to an agent |
| `d` | what it has changed |
| `pgup` `ctrl+b` | page the card, when it holds more |
| `pgdn` `ctrl+f` | and the other way |
| `ctrl+u` | half a page of it, toward the edge |
| `ctrl+d` | and half a page away |
| `ctrl+x` | stop it, twice to forget it, and twice on a heading to stop and clear the group |
| `ctrl+r` | call it something else, as `amx rename` does from a shell |
| `ctrl+g` | write the line in `$EDITOR` |
| `alt+↑` `alt+↓` | the lines sent before, newest first; a task line takes `↑` `↓` for the same |
| `alt+1..9` | reach the agent at that place on the wall |
| `/` | find by name, task or `#12`, as you type; `esc` clears it |
| `ctrl+s` | gather them by state or by project |
| `ctrl+t` | pin it over the wall, and again to let it go |
| `shift+↑` `shift+↓` | move it up or down its group |
| `shift+enter` | a newline in the line, without sending it |
| `alt+enter` | the same newline, where `shift+enter` does not arrive |
| `ctrl+j` | the same again, where neither of those arrives |
| `tab` | the word offered under the line, or on nothing the agents |
| `←` `→` `ctrl+←` `ctrl+→` | the cursor along the line, by a character or by a word |
| `home` `end` `ctrl+a` `ctrl+e` | the front of the line, and the end of it |
| `backspace` `delete` | the character behind the cursor, or the one under it |
| `ctrl+w` `alt+backspace` | the word behind the cursor, in one press |
| `alt+a` | which vendor the next agent runs |
| `alt+m` | which model the next agent is given |
| `alt+w` | whether it gets a worktree of its own |
| `shift+tab` | what the next agent may do without asking |
| `?` | every key, where the list was |
| `q` `ctrl+c` | close the view |

Inside tmux, `enter` moves your client to the agent's session and leaves the
view drawing in the session it was already in, so switching back lands on the
list where you left it. Outside tmux the view is the only thing on the
terminal, so it lends the terminal to a tmux client instead and takes it back
when you detach. Either way `ctrl+z` inside the session is the way back: it
moves your client to the session it came from, and where it came from nowhere
it detaches, which puts the view back on its terminal, or `amx attach` back at
the shell it was typed at. It is a key amx binds on that tmux server whenever
it hands a terminal over, and it is read only in a session amx named — in any
other session on the server `ctrl+z` is still whatever it was. Nothing was lost
under it: claude uses the key to suspend itself to a shell, and an agent's pane
has no shell under it.

`alt+1..9` does the same for the first nine agents by where they are on the
wall, counting rows from the top and skipping the headings, without walking the
cursor to them first. It is the fleet you already have in front of you, reached
by the number you were about to count to.

`space` opens the card, and the card is not a box. It stands at the foot of the
list, where the line you type stands, and is drawn the way that band is: a rule
across the screen with the agent's own name at the front of it, in the colour
its row says its state in, and everything the card says under that, two cells
in beneath the chevron. On a card holding a patch the rule says that too, after
the name; on one you have paged, it says how far from its edge it stands at the
far end. Nothing above it moves. A card hung under its own row moves every row
below it down, so walking the cursor with one open shakes the wall you are
reading it against — at the foot, the list stands still and the card changes
under it. Everything above it goes dim for as long as it is up, exactly as it
does under the task line, because the card is the same kind of thing: a modal
whose letters are its line's until `esc`, and the dim is what says so. It
stands over the list rather than taking rows off it: the wall is laid out as it
is with no card up, fold and all, and the card covers the foot of it, so
opening one folds nothing — the list scrolls only as far as keeping the
cursor's row above the card needs. It takes
fourteen rows at most, never more than half the screen, and
always leaves a row of the list it was opened from. It opens straight onto what
it has to show: what the agent is doing is on its own row up in the list, and a
card that repeated it would spend a row on what you were already looking at.

What the card's body is follows the agent. Where its vendor keeps the
conversation in a file — claude and pi both do — the card is that whole
conversation, drawn rather than pictured: every prompt behind the composer's
own `❯`, every answer with its markdown rendered, every tool call as one row
naming the tool with the command or path it was given dim beside it, and a run
of calls as one block. Nothing the vendor draws under its pane is in it. One
whose turn is over — idle at its prompt, done, failed or stopped alike — opens
on the end of its last answer, where the conclusion of it is, with the rest of
that answer and every earlier turn a page up. One that is working ends on a
live tail, standing off the record above it by a blank row: the last few rows
of what pi is saying at this moment, streamed by the extension `doctor --fix`
installs. claude streams nothing, so its card is the record alone until the
next message lands — its tool calls as they are issued, its answers as each
message ends — and the row over the card says what it is doing in between. The
pane is never drawn under a record: it is the same turn in the vendor's own
dress, boxes and banners and spinner lines around words the record already
has. The tail is kept short enough that a row of the record stays above it,
and where the vendor has streamed nothing yet — the seconds between a turn
starting and its first word landing — the card is the record alone. A command
row is neither: its card is the last quarter megabyte of the file its pane is
piped into, which is more rows than a card is paged through and the same cost
however long the command has printed — the end of that while the command runs
and the top of it once the command has ended, with nothing cut off the bottom,
since the anchors that cut are a vendor's own and every row a command prints is
its own work. An agent with no conversation to read — one adopted before its
first report has named one — keeps the older card: its live screen, chrome cut,
while it works, and the recorded answer once its turn is over. A waiting
agent's card is the question block alone.

Every card ends with a line, whatever the agent is doing, and the line opens
with the card, so one key puts both the look and the way to answer it in front
of you. While nothing is typed on it the line says what it will take. At a
question that is what the question will take. On an agent still at work it reads
`reply`, because whatever you write there goes to it as it stands. On one whose
command has ended it reads `nothing is listening`, which is the refusal a reply
would come back with — a line that invited one there would be the card telling
you to type into the dark. The `❯` carries the waiting colour at a question and
is dim everywhere else: a prompt waiting on you should not look like one you may
type at if you feel like it. A blank row stands between what the card says and
the line, and goes first where the band is short.

The line is the task line in everything that is not about starting an agent.
It grows a row at a time as you write, up to the same cap, taking rows off
what the card says and never its last one; `shift+enter` breaks a line in it
and `ctrl+g` opens it in `$EDITOR`. And it offers the same words under itself,
in the same band: `/` for the vendor's skills and commands, `@` for its agents
and the project's files, `tab` and `↑` `↓` walking them exactly as under the
task line. They are read against the agent on the card — its own vendor and the
directory it runs in — rather than the header's dials, because a word offered
out of anywhere else is a word that agent would not find. The dials themselves
are the task line's alone: `m:`, `p:`, `w:`, `d:` and `agent:` typed here are
words of the message, since the agent is already running under whatever it was
started with.

At a question of the vendor's own whose choices are all it takes, the line reads
`1-2 picks` and the number pressed is the answer: it reaches the agent as it is
pressed, with no enter behind it, which is what the vendor's own screen does
with a number there. Three prompts read `press` instead and wait for the enter.
A question that takes more than one choice, where a digit names one box and the
rest are still to come. One whose choices carry a preview, where the note is
typed after the key it rides beside. And a permission box, whose numbers are
amx's reading of a picture of a pane rather than anything the vendor wrote down
— an allowed tool call cannot be taken back. What picking costs is the
answer of your own that opens with a digit: the numbers are read on an empty
line only, so a number meant as a character is typed after some other one.

With a card up, one rule decides every key: a key the line has a use for is the
line's, and every other key is the list's, as if the line were not there. So
every letter is text — `j`, `k`, `h`, `l`, `q`, `n`, `d`, `/` and `?` included
— and so are the keys that move along a line and end it. The arrows keep
walking the wall with the card in tow, the page keys and the chords under them
page the card, and `ctrl+x`, `ctrl+t`, `ctrl+s`, `alt+1..9` and the shifted
arrows do to the list exactly what they do with no card open. So does the
mouse, which has no use for a line either.

Two keys are read on the empty line before that rule, because there they have
nothing to do to the line: `space` closes the card, which is the key that
opened it, and `enter` is the list's own — an attach on a row, a group shut or
opened on a heading, the fold's rows given back on the fold. The first
character typed takes both back, and after it `space` is a space and `enter`
sends what you have written. `esc` closes the line and the card together
whatever is on it. The row under the card says which of the two it is reading:
`enter attach`, `space closes it` and `pgup pages it` where the body holds
more, or `enter sends it` — `answers it` at a question — with `alt+enter
newline` behind it, and `esc closes it` pinned to the end of both.

`alt+↑` brings back the last line you sent, and again the one before it, back
to the oldest; `alt+↓` walks forward again, and the step past the newest gives
back whatever you were typing when you started. A task line takes `↑` and `↓`
for the same walk, since nothing else has a use for them there; on the card's
line the plain arrows keep moving the card. Tasks and replies are kept apart,
fifty of each, in `view.json` beside the wall's arrangement, so the line you
want back is there in the next view too. While words are offered under the
cursor the plain arrows walk those first, and a line brought back is not
looked up for words until you type the next character.

`pgup` and `pgdn` page inside the card's body when it holds more than the card
has room for: a patch or a finished conversation down from where it opened, a
live one and a live screen up from their bottom. A paged card says how far from
that edge it stands at the far end of its rule — `↓ 12 more` — and holds still,
new output and all, until you page back to where it opened, press an arrow, or
open it again; an agent stopping at a question takes
the card back regardless, because a question is never left behind history. The
arrows never page: they keep walking the list, card in tow. `ctrl+b` and
`ctrl+f` are the same two pages for a keyboard with no page keys on it — pgup
and pgdn exactly. A lone `ctrl+b` under tmux's default prefix is tmux's own
business: `ctrl+b ctrl+b` is what reaches the view there.

`?` puts every key where the list was, in the five groups they are learned in —
walk, look, start, arrange, dials — each under a heading of its own: the label,
a dim rule, and how many keys stand under it at the far end. From 100 columns
the groups stand in two ruled columns, cut where the two halves come nearest to
holding the same number of keys, and nothing is shortened to fit. Narrower than
that the second column is given up whole rather than squeezed: one column,
every key still saying what it does in full, paged with `pgup` and `pgdn`, and
the foot of each page says which page it is and which of those two keys turns
it — a key nobody can reach is a key the screen may as well not list. Any other
key goes back to the list, and `q` closes the view.

`ctrl+x` forgets nothing on the first press, and the first press is the same
press on every row. An agent that is still running — sitting idle at its
prompt included — is stopped, which costs you the pane and nothing else, and
the row is armed in the same move; one whose command has ended is armed as it
stands. Where the row was saying what the agent did it says `ctrl+x again
forgets` for about two seconds, and the press inside that window is the one
that takes the record away, and the worktree with it where that tree holds
nothing no commit has. Leave it alone and the row goes back to saying what it
was saying, having forgotten nothing. On a heading the two presses reach the
whole group, whatever the states beneath, and the first of them costs nothing
at all: every row under the heading is armed in place — each row saying
`ctrl+x again stops and forgets` where its summary was, because the press
after this one reaches the whole group — and not one agent is stopped. The
press inside the window is the one that acts: every agent under the heading
still running is stopped, and then they are all forgotten, each under the same
worktree safety a single row gets. A row's first press costs you one pane,
which is a thing you can watch happen; the same press on a heading could cost
you every pane on the screen, so the group waits for the press you have been
warned about. The heading that press lands on is not always the one that was
pressed, because a group dissolves as its rows change state: the rows carry
the arming, the cursor is kept with them while the window is open, and the
heading standing over them is where the second press lands.

The list takes the mouse: a click on an agent's row is `enter` on it — the
cursor lands and the agent's session comes in front of you — a click on a
heading opens and shuts it, a click on the `… N more` fold unfolds it, the
wheel scrolls — and pages an open card when the pointer is over it — and
resting the pointer on a row tints its name. Hold shift for the terminal's own
text selection, which is how every mouse-capture program behaves under tmux or
ghostty. The mouse is handed back with the screen whenever the view lends the
terminal out, and when it closes.

While the view is open your terminal is called `amx · 2 waiting`, or `amx` when
nothing is waiting on you, so a window behind something else still answers the
one question worth pulling it forward for. The title your terminal came with is
put back when the view closes.

Every key answers to the one chord it is written under, so the `alt+q` of
somebody arranging their windows closes nothing. The four dials are about the
agent that does not exist yet: they change what the next `n` starts and nothing
about what is already running, and the header says where they point.

The order the list puts agents in is amx's until you say otherwise. `ctrl+t`
pins the one under the cursor over the wall: it leaves whatever group amx had
it in, stands under `PINNED` at the top of the list, and stays there whether it
is asking, working or finished, so the agent you are watching is where you are
looking. The same key lets it go, and it drops back into the group its state
puts it in. `shift+↑` and `shift+↓` move an agent a row at a time past the
others in its group; an agent that starts after you have put a group in order
joins the bottom of it, because a group you arranged by hand is not one amx
goes on sorting under you. Both of those and whichever way `ctrl+s` last
gathered the fleet are written to `~/.local/state/amx/view.json` as you go, so
the next view opens on the wall you left.

A line being typed in a band of its own hangs off a rule, and the rule is where
the whole mode is said. Its near end names which line this is — a task, or a
rename — with the agent it is aimed at beside it where it is aimed at one, and
after that the one thing true of both: while the line is open a letter is a
letter and not the key it is bound to, and `esc` is the way out. Its far end
carries what the next agent may do without asking, in reverse video, set into
the edge: it is the one dial somebody is about to press enter past. The dashes
are `┈`, the lightest there is, because a terminal inks a box-drawing glyph
across the whole cell and a solid rule would read brighter than the dim words
beside it — half the cells left blank is what puts the two level.

The line itself begins with a `❯` in the column the rule's own label starts in,
whichever of the two it is holding, so moving between them does not move the
words you are reading. It is drawn at the weight you type anything at, because
what says where you are is the block. That block is a cell turned over rather
than a glyph put in one: the character you are standing on keeps its place and
is read through the block, and past the end of the line the cell turned over is
a space, which is a whole block. It is the only cursor there is. The
terminal's own is hidden for as long as the view has the screen, so nothing of
the terminal's blinks over the cell amx is painting, and the find line and the
line at the foot of a card draw the same block the same way. It comes back when
the view hands the screen over: the editor `ctrl+g` opens and the session
`enter` attaches to are each given a terminal with a cursor in it, and the
first frame the view draws on taking the screen back puts it away again.
Everything above the rule goes dim for as long as the mode is on — rows,
headings, counts and dials in the one pass — so the band below the rule is the
only thing on the screen left undimmed.

```
TASK · letters are text until esc ┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈ vendor default ┈┈
❯ add a retry to the webhook█
enter starts it   alt+enter newline   shift+tab permission   esc cancels
```

The keys at the foot are pairs: the key carrying the weight and what it does
dim behind it, with wall between one and the next. The weight changing is the
edge, so there is no character spent being one. They shed from the far end as
the terminal narrows — the eighty columns above have already dropped
`ctrl+g $EDITOR` — and the way out of the mode is pinned to the end and never
goes.

The line a task is typed on reads a few words of its own, at the front of it
and nowhere else. `m:`, `p:`, `w:`, `d:` and `agent:` turn the dials for the
one agent that line starts, as in `m:opus w:off port the importer`. `d:` is
where that one runs: `d:/srv/app port it`, `d:~/code/importer port it`, or a
name read against the directory you opened the view in, the way a shell prompt
standing there would read it. Nothing else on it is a word amx reads, so
`s:waiting` typed here is the task and `enter` starts an agent on it.

Without a `d:` the agent starts where you opened the view, unless the wall says
somewhere else: with the agents gathered by project, a line opened on a heading
or on any row under it starts in that project, and the rule says which.

```
TASK · in ~/code/importer · letters are text until esc ┈┈┈┈┈┈┈ vendor default ┈┈
❯ port the last of the callers█
enter starts it   alt+enter newline   shift+tab permission   esc cancels
```

The cursor is where you are looking; a `d:` is you saying where, so it wins.
Which project a row belongs to is the heading's answer and not the row's own
directory, so a line opened on an agent running in a worktree or a
subdirectory starts at the top of the repository the heading names.

A line led with `!` runs rather than asks. `!cargo test --all` is the row
`amx new --exec` starts at a prompt: the rest of the line is the command, it
goes to `sh -c` whole, and the rule over the line reads `COMMAND` for as long
as the bang stands. The rule's dashes and the `❯` under them take the accent
while it stands as well, so the band says what enter will do before a word of
it is read, and go back to dim the keystroke the bang comes off. It runs where
a task typed in its place would have: the view's directory, the project the
line was opened under, or the one a `d:` beside the bang names. It is given no
worktree and no dials — there is no vendor on that row for a dial to be about,
so `!m:opus cargo test` is refused by name and the line comes back with what
you typed still on it. The front of the line and nowhere else: a bang further
along is a character of the task.

A word on that line opening with `/` or `@` is the vendor's own, and a band
under the line says what it could be: the skills and commands it runs by name,
the agents it can be told to be, read out of its own directories as you type
and narrowed to what you have typed of the word. `agent:` is offered the same
way, out of the vendors amx has an entry for. Six of them at most, and the
rows come off the wall the way the line's own do.

The dials are offered too: `m:` and `p:` the values that vendor's dial takes,
`w:` its two, and `d:` the directories under the one the line will run in
along with every project you already have an agent in. An `@` word that names
none of the vendor's agents is a path — the other thing the mark is for — and
what it offers is what is in the directory it names, read against the line's
own `d:` where it has one, under the project whose heading the line was opened
under where the wall is showing projects, and under the view's directory
otherwise, with `~` read as your home. Directories carry a `/` and keep the
space off the end of the word, so `@src/` is a path you go on typing; `.git`
and `.amx` are left out.

```
TASK · letters are text until esc ┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈ vendor default ┈┈
❯ /rev█
  /review  Read the diff.
  /revise  Say it again.
enter starts it   alt+enter newline   shift+tab permission   esc cancels
```

`↑` and `↓` walk the band, `tab` writes the word the choice is standing on in
the place of the one you are typing, and `enter` does the same while the word
is still short of it, because a line with a band open under it is a line you
are still writing a word of. A word already spelled the way the choice spells
it is finished, and `enter` on it starts the agent as it would on any line:
`/review` typed out goes on the first `enter`, not the second. `esc` puts the
band away and leaves the line where it was, so the way out of a suggestion is
not the way out of the mode.

On a task line with nothing on it there is no word to take, so `tab` writes the
`@` itself and the band opens on what answers to it: the vendor's own agents,
or the project's files where it has none.

A line led with one of the vendor's own agents is a task for that agent:
`@scout port the importer` starts the session as `scout` — `--agent scout` on
claude's argv — and the task it is given is the rest of the line. The front of
the line and nowhere else, and only a name the vendor loads: `@scout` further
along, or a word naming none of its agents, stays where you typed it and is
read as the file or the sentence it is. pi has no agents of its own and no flag
to be one with, so on a line for it every `@` is left alone.

`enter` starts the agent and leaves you at the wall, with the cursor on its
row. The row is not there until the reading after the start, so the cursor
lands on it the moment the wall draws it, and the card follows as it does on
any other move. An agent the wall is narrowed away from is not waited for: the
cursor stays where it is.

`alt+n` enters that line and takes you with it: the agent is started and your
terminal lands in its session, by the same two roads `enter` on a row takes.
Everything else about it is what `enter` on the line would have done, the
question a task of three letters is asked included, so the answer to that
question is what carries you there.

`ctrl+g` writes that line in your own editor. It opens `$VISUAL`, or `$EDITOR`,
or `vi`, on a file holding whatever is on the line with any folded paste
written out in full, hands it the terminal, and puts back what you wrote when
you close it — pressed on the list it opens a task line first, so a task worth
a paragraph is one keystroke away. An editor that exits unhappily, which is
`:cq` in vim, leaves the line exactly as it was.

A task of fewer than four characters is asked about once before anything is
started: `y` starts it and any other key hands the line back with what you
typed still on it. `n` opens the line and the letter after a stray `n` is a
task nobody meant, where `wip` is one somebody does.

Narrowing the wall is `/`, and its line is the only one that does it. It
narrows as you type, on a name, a piece of a task, or `#12` for the pull
request the branch carries — whole and untokenised, so `port the` looks for an
agent called that rather than for two things. A line of nothing but `s:` tokens
narrows by state instead: `s:waiting`, or `s:waiting s:working` for either of
them, and a token with nothing after it drops that narrowing. The word is one
of the five the counters say, which keeps that group, or a state the record
knows and no counter names: `s:failed` picks the one that died out of
everything under completed, and `s:idle` the ones sitting at a prompt.
`enter` closes the line and leaves the wall as it is; `esc` gives the fleet
back, with or without a line open.

The keys that edit any of these lines are a shell prompt's. `←` and `→` walk a
character and `ctrl+←` and `ctrl+→` a word; `home` and `end`, or `ctrl+a` and
`ctrl+e`, go to the ends. `backspace` takes the character behind the cursor and
`delete` the one under it, and `ctrl+w` or `alt+backspace` takes the whole word
behind it. `shift+enter` puts a newline in the line without sending it, on a
terminal that can tell that chord from a plain `enter` — amx asks for the kitty
keyboard protocol's disambiguation while it holds the screen, and a terminal
that does not speak it sends `enter` and sends the line. `alt+enter` does the
same newline everywhere, and `ctrl+j` where your terminal will not send
`alt+enter` either.

A paste is one edit, every newline in it included, and it lands where the
cursor is standing — pasted at the list it opens a task line rather than being
read as the keys it is made of. A long one folds: over 800 characters, or more
than three rows, and it stands on the line as `[Pasted text #1]`, numbered from
one on each line you open. That is what you read your sentence around, not what
is sent — the task, the message and the editor `ctrl+g` opens all get every
character you pasted. `backspace`, `delete` or `ctrl+w` against a marker takes
the marker whole, and the paste with it, and pasting the same text again while
its marker still stands unfolds it: the text goes back where the marker was, so
you can read what you pasted rather than take the row on trust. A shorter paste
is left on the line as it is, and a name or a find line takes whatever you paste
at it whole, since neither is a line you write a paragraph on.

## Pull requests

An agent's work goes onto a branch, and what becomes of it after that is the
one thing the rest of a row cannot say. Where the branch has a pull request,
the row carries its number:

```
  ∙ fix-login-a1b     #12  the login bug is fixed                             4m
  ✻ port-import-b2c   #40  Running Bash                                       4s
```

The number is coloured by how it is going, in the same colours the rest of the
view uses. Merged and approved are green, a failing check is red, changes
requested is yellow, a request closed without going in is grey, and a draft is
dimmed. A request whose checks are still running and one nobody has read yet
both take your terminal's own colour, because neither has an answer to that
question yet. The card says which of the two it is in words, and lists every
request the branch has rather than the one the row had room for.

`/#12` narrows the list to it. That is the word you have in front of you when
you arrive at the wall from the request rather than from the agent.

Reading it is `gh`, and `glab` where gh has nothing to say. Neither is required
and amx installs neither: without them the column is not drawn and nothing else
changes. What the forge said is written down beside the agent's record and
taken at its word for a minute — or for good, once everything on the branch has
been merged or closed and nothing more can happen to it. Asking again happens
in the background, because a list redrawn every second cannot stop for a
network.

## Looking, and answering

```sh
amx ls                 # every agent, one line each
amx ls --json          # the same reading, for a program
amx ls --dir /srv/app  # only the agents working under that directory
amx ls --dir .         # only this project's
amx --dir /srv/app     # the same narrowing at the front door, drawn or printed
amx status <id>        # one agent, and which signal that state came from
amx status <id> --json
amx rename <id> auth   # call it something else on the wall; the id stays
amx attach <id>        # hand this terminal to its pane
amx logs <id>          # the last of what its pane has printed, without attaching
amx logs <id> --lines 40
amx send <id> "and now the linter"
amx answer <id> y      # the keys a prompt reads: y, n, 1-9, enter, esc
amx answer <id> 1,3    # a question that takes several: check these two
amx answer <id> --text "keep the old importer"   # the row it offers for words
amx answer <id> 1 --note "and keep the subtitle" # a note beside the choice
amx interrupt <id>     # stop the turn it is in the middle of
amx diff <id>          # its worktree against the commit it was cut from
amx diff <id> --stat   # the shape of it: a file per line, and the totals
amx events --follow    # every agent's log, merged
amx events <id> --json # one object per event, payloads whole
```

`--dir` is one machine read one project at a time. An agent belongs to a
directory when it runs under it, and an agent in a worktree belongs to the
repository the tree was cut from: the tree is `<repo>/.amx/worktrees/<id>`, and
what you mean by the project is the repository. A directory reached through a
link is the directory it leads to, so `amx ls --dir .` answers the same from
either name. Nothing is written down and nothing is hidden anywhere else — it
is one reading of one question, and an agent is in two of them when the
directories nest.

`send` refuses while an agent is waiting on a question. Text typed at a
permission prompt answers the prompt, and that is not something you can take
back. It hands you the question instead. `answer` is for those, and refuses
when nothing is pending.

What a question will take depends on what kind it is. A permission prompt and
the folder-trust screen read one key. A question the vendor asked itself offers
choices and a field, so it also takes words of your own:
`amx answer <id> "keep the old importer"`. Anything outside that grammar is
refused before a byte reaches the pane.

Some of the vendor's own questions take more than one choice, and nothing on
the screen says which those are — `amx status <id> --json` says so under
`.multi`. Name the choices and amx checks each one and submits:
`amx answer <id> 1,3`. At a question that takes a single choice the same
command line is refused rather than half taken, because a `1` there is chosen
and submitted the moment it is typed and the `3` after it would land on
whatever the agent drew next.

The numbers are the question's own choices and they stop there. Every menu the
vendor draws carries two rows under them that no question asked for — one for
words of your own, and `Chat about this` — so a question of three choices is
five numbered rows on the screen, and `.options` in the JSON is the three. A
number past them is refused and told which row it is, because pressing the
first parks the cursor in a text field and answers nothing, and pressing it at a
question that takes several checks that empty field, which submits as an empty
string.

`--text` is for the answer that reads as something else, and it is how that row
is filled. It takes every key as a character, so `amx answer <id> --text 2`
answers with the character `2` where a bare `2` would be the second choice.
Words are still words without it, and the flag is refused at a prompt that
offers no such row.

`--note` rides beside a choice, at the questions that draw a field for one.
The vendor draws that field where its choices carry a preview, and there it
has no row for words instead, so the two flags are never both an answer to the
same question. A note without a choice is refused as well: submitting from
inside that field answers with no choice at all.

`logs` is what the agent has been up to, without taking the terminal for it:
the last hundred lines of it, or however many `--lines` asks for. Where the
vendor keeps a conversation, that is what it reads — every prompt, answer and
tool call of the recent history, whole, where a pane could only ever hold one
screen of it. An agent with no conversation to read — a command row, or an
adopted agent before its first report has named one — gets the pane's picture
instead, with the vendor's own composer, statusline and mode footer cut off the
bottom the way the card cuts them. Once the pane is gone the record is what is left, and `logs` prints the
answer amx captured from it, so the same command line says something about an
agent whether or not it is still running. `amx result` is still the one that
hands back a turn's answer alone, and blocks for it.

`amx statusline` prints the two numbers a status line has room for, and nothing
at all when no agent needs saying:

```tmux
set -g status-right '#(amx statusline)'
```

## A copy of a conversation

An agent that has gone a long way down one road is worth keeping when you want
to see the other one. `fork` starts a second agent on a copy of everything the
first has been told:

```sh
amx fork <id>                        # a copy, waiting for a turn
amx fork <id> "try it with sqlite"   # a copy, and what to do next
```

It prints the new agent's id, like `new`, and the two are their own from that
moment: nothing either says reaches the other. The copy runs in the directory
the original ran in, because a conversation is about the files it was held over,
down to the ones no commit has yet. That directory is the original's, so amx
records no worktree and no branch for the copy — `amx stop` on a fork takes its
pane and nothing else — and `amx diff <id>` on the original is where that work
is read.

What is copied is the session amx has on the record: the id amx minted at
spawn, where the vendor opens under one amx hands it, and the id the vendor
reported, where it reports through hooks. An agent with no session on its
record cannot be forked — a command row, or a vendor that reports and has not
reported yet — and amx says so rather than starting the task over. The copy's
log opens with a `fork` line naming the agent it came from and the conversation
it took, before the vendor has said anything at all: `amx events <id> --json`
is where to read it.

## An agent that was already there

Not every agent is one amx started. `adopt` writes the record an agent you
started yourself has been missing, so it joins the list beside the rest:

```sh
amx adopt                                   # the agent this is typed inside
amx adopt --task "port the importer"        # and what the row should say
amx adopt --name importer --task "port it"  # a name you chose
```

It is typed inside the agent being adopted — ask the agent to run it, or run
it yourself where it offers you a shell — and that is what says which pane and
which conversation are meant. Two variables carry it, and both describe the
agent that ran the command and no other: tmux's own `$TMUX_PANE`, and
whichever variable the vendor names its session in, which it puts in the
environment of every command it starts. `$CLAUDE_CODE_SESSION_ID` is claude's
and `$PI_SESSION_ID` is pi's, and the one that is here is what says which
vendor is in the pane: what somebody started themselves need not be what
`amx new` would spawn. Without them there is nothing to adopt and amx says so
rather than guessing at which agent on the machine was meant. An agent outside
tmux cannot be adopted at all: adopt needs it to be running inside a tmux pane,
because a pane is the only thing amx can watch and type at.

Nothing is started and nothing is sent. The agent goes on with whatever it was
in the middle of, amx prints the new id the way `new` does, and the row is there
the moment the command returns with what the pane was showing already on it — a
question and its choices, if that is where the agent is standing.

What that session id is worth afterwards is the vendor's answer. amx cannot put
its own `AMX_ID` in a pane it did not open, so the events of a vendor that
reports through hooks arrive with nothing on them saying whose they are, and
the session is what carries them home: amx finds the record whose session
matches the payload's. pi's reports carry its session the same way, so an
adopted pi comes home through the extension like any other, and the hook
answers each report with where the record is, so what an adopted pi is saying
streams to its card the same as one amx started. One whose
extension is not installed is read off its pane the way any quiet pi is. The
first report about that session also names the conversation the vendor keeps,
which `adopt` itself could not: from then on the card and `logs` read it the
way they read any agent's.
The id is worth writing down either way: a conversation amx still has an agent
going on is one it refuses to adopt a second time. An agent whose record has
ended is not in the way.

What amx did not do for this agent it does not claim. There is no worktree, no
branch and no commit to measure against, so `amx diff` has nothing to show and
`amx stop` takes the pane and nothing else — the pane the agent is sitting in,
so stopping an adopted agent is what ends it. amx holds no command it was
launched with either, so `resume` and `fork` have nothing to start again: it was
started by hand, and can be again.

## Ending one

```sh
amx interrupt <id>       # end the turn it is on, and leave the agent standing
amx stop <id>            # asks what to do with the worktree and the branch
amx stop <id> --force    # takes the defaults, asks nothing
amx stop <id> --worktree keep --branch delete
amx stop <id> --delete   # and forget the record too
amx resume <id>          # start it again on the conversation it had
amx resume --all         # everything that was stopped, as after a server death
```

`interrupt` ends the turn; `stop` ends the agent. `interrupt` sends `esc`,
which a vendor at work reads as drop what you are doing, so the work stops
where it stands and the agent is back at its prompt with the conversation
behind it whole. The pane stands, the worktree stands and the record keeps that
conversation, so an agent you interrupted is one you can go on talking to with
the next `send`. claude says nothing about a turn it was interrupted out of,
but amx ended that one itself, so the row does not wait a word out: it reads
idle as soon as the prompt is back on the pane.

A turn is the only thing that key can cut short, so `interrupt` exits `0`
having sent one and refuses in two other ways. An agent stopped on a question
is not working, and `esc` there dismisses the question, which is an answer
nobody can take back: the question goes to stdout the way `result` puts it
there, stderr names `amx answer <id> esc` as the verb that does mean to dismiss
it, and the code is `2`. A command row has no vendor in it to read a key at
all, and an agent amx can see no turn on — coming up, idle, parked or ended —
has nothing for one to cut short: both are `1`, and the line says what to do
instead. A `result` waiting on a turn somebody interrupted ends `1` as well,
because that turn is over and there is no answer to hand back.

Stopping asks the pane's process group to stop, waits, and only then kills it.
An agent cut down mid-sentence loses the answer it was writing. The defaults
lose nothing: the worktree goes, the branch stays, the record stays. A
worktree with uncommitted work in it is always kept, whatever you answer.
`--delete` says the record goes; `--force` says every question takes its
default. They are separate on purpose.

`resume` is also what brings back an agent amx parked. One that sat idle past
`park_after` lost its pane and nothing else, so there is a conversation to pick
up and no ending to undo: `amx resume <id>` puts it in a pane again on the
session it was parked on.

## Worktrees

In a git repository, `new` gives each agent a worktree of its own at
`<repo>/.amx/worktrees/<id>` on branch `amx/<id>`, cut from whatever was
checked out at the time. That commit is recorded, which is what lets `amx diff`
show the whole of an agent's work, including what it has already committed,
rather than only what it has not.

`.amx/` is kept out of the repository's status through `.git/info/exclude`, so
nothing about this shows up in a diff of yours. `--no-worktree` runs the agent
in the directory as it is, and `worktrees = false` makes that the default.

## Driving amx from a program

Four questions, four commands, and the exit code is the answer:

| code | means |
| ---- | ----- |
| `0`  | done: the answer, if there is one, is on stdout |
| `1`  | failed, stopped, or ended without an answer; nothing more is coming |
| `2`  | blocked: `result` and `send` on an agent that is asking, `answer` with nothing pending, `new` or `fork` at the agent cap |
| `3`  | `result --timeout` expired |
| `64` | the command line was wrong, including an answer the question would not take |

```sh
id=$(amx new --no-worktree --dir "$dir" "Read $brief and execute it exactly." \
     -- --session-id "$session")

amx ls --json                 # every agent: state, since, last_event, summary, question
amx ls --json --dir "$dir"    # the ones this run started, and no other run's

said=$(amx result "$id" --timeout 900)
case $? in
  0) merge "$said" ;;        # the turn ended, and that is what it said
  1) redispatch "$id" ;;     # it will not answer
  2) park "$id" "$said" ;;   # it is asking, and the question is what came back
  3) amx stop "$id" --force ;;
esac
```

`result` blocks until the turn ends and prints what the agent said, verbatim.
What you capture is what it wrote, not a rendering of it. It never waits
through a question. A question usually arrives *during* the wait, and a caller
that cannot see it cannot answer it, so the question goes to stdout with its
choices numbered under it, and the numbers are the ones `amx answer` takes.
After a `send` it waits for the turn after that message, never handing back the
previous turn's answer.

When the caller is itself an agent, hand it `skill/amx/SKILL.md`, which is
this loop written for one.

`ls --json` and `status --json` are stable. Fields are added, never renamed or
removed. Each row carries `id`, `state`, `evidence`, `rule`, `age`, `since`,
`last_event`, `ended`, `worked`, `seq`, `summary`, `question`, `options`,
`result`, `source`, `exit`, `kind`, `pr`, `task`, `dir`, `worktree`, `branch`,
`base`, `pane`, `socket`, `session`, `name` and `created`, so one `ls` call
answers both "is it still going?" and "when was it last heard from?" for every
agent at once. `age` keeps its three questions — how long a finished run
worked, how long a waiting agent has waited, and how long since anything was
heard from one still going — and `worked` is the spans of work the record has
added up. The table's column is the human reading of the same spans; programs
read the fields.

`state` is one of `starting`, `working`, `waiting`, `idle`, `done`, `failed`,
`stopped`, `unknown`. `done`, `failed` and `stopped` are endings; every other
state is an agent still worth waiting on. `kind` says what an outstanding
question is: `permission`, `question` or `trust`. `name` is what somebody here
called the agent, and null where nobody has: a program drawing its own list of
agents wants that word where there is one. `pr` is what the agent's branch has
open, each entry a `number` and a `standing` — `merged`, `closed`, `draft`,
`failing`, `changes`, `running`, `ready` or `open` — which is the word the
number's colour is drawn from in the view.

## How amx knows what an agent is doing

Nothing amx runs stays resident, so there is no process keeping this up to
date. A reader works it out at the moment you ask, in this order:

1. The record ended it. An exit code was written, or a stop was. Nothing
   overrules that.
2. The pane is gone. No pane, no agent — unless amx is the one that took it.
   A pane number on its own is not the question: tmux numbers panes from `%0`
   for each server, so a record that outlived its server names a number the
   next server handed to somebody else. A pane answers for the agent whose id
   is written on it, else for the agent its session is named for, else for
   nobody — amx opens every pane of its own in a session called `amx-<id>`,
   and writes the id on a pane it adopts, which is sitting in a session of
   yours. A record whose pane answers for somebody else has lost that pane and
   reads as stopped, the same as a record whose pane is missing. A pane let go
   after `park_after` is stamped on the record, so the agent keeps the state it
   was idle in and the evidence is `parked`: the next `enter`, `attach` or
   `resume` gives it a pane again.
3. The hooks are fresh. Within 8 seconds, the agent's own events are the best
   account there is.
4. The screen is all there is. Older than that, the pane is captured and
   matched against a ruleset of what the agent's screens actually look like.
   An extension may also say on disk that the turn goes on: it runs for as
   long as the turn does, so it beats on the record every few seconds until
   the turn ends, and a beat is heard from the same as a hook. While the beats
   keep landing the record is fresh and step 3 still holds. That is what a
   long tool call needs — the record goes quiet for as long as the call runs,
   and a screen no rule claims is otherwise the whole of what is left to read
   a turn that is plainly still going.
5. Neither says anything. No rule claims the screen, so the answer is
   `unknown`, with how long it has been since anything was heard, because "I
   can't tell" is only useful with that beside it. One exception: on a vendor
   that reports through hooks, a record the hooks left at `idle` or `waiting`
   keeps that word. Such a vendor says itself when a turn ends and names the
   question it stops on, so a screen amx cannot read — pi's prompt under its
   update notice — takes neither back. A record mid-turn still reads
   `unknown`: nothing has said that turn is over.

`amx status` says which of these it used. `evidence` in the JSON is the same
answer for a program.

A screen with a turn running on it says more than which state the agent is in.
claude spins one line above its composer for as long as the turn lasts, and
that line — `Forging… (22s · ↓ 1.3k tokens)` — says the turn is running and how
long for, in place of the tool call the record last wrote down. Before the
first tool call there is nothing written down at all, so the line is read from
the first look rather than after the hooks go quiet. It is read and not
recorded: it is true for a second, and the next reading takes it again.

The transcript is fresher than any of that. A vendor writes each thing it says
and each tool it calls to the session file at the moment it happens, while a
hook reaches amx some time afterwards, so a working row shows the last line of
that file: `Read src/importer.rs` while the record still says `Running Read`,
and the sentence the agent wrote between two calls while the record says
nothing about it at all. Only the end of the file is read, so a session that
has been running all day costs a row what a minute-old one does. Read and not
recorded, like the line over the composer.

So a working row says, newest account first: what the vendor is streaming as
the row is drawn — pi streams the words of the answer it is writing, and a
tool running has no stream — else the newest line of its transcript, else the
line it spins over its composer, else the `Running Bash` its last tool hook
left on the record.

## Vendors

What amx knows about a vendor is an entry in a table, not the shape of the
program. The entry says which dials the vendor declares and what flags they
become, which environment variables name its sessions, which hooks to wire
and what its screens look like — and what amx may ask of it: hooks, a
transcript, resume, fork, adopt, trust. A verb asks before it acts, so
`amx fork` on a vendor that cannot branch a session is a refusal naming the
gap, not a spawn that fails somewhere in a pane.

Two commands have an entry today. `claude` is the one amx runs unless told
otherwise. `pi` is the other. It has no settings file a hook can be named in,
so it reports through an extension amx writes where pi loads one from — `amx
doctor --fix` puts it there — one event per moment the way claude's hooks
report, and the report names the session file pi writes, which is the
conversation `amx logs` and the card read back. A pi without the extension is
read off its pane, the way a claude with its hooks unwired is, and `doctor`
says so. `agent = "pi"` in the config, or `--agent pi` on one spawn, runs it
for every new agent.

An entry also says where that vendor's models are found, which is what lets a
typed model pick the harness. claude's are a handful of aliases, so the entry
carries them itself: fable, opus, sonnet, haiku. pi reaches whatever its
providers hold, so the entry carries the command that prints them instead — `pi
--list-models`, a header line and then a row per model with the provider and the
id first — and amx runs it when it has to, keeping what it read for an hour so a
morning of spawns costs one pi process. A harness's own `models` in the config
stands in place of either, and a listing is never run once an earlier harness's
list has answered the word.

A command the table has no entry for — `agent = "opencode"` — gets the floor:
a real pane, a row that reads what the screen says, and no pretending beyond
that — the same footing every `--exec` command stands on.

Adding a vendor is adding an entry: the dials out of its `--help`, the
moments its hooks report, the anchors its screens are measured by. Every law
an entry must keep is a test over the whole table, and a test-only second
vendor answers most questions the other way from claude, so nothing in amx
can quietly assume the first entry is the only shape. `docs/vendors.md` is
the walk through it.

## Notifications

Two moments are worth interrupting somebody for: an agent that has stopped on a
question, and one whose command has finished. Both post a desktop notice
through `notify-send`, or `osascript` on macOS. Nothing is posted about a pane
its person is already looking at, and a machine with no notifier costs nothing:
the notice is handed over and never waited for.

## Configuration

`~/.config/amx/config.toml`, twelve keys and a table per harness:

```toml
agent = "claude"        # the command a new agent runs: claude, pi or your own
max_agents = 5          # how many live agents in a project before `new` refuses
max_total = 10          # a ceiling over every project on the machine
worktrees = true        # give each agent its own worktree in a repository
notifications = true    # desktop notification when one needs you or finishes
trust = false           # answer claude's folder-trust screen for trees amx cuts
park_after = 3600       # seconds an idle agent nobody is watching keeps its pane
theme = "default"       # which palette the view paints in

# The dials. A key left out is a flag amx does not pass, which leaves the
# choice to the vendor.
model = "opus"
permission = "plan"
effort = "high"

# What writes the one line a turn is worth, finished or still running. Left
# out, nothing runs.
summary_command = "claude -p 'Sum this up in eight words. Answer with the words alone.'"
```

Each harness amx has an entry for — claude and pi today — can have a table of
its own, named after the command it runs:

```toml
[claude]
models = ["opus", "sonnet"]
args = ["--add-dir", "/srv/shared"]

[pi]
models = ["anthropic/claude-opus-4-1", "openai/gpt-5"]
args = ["--approve"]
```

`models` is the models that pick that harness: it is the list a model is
looked for in, so a model named here says which harness starts and not only
which flag it is handed. Leave it out and the list is whatever the harness
itself offers. `args` is what every agent that harness runs carries on its
argv, however that agent was started, and a dial amx would have set stands down
for a flag written here the way it does for one written into `agent`.

A table naming a harness amx has no entry for is warned about by name and the
rest of the file still applies. A table that sets neither list says nothing,
so naming a harness costs nothing until you write one of them.

The repository ships the same file with every key explained and the defaults
written out, at `assets/config.toml`: copy it and change what you want changed,
and a copy nobody edits runs exactly as no file at all would.

Config is a convenience, never a gate. A file amx cannot read or parse falls
back to these defaults with a warning, because losing an agent to a stray
comma is the worse outcome. An unknown key is a warning and the rest of the
file still applies, and so is a dial the configured agent would not take.

A project can keep the same file at `<repo>/.amx/config.toml`, and it is laid
over yours a key at a time. A project file holding one line has changed its
mind about one key: everything else is still what you set. A harness table is
one key like any other, so a project that names one has said what that harness
is there, both lists, rather than edited the table you keep. Whatever amx cannot
use in it — an unknown key, a key of the wrong type, a file that will not open
— is a warning naming that file, and the file under it still stands.

The project is the repository rather than the tree of it you are standing in,
so several agents on one repository read one file. A worktree amx cut reads the
repository it was cut from; any other linked worktree reads the repository it
belongs to; a directory git has never heard of is the whole of its own project
and reads `<dir>/.amx/config.toml`. `.amx/` is kept out of the repository's
status, so the file is a checkout's own until somebody commits it.

That is what makes the two caps different questions. `max_agents` is a
project's: it counts the live agents in the project the new one would run in,
so a repository at its cap refuses nothing in the next one. `max_total` is the
ceiling over all of them, counted over every live agent on the machine, and
where nobody sets one there is none — a machine is as busy as the projects on
it ask between them.

The view reads the same files. `amx --dir /srv/app` is drawn under that
project's file — the dials it launches at, the palette it paints in — and its
header counts that project's agents against that project's `max_agents`. `amx`
on its own is about every agent there is, so it counts them against `max_total`
where you set one, and against nothing where you have not.

`park_after` is how long an agent that has stopped working keeps its pane. A
vendor sitting at its prompt holds a couple of hundred megabytes to do nothing
with, and a wall of them is the machine's memory spent on turns that ended
hours ago. So an agent that has been idle that long with nobody attached to it
loses the pane and keeps everything else — the record, the transcript, what it
last said — and its row turns from `✻` to the dot. `enter` on that row, `amx
attach` or `amx resume` starts the agent again where it left off. A pane
somebody is looking at is never taken, and neither is one whose agent you have
pinned over the wall with `ctrl+t`: pinning it is having said you want it in
front of you. `park_after = 0` turns the whole thing off, and a pane stands
until you stop it.

`summary_command` is what a row says about a turn. What a turn leaves behind is
an answer, and an answer does not open with a summary of itself, so without this
a finished row shows its first line. With it, the first reader to see a turn end
runs the command where the agent ran, hands it the whole answer on stdin with
`$AMX_ID` naming the agent, and writes the first line it prints onto the record
for every reader after. The command is the one the project that turn ran in
names, so a view standing over several projects boils each row down by its own
project's file. Nothing waits for it: the row keeps the answer until
the line arrives, and a command that fails, that is not installed, or that says
nothing costs the line and nothing else. Each finished turn is asked about once,
by one amx, and one turn at a time: a view opened on a week of finished agents is
a queue rather than a week of model calls at once, and a caller running `ls` in a
loop does not start the command again on every pass. Left out, nothing is run
and nothing is spent.

The same command says what a row means while the turn is still running. There is
no answer yet, so a working row shows the last thing the agent did — `Read
src/importer.rs`, or the `Running Bash` a tool hook left. With the key set, the
view asks the command what the turn is about every three minutes and puts its
line on the row instead. What goes in on stdin is the conversation so far, in
the shape `amx logs` prints it and read off the end of the transcript, so a
session that has run all day costs the same as one that started ten minutes ago.
The line stands until the agent says something the command cannot have read; the
row goes back to the transcript's newest line then, until the next rewrite. It
goes off the record when the turn ends, so a finished row is about the answer.

Only the view ever runs the command, for either question. `ls`, `status` and
`statusline` print and exit, and starting a command nothing will be there to
hear back would cost money for a line nobody reads. So the price of the key is
the view being open: one call per finished turn, plus one per working agent
every three minutes.

## Themes

The view spends colour on six things, and a theme is those six answers:
`waiting`, `done`, `failed`, `stopped`, `accent`, `cursor`. Everything else
on the screen is said in words, in weight and in the dim your terminal
already has, so a theme is a file a minute writes:

```toml
# ~/.config/amx/themes/mine.toml
waiting = "#ffc107"   # something is waiting on a person
done    = "#4eba65"   # it went the way it was meant to
failed  = "#ff6b80"   # it was attempted and it failed
stopped = "#999999"   # it was ended by hand
accent  = "cyan"      # what the next agent will be started with
cursor  = "#373737"   # the line the cursor is on, as a background
```

`theme = "mine"` in the config names it. A value is a colour the way a
terminal says one: a name (`cyan`), a 256-colour index (`134`), or a hex
(`#4eba65`). A role left out keeps the default's answer, and a file that
cannot be read or understood degrades to the default palette whole, with a
warning, under the same law as the config — a view painted wrong is a view,
and no view at all is not.

Two themes ship in the binary. `default` is measured off claude's own
palette, so the wall and the panes beside it read as one thing. `terminal`
names no colour of its own: every value is one of your terminal's named
colours, so the view follows whatever your terminal wears, light or dark.

The file is live. The view stats it once a second, beside the reading it is
already taking, and an edit repaints the open view on the next pass — no
restart. A name with a `/` in it is read as a path, which is how a theme
kept beside a project or shared between machines is reached. The rest —
what is deliberately not themed, and why — is in `docs/themes.md`.

## What is on disk

One directory per agent under `~/.local/state/amx/agents/<id>/`:

- `meta.json` holds how it was started and where to find it again.
- `state.json` holds what it is doing, as the last event left it.
- `events.jsonl` holds one line per event, in the order they arrived.
- `output` holds everything a command printed, piped there from its pane while
  it ran. Only a `--exec` row has one: an agent's pane is its vendor's
  full-screen drawing, and a file of that says nothing anybody can read.
- `pr.json` holds what its branch's pull requests were doing when the view last
  asked, and is only there once one has.
- `summary.asked` holds the last ask a `summary_command` made: the turn it was
  about, when it went out, and whether it came back. It is what keeps one amx
  asking and every other one reading, what spaces the rewrites of a turn still
  running, and it is only there once the key is set.
- `scratch/` is the agent's own directory to write in. Every pane amx starts is
  told where it is in `$AMX_AGENT_DIR`, and it goes when the record goes, so
  anything worth keeping belongs in the worktree with the rest of the work.

Beside that directory rather than in it, `~/.local/state/amx/view.json` is the
view's own: how you last arranged the list, and whether the status line has
been offered. Nothing in it belongs to an agent, and deleting it costs you the
arrangement and nothing else.

Writes go through a lock, one at a time; readers never lock and never see half
a document. `ls` sweeps records whose agent finished more than a week ago. A
stopped agent's record is never swept. Somebody stopped it on purpose, and its
record is where the branch it left behind is named.

## Building

```sh
cargo build
cargo test
```

The test suite drives the built binary against throwaway tmux servers and a
stand-in for the agent CLI, so it needs a real tmux but no network and no API
key.
