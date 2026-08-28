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

`doctor` checks the six things that have to be true before an agent can run:
tmux, the agent command, the config file, the hooks, a state directory amx can
keep records in, and no agent already stopped at a screen the vendor puts in
front of the work. Every check that fails says what to do about it.

Where a tmux server is already running, it checks a seventh: that the directory
that server is standing in still exists. A server keeps the directory it was
started in for as long as it lives, and once that directory is deleted every
pane it starts lands somewhere that is not there and dies immediately — which
from the outside looks like agents failing in under a second having said
nothing. Restarting the server is the fix, and the check prints the command.

`--fix` does the one repair amx can make safely: wiring amx's seven hooks into
`~/.claude/settings.json`, beside whatever is already there, after asking once
and backing the file up. `amx uninstall` puts the backed-up bytes back and
removes amx's records. It refuses while any agent is still running: those
records are the only place their answers are kept.

Without the hooks amx falls back to reading panes, which is enough to say what
an agent is doing but not enough to hand you what it said. Answers come from
the events. `amx doctor` says when they are missing.

## Starting an agent

```sh
amx new "fix the login bug"            # in a repository: its own worktree
amx new --no-worktree "run the tests"  # in this directory, as it is
amx new --name importer "port it"      # a name you chose
amx new --dir /srv/app "tail the log"  # somewhere other than here
```

Every agent is one detached tmux session called `amx-<id>`, on the server you
are already in or on your default one. Nothing is tiled and nothing is
switched to: starting an agent from inside tmux leaves the window you were
looking at exactly where it was. The server is yours, so it reads the
`~/.tmux.conf` you wrote for it and amx brings no config of its own.

`new` prints the agent's id and nothing else. Everything after this takes that
id.

Four dials say what is being launched and how it should behave:

```sh
amx new --agent claude --model opus --permission plan --effort high "port it"
```

Each falls back to the config, and the config falls back to the vendor: amx
passes no flag at all for a dial nobody turned. Which dials exist is the
vendor's answer, so a value claude would refuse is refused here instead, while
the command is still on your screen.

Anything after `--` is handed to the agent command untouched, and the task is
added after it, where a prompt goes:

```sh
amx new "port the importer" -- --model opus --session-id "$uuid"
```

`--model` before the separator turns amx's dial; the same word after it is
claude's own flag, and amx stands its dial down rather than send the flag
twice.

## A shell command as a row

Not everything worth keeping an eye on is an agent. `--exec` runs a plain
command in a pane, and it gets a row beside the rest:

```sh
amx new --exec 'cargo test --all'
amx new --exec 'ssh build01 make release && curl -fsS "$HOOK"'
```

The whole command goes to `sh -c`, so a pipeline, an `&&` or a redirect is one
row and one exit code. It runs where you typed it rather than in a worktree of
its own: a command has no conversation to keep apart from the next one, and a
fresh checkout is not where `cargo test` was meant to run. There is no vendor
either, so the four dials and anything after `--` are refused beside `--exec`
rather than quietly dropped.

The row ends `done` or `failed` by what the command exited with, and the code
itself is `exit` in the JSON. `amx result` has nothing to hand back — a command
answers nothing, it exits — and `amx logs` reads the pane only while the
command is still in it, because the pane goes when the command does. Output
worth reading afterwards is output to redirect somewhere:
`amx new --exec 'make release > build.log 2>&1'`.

Like every pane amx starts, it is told where its own scratch directory is in
`$AMX_AGENT_DIR`. While it runs there are no hook events to hear from, so after
the first few seconds the row reads `unknown` — amx saying it cannot account
for what is on that screen, which it cannot: the screen belongs to somebody
else's program.

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
AMX                                  1 idle   1 done   2/5 running    1 WAITING
└ next  claude   model  default   permission  default   worktree  new

 NEEDS INPUT ┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈     1
  ? port-import-b2c   Which fixture should the port keep?                    29s
  │ Which fixture should the port keep?
  ╰ 1. the old one   2. the new one   3. both

 IDLE ┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈     1
  ○ fix-login-a1b     the login bug is fixed                                  4m

 COMPLETED ┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈     1
  ● tidy-imports-d4e  did what it was asked                                   2m

space closes it   enter attach   ctrl+x stop   ctrl+s axis   q quit   ? keys
```

The first row is what there is: `AMX`, the directory the view was opened on
where one was named, a count per group, how many are running against the limit
that would refuse the next one, and at the far end the number the view is
opened to read — how many agents are waiting on you, in reverse video, or
`nothing waiting` in the same place when none are. The second row hangs off it
on a `└` and holds the dials, which are about the agent that does not exist
yet, so nothing on it can be read as a fact about the fleet. On a terminal
under ten rows the second row goes and the first stays. The row at the foot is
whatever keys the line under the cursor makes true, cut to what the terminal
holds, with `?` pinned to the end of it.

The list answers one question, so it is gathered under the answer: the agents
stopped on a question come first, then the ones mid-turn, then the ones sitting
at their prompt, then the ones whose command has ended. A heading is a line like
the rows under it: the group's name in caps, a dim rule run out to its count,
and the count right-aligned in the column the ages under it are right-aligned
in, so the right margin is one line of numbers rather than two. The rule is
drawn in `┈`, the lightest dash there is, and every rule on the screen is —
dim is the weight a summary wears too, but a terminal inks a box-drawing glyph
across the whole cell, so a solid rule reads brighter than the words beside it
at the same colour. Half the cells left blank is what puts the two level. A
group holding a failure says so in front of the rule, because a failed agent is
why somebody came to the screen. The cursor stops on a heading, and shutting it
puts its agents away behind the count that was already there.

A row is one line, always, on columns the screen fixes rather than the fleet:
two marks, the state glyph, the name, what the agent last said, and the seconds
at the end. They stand where they stood when the last agent ended, so the row
you learned wide is the row you get narrow — under 100 columns the name column
is the one that gives way. The marks cost the row no width and line up into
columns of their own: the first is on a row nobody has been to read, the second
on one you are holding at the top of its group. A wall with nothing on it keeps
everything above the list and offers, where a name would be, the only two keys
that lead anywhere from there: `n` and `?`.

The seconds at the end of a row are the time the agent has worked: ticking
while it works, standing still while it waits or sits idle — an agent left at
a question all afternoon has not worked an afternoon — and stopped for good
when the run ends. The `amx ls` table prints the same number. How long a
question has been standing is on the card, in its title.

`ctrl+s` gathers the same agents under the directory each one runs in, and the
screen changes twice for it. A heading is a path rather than a word, so it is
not put in caps: the parents stand dim behind a bold last segment, on the same
rule and the same count a group heading carries, and a path too long for the
line loses its middle rather than its end — the end is the segment that says
which worktree of a project this is. And every row grows a state word between
its name and what the agent said, because the heading over it no longer says
what state the row is in:

```
 ~/code/amx ┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈     2
  ? port-import-b2c   waiting   Which fixture should the port keep?          29s
  ● tidy-imports-d4e  done      did what it was asked                         2m

 /srv/app ┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈     1
  ○ fix-login-a1b     idle      the login bug is fixed                        4m
```

Eight cells, which is what `starting` needs and what the shorter words are
padded out to: a state word cut down would be a lie. The summary column pays
for all of it, so the name, the seconds and the group's count sit exactly where
they sat on the other axis — switching the axis moves one boundary rather than
the whole table.

| Key | What it does |
| --- | ------------ |
| `↑` `↓` | walk the agents |
| `space` | the card: what one is asking, and the answer |
| `enter` `→` | put its session in front of you, or shut the group under the cursor |
| `esc` | put the card away, or leave a line alone |
| `n` | start an agent |
| `alt+n` | start the line and go to the agent it started |
| `r` | reply: a message, or an answer on the card |
| `d` | what it has changed |
| `pgup` `ctrl+b` | page the card, when it holds more |
| `pgdn` `ctrl+f` | and the other way |
| `ctrl+x` | stop it, twice to forget it, and twice on a heading to clear the group |
| `ctrl+r` | call it something else |
| `ctrl+g` | write the line in `$EDITOR` |
| `alt+1..9` | reach the agent at that place on the wall |
| `ctrl+s` | gather them by state or by project |
| `ctrl+t` | hold it at the top of its group |
| `shift+↑` `shift+↓` | move it up or down its group |
| `alt+enter` | a newline in the line, without sending it |
| `alt+v` | which vendor the next agent runs |
| `alt+m` | which model the next agent is given |
| `alt+w` | whether it gets a worktree of its own |
| `shift+tab` | what the next agent may do without asking |
| `?` | every key, where the list was |
| `q` `ctrl+c` | close the view |

Inside tmux, `enter` moves your client to the agent's session and leaves the
view drawing in the session it was already in, so switching back lands on the
list where you left it. Outside tmux the view is the only thing on the
terminal, so it lends the terminal to a tmux client instead and takes it back
when you detach.

`alt+1..9` does the same for the first nine agents by where they are on the
wall, counting rows from the top and skipping the headings, without walking the
cursor to them first. It is the fleet you already have in front of you, reached
by the number you were about to count to.

`space` opens the card, and the card is not a box. It is a spine: a column of
`│` standing in the column the row drew its own state glyph in, closed with a
`╰` on its last line, and everything it says written from the column the row's
name starts in. It hangs directly under the row it is a look at and moves the
rows below it down, so what it belongs to is said by where it stands rather
than by four borders and the two rows and two columns of wall they would cost.
It takes half the screen at most, and always leaves a row of the list it was
opened from. It opens straight onto what it has to show: which agent this is
and what it is doing are on the row two cells above, and a card that repeated
them would spend a row of the wall on what you were already looking at.

What the card's body is follows the agent. One that is working shows its live
screen, chrome cut, tracking output as it lands. One whose turn is over and
whose record holds the answer — idle at its prompt, done, failed or stopped
alike — shows that whole answer, top first, however the pane happens to be
scrolled; only an idle agent with nothing recorded falls back to the screen
picture. A waiting agent's card is the question block alone.

`pgup` and `pgdn` page inside the card's body when it holds more than the card
has room for: a patch or a recorded answer down from its top, a live screen up
from its bottom. A paged card takes a row of its own to say how far from that
edge it stands — `↓ 12 more`, at the far end of it — and holds still, new
output and all, until you page back to the edge, press an arrow, or open it
again; an agent stopping at a question takes the card back regardless, because
a question is never left behind history. The arrows never page: they keep
walking the list, card in tow. `ctrl+b` and `ctrl+f` are the same two pages for
a keyboard with no page keys on it — pgup and pgdn exactly. A lone `ctrl+b`
under tmux's default prefix is tmux's own business: `ctrl+b ctrl+b` is what
reaches the view there.

`?` puts every key where the list was, in the five groups they are learned in —
walk, look, start, arrange, dials — each under the heading a group of agents
wears, so the overlay reads as the same screen showing something else rather
than as a manual somebody opened. From 100 columns the groups stand in two
ruled columns, cut where the two halves come nearest to holding the same number
of keys, and nothing is shortened to fit. Narrower than that the second column
is given up whole rather than squeezed: one column, every key still saying what
it does in full, paged with `pgup` and `pgdn`, and the foot of each page says
which page it is and which of those two keys turns it — a key nobody can reach
is a key the screen may as well not list. Any other key goes back to the list,
and `q` closes the view.

`ctrl+x` forgets nothing on the first press, and the first press is the same
press on every row. An agent that is still running — sitting idle at its
prompt included — is stopped, which costs you the pane and nothing else, and
the row is armed in the same move; one whose command has ended is armed as it
stands. Where the row was saying what the agent did it says `ctrl+x again
forgets` for about two seconds, and the press inside that window is the one
that takes the record away, and the worktree with it where that tree holds
nothing no commit has. Leave it alone and the row goes back to saying what it
was saying, having forgotten nothing. On a heading the same two presses reach
the whole group, whatever the states beneath: the first stops every agent
under it that is still running and arms every row in place — each row wears
the warning itself — and the second press on a heading over the armed rows,
inside the window, forgets them all, each under the same worktree safety a
single row gets. That heading is not always the one that was pressed, because
stopping is what moves a group to completed: the rows carry the arming, the
cursor is kept with them while the window is open, and the heading standing
over them is where the second press lands.

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
holds the one under the cursor at the top of its group, so the agent you are
watching stays where you are looking, and its row carries a `▲` beside the `•`
of a row nobody has read. `shift+↑` and `shift+↓` move an agent a row at a time
past the others in its group; an agent that starts after you have put a group
in order joins the bottom of it, because a group you arranged by hand is not
one amx goes on sorting under you. Both of those and whichever way `ctrl+s`
last gathered the fleet are written to `~/.local/state/amx/view.json` as you
go, so the next view opens on the wall you left.

A line being typed hangs off a rule, and the rule is where the whole mode is
said. Its near end names which of the five lines this is — a task, a narrowing,
a message, an answer, a rename — with the agent it is aimed at beside it where
it is aimed at one, and after that the one thing true of all five: while the
line is open a letter is a letter and not the key it is bound to, and `esc` is
the way out. Its far end carries what the next agent may do without asking, in
reverse video, set into the edge: it is the one dial somebody is about to press
enter past.

The line itself begins with a `❯` in the column the rule's own label starts in,
whichever of the five it is holding, so moving between them does not move the
words you are reading. It is bold under a block cursor, and everything above
the rule goes dim for as long as the mode is on — rows, headings, counts and
dials in the one pass — so the band below the rule is the only thing on the
screen still carrying weight.

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
and nowhere else. `s:` and `a:` narrow the list by state and by name rather
than starting anything: `s:waiting`, `a:import`, or `a:#12`. `m:`, `p:`, `w:`,
`d:` and `agent:` turn the dials for the one agent that line starts, as in
`m:opus w:off port the importer`. `d:` is where that one runs:
`d:/srv/app port it`, `d:~/code/importer port it`, or a name read against the
directory you opened the view in, the way a shell prompt standing there would
read it.

`alt+n` enters that line and takes you with it: the agent is started and your
terminal lands in its session, by the same two roads `enter` on a row takes.
Everything else about it is what `enter` on the line would have done, the
question a task of three letters is asked included, so the answer to that
question is what carries you there.

`ctrl+g` writes that line in your own editor. It opens `$VISUAL`, or `$EDITOR`,
or `vi`, on a file holding whatever is on the line, hands it the terminal, and
puts back what you wrote when you close it — pressed on the list it opens a
task line first, so a task worth a paragraph is one keystroke away. An editor
that exits unhappily, which is `:cq` in vim, leaves the line exactly as it was.

A task of fewer than four characters is asked about once before anything is
started: `y` starts it and any other key hands the line back with what you
typed still on it. `n` opens the line and the letter after a stray `n` is a
task nobody meant, where `wip` is one somebody does.

## Pull requests

An agent's work goes onto a branch, and what becomes of it after that is the
one thing the rest of a row cannot say. Where the branch has a pull request,
the row carries its number:

```
  ● fix-login-a1b     #12  the login bug is fixed                             4m
  ✻ port-import-b2c   #40  Running Bash                                       4s
```

The number is coloured by how it is going, in the same colours the rest of the
view uses. Merged and approved are green, a failing check is red, changes
requested is yellow, a request closed without going in is grey, and a draft is
dimmed. A request whose checks are still running and one nobody has read yet
both take your terminal's own colour, because neither has an answer to that
question yet. The card says which of the two it is in words, and lists every
request the branch has rather than the one the row had room for.

`a:#12` narrows the list to it. That is the word you have in front of you when
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
amx attach <id>        # hand this terminal to its pane
amx logs <id>          # the last of what its pane has printed, without attaching
amx logs <id> --lines 40
amx send <id> "and now the linter"
amx answer <id> y      # the keys a prompt reads: y, n, 1-9, enter, esc
amx answer <id> 1,3    # a question that takes several: check these two
amx answer <id> --text "keep the old importer"   # the row it offers for words
amx answer <id> 1 --note "and keep the subtitle" # a note beside the choice
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
screen of it. An agent with no conversation to read — a command row, a claude
adopted mid-session — gets the pane's picture instead, with the vendor's own
composer, statusline and mode footer cut off the bottom the way the card cuts
them. Once the pane is gone the record is what is left, and `logs` prints the
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

What is copied is the session the vendor recorded, so an agent that never
announced one cannot be forked, and amx says so rather than starting the task
over. The copy's log opens with a `fork` line naming the agent it came from and
the conversation it took, before the vendor has said anything at all:
`amx events <id> --json` is where to read it.

## A claude that was already there

Not every agent is one amx started. `adopt` writes the record a claude you
started yourself has been missing, so it joins the list beside the rest:

```sh
amx adopt                                   # the claude this is typed inside
amx adopt --task "port the importer"        # and what the row should say
amx adopt --name importer --task "port it"  # a name you chose
```

It is typed inside the claude being adopted — ask the agent to run it, or run it
yourself in its shell mode — and that is what says which pane and which
conversation are meant. Two variables carry it, and both describe the claude
that ran the command and no other: tmux's own `$TMUX_PANE`, and
`$CLAUDE_CODE_SESSION_ID`, which claude puts in the environment of every command
it starts. Without them there is nothing to adopt and amx says so rather than
guessing at which claude on the machine was meant. A claude outside tmux
cannot be adopted at all: adopt needs it to be running inside a tmux pane,
because a pane is the only thing amx can watch and type at.

Nothing is started and nothing is sent. The agent goes on with whatever it was
in the middle of, amx prints the new id the way `new` does, and the row is there
the moment the command returns with what the pane was showing already on it — a
question and its choices, if that is where the agent is standing.

That session id is what keeps it working afterwards. amx cannot put its own
`AMX_ID` in a pane it did not open, so this agent's hook events arrive with
nothing on them saying whose they are, and amx finds the record by the session
the vendor stamps on every one of them instead.

What amx did not do for this agent it does not claim. There is no worktree, no
branch and no commit to measure against, so `amx diff` has nothing to show and
`amx stop` takes the pane and nothing else — the pane that claude is sitting in,
so stopping an adopted agent is what ends it. amx holds no command it was
launched with either, so `resume` and `fork` have nothing to start again: it was
started by hand, and can be again.

## Ending one

```sh
amx stop <id>            # asks what to do with the worktree and the branch
amx stop <id> --force    # takes the defaults, asks nothing
amx stop <id> --worktree keep --branch delete
amx stop <id> --delete   # and forget the record too
amx resume <id>          # start it again on the conversation it had
amx resume --all         # everything that was stopped, as after a server death
```

Stopping asks the pane's process group to stop, waits, and only then kills it.
An agent cut down mid-sentence loses the answer it was writing. The defaults
lose nothing: the worktree goes, the branch stays, the record stays. A
worktree with uncommitted work in it is always kept, whatever you answer.
`--delete` says the record goes; `--force` says every question takes its
default. They are separate on purpose.

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
`base`, `pane`, `socket`, `session` and `created`, so one `ls` call answers
both "is it still going?" and "when was it last heard from?" for every agent
at once. `age` keeps its three questions — how long a finished run worked, how
long a waiting agent has waited, and how long since anything was heard from
one still going — and `worked` is the spans of work the record has added up.
The table's column is the human reading of the same spans; programs read the
fields.

`state` is one of `starting`, `working`, `waiting`, `idle`, `done`, `failed`,
`stopped`, `unknown`. `done`, `failed` and `stopped` are endings; every other
state is an agent still worth waiting on. `kind` says what an outstanding
question is: `permission`, `question` or `trust`. `pr` is what the agent's
branch has open, each entry a `number` and a `standing` — `merged`, `closed`,
`draft`, `failing`, `changes`, `running`, `ready` or `open` — which is the word
the number's colour is drawn from in the view.

## How amx knows what an agent is doing

Nothing amx runs stays resident, so there is no process keeping this up to
date. A reader works it out at the moment you ask, in this order:

1. The record ended it. An exit code was written, or a stop was. Nothing
   overrules that.
2. The pane is gone. No pane, no agent.
3. The hooks are fresh. Within 8 seconds, the agent's own events are the best
   account there is.
4. The screen is all there is. Older than that, the pane is captured and
   matched against a ruleset of what the agent's screens actually look like.
5. Neither says anything. No rule claims the screen, so the answer is
   `unknown`, with how long it has been since anything was heard, because "I
   can't tell" is only useful with that beside it.

`amx status` says which of these it used. `evidence` in the JSON is the same
answer for a program.

A screen with a turn running on it says more than which state the agent is in.
claude spins one line above its composer for as long as the turn lasts, and
that line — `Forging… (22s · ↓ 1.3k tokens)` — is what the row shows the agent
doing, in place of the tool call the record last wrote down. It is read and not
recorded: it is true for a second, and the next reading takes it again.

## Vendors

What amx knows about claude is an entry in a table, not the shape of the
program. The entry says which dials the vendor declares and what flags they
become, which environment variables name its sessions, which hooks to wire
and what its screens look like — and what amx may ask of it: hooks, a
transcript, resume, fork, adopt, trust. A verb asks before it acts, so
`amx fork` on a vendor that cannot branch a session is a refusal naming the
gap, not a spawn that fails somewhere in a pane.

`agent = "opencode"` in the config, or `--agent opencode` on one spawn, runs
that command for every new agent. A command the table has no entry for gets
the floor: a real pane, a row that reads what the screen says, and no
pretending beyond that — the same footing every `--exec` command stands on.

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

`~/.config/amx/config.toml`, ten keys and no more:

```toml
agent = "claude"        # the command a new agent runs
max_agents = 5          # how many live agents before `new` refuses
worktrees = true        # give each agent its own worktree in a repository
notifications = true    # desktop notification when one needs you or finishes
trust = false           # answer claude's folder-trust screen for trees amx cuts
theme = "default"       # which palette the view paints in

# The dials. A key left out is a flag amx does not pass, which leaves the
# choice to the vendor.
model = "opus"
permission = "plan"
effort = "high"

# What writes the one line a finished turn is worth. Left out, nothing runs.
summary_command = "claude -p 'Sum this up in eight words. Answer with the words alone.'"
```

Config is a convenience, never a gate. A file amx cannot read or parse falls
back to these defaults with a warning, because losing an agent to a stray
comma is the worse outcome. An unknown key is a warning and the rest of the
file still applies, and so is a dial the configured agent would not take.

`summary_command` is what a finished row says. What a turn leaves behind is an
answer, and an answer does not open with a summary of itself, so without this
the row shows its first line. With it, the first reader to see a turn end runs
the command where the agent ran, hands it the whole answer on stdin with
`$AMX_ID` naming the agent, and writes the first line it prints onto the record
for every reader after. Nothing waits for it: the row keeps the answer until
the line arrives, and a command that fails, that is not installed, or that says
nothing costs the line and nothing else. Each turn is asked about once, by one
amx, and one turn at a time: a view opened on a week of finished agents is a
queue rather than a week of model calls at once, and a caller running `ls` in a
loop does not start the command again on every pass. A verb that prints and
exits while the command is still thinking takes the ask with it, and the next
reader along makes it again. Left out, nothing is run and nothing is spent.

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
- `pr.json` holds what its branch's pull requests were doing when the view last
  asked, and is only there once one has.
- `summary.asked` holds the last ask a `summary_command` made: the turn it was
  about, when it went out, and whether it came back. It is what keeps one amx
  asking and every other one reading, and is only there once the key is set.
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
