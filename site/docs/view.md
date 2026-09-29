# The view

Type `amx` with no verb and you get the view: a full-screen list of every
agent, grouped by what it needs from you. It runs on whatever terminal you
typed it at, inside tmux or outside it. Down a pipe it prints the `amx ls`
table instead and exits.

```sh
amx                    # every agent
amx --dir ~/code/app   # only agents working under that directory
```

With `--dir`, the view also stands in that directory: new agents start there,
and the header counts against that project's `max_agents`.

## The screen

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

From the top:

- The header counts each group, says how many agents are running (against
  the cap where there is one), and ends with how many are waiting on you.
- The second row holds the dials for the next agent you start: vendor,
  model, permission, effort and worktree. They change nothing about agents
  already running.
- The list.
- A line you are typing, when you are typing one.
- The keys that apply to the line under the cursor, with `?` always last.
  Messages about the key you just pressed appear here too: red for a failure,
  amber for a refusal, dim for advice or success.

While the view is open the terminal title reads `amx · 2 waiting`, or `amx`
when nothing waits.

## Groups

| Group | What is in it |
| --- | --- |
| Pinned | Agents you pinned with `ctrl+t`, whatever their state. |
| Ready for review | Finished agents whose branch has an open pull request that is not a draft. |
| Needs input | Agents stopped on a question or a permission prompt. |
| Working | Agents starting up or in the middle of a turn. |
| Completed | Everything that has ended: idle at its prompt, done, failed, stopped, or unknown. |
| Asleep | Agents you put to sleep with `z`. They still count as waiting if they are. |

`enter` on a heading shuts the group and shows a count in its place, such as
`Completed 27`. A group holding failures says so either way:
`Completed 27 · 2 failed`. A group with more than 30 rows folds the rest
behind a `… N more` line.

An agent with subagents is drawn with its children under it, and the family
sits under the most urgent group any of them is in. A child waiting on a
question pulls the whole family under Needs input. Pins and sleep are the
exception: they apply to the one row you pressed them on.

## Rows

Each row is one line: a glyph, the name, what the agent last said or did, and
the time it has spent working.

| Glyph | Meaning |
| --- | --- |
| `✻` | There is a live process: you can attach, answer or stop it. It animates while a turn runs. |
| `∙` | The process has gone. The record is still there to read, and a parked agent comes back on `enter`. |

The glyph's colour is the state:

| Colour | State |
| --- | --- |
| amber | waiting on a question |
| green | turn over, at its prompt or ended |
| red | failed |
| grey | stopped by you |
| your terminal's colour | starting, working, or unknown |

The name also turns amber or red for waiting and failed agents, so you can find
them without reading glyphs. The name under the cursor is brighter, and the
agent you last went into keeps the accent colour until you go into another.

The name is the agent's session title where the vendor gives one (claude
does), else its id. `ctrl+r` renames it. The id itself never changes; `/`
finds a row by either.

The time ticks while the agent works and stands still while it waits or sits
idle. How long a question has been waiting is on the card.

When the branch has pull requests, the row shows the number, coloured by its
state. See [Worktrees](worktrees.md#the-pull-request-column).

`v` adds the vendor, model and effort to every row. Press it again to hide
them.

## Gathering by directory or repository

`ctrl+s` changes how the list is grouped. It cycles state, directory, state,
repository:

- state: the groups above.
- directory: one heading per project directory.
- repository: one heading per git repository, with all its worktrees under it.

On the path axes each heading ends with counts in the header's words, and each
row gets a state column:

```
 ~/code/amx                                              1 waiting     1 done
  ✻ port-import-b2c   waiting   Which fixture should the port keep?          29s
  ∙ tidy-imports-d4e  done      did what it was asked                         2m

 /srv/app                                                              1 done
  ✻ fix-login-a1b     done      the login bug is fixed                        4m
```

A task line opened on a heading, or on any row under it, starts the agent in
that project.

## The card

`space` opens the card for the agent under the cursor. It takes the bottom of
the screen (at most 14 rows, never more than half) and the list stays where it
is above it. Moving the cursor changes the card. `space` or `esc` closes it.

What the card shows:

| Agent | Card |
| --- | --- |
| claude, pi, codex, opencode | The conversation from the vendor's transcript: prompts, answers with Markdown rendered, one line per tool call. A finished turn opens at the end of its last answer. |
| pi, while working | The conversation plus a live tail of the answer being written. |
| waiting on a question | The question and its choices. |
| a command row | The last 256 KB of what the command printed, in colour. |
| an adopted agent before its first report | Its live screen, or its recorded answer once the turn ends. |

Paging:

| Key | Does |
| --- | --- |
| `pgup` `ctrl+b` | Page up. |
| `pgdn` `ctrl+f` | Page down. |
| `ctrl+u` `ctrl+d` | Half a page. |

A paged card shows how far it is from its edge (`↓ 12 more`) and holds still
until you page back, press an arrow, or reopen it. A new question always takes
the card back.

Under tmux's default prefix, press `ctrl+b ctrl+b` to send `ctrl+b` to the
view.

### The diff card

`d` shows the agent's changes as a patch on the card, measured the way
`amx diff` measures them. `alt+d` opens the same patch in the viewer named by
the `diff` config key, full screen.

`ctrl+n` and `ctrl+p` step through the patch a hunk at a time. Text you type on
the card's line stays with the hunk you were on, so you can write a note per
hunk. Before the first hunk is the top of the patch, for an opening remark.
`enter` sends everything as one message: the opening, then each note under the
file and line it is about. Noted hunks are marked in the waiting colour, and the
key row says how many notes `enter` would send. `esc` closes the card and drops
the notes.

## Replying and answering

Every card ends with a line. Type on it and `enter` sends what you typed.

| Agent state | The line |
| --- | --- |
| working or idle | `reply`: the text goes to the agent as a message, like `amx send`. |
| ended or parked | `reply`: the text brings the agent back on its session, like `amx resume <id> "text"`. |
| waiting on a question | The answer the question takes, like `amx answer`. |
| command row, or nothing to resume | `nothing is listening`. |

On an empty line, `space` closes the card and `enter` attaches, as they do
without a card. Once you type a character they become a space and send. `esc`
closes the card and the line together. `shift+enter`, `alt+enter` or `ctrl+j`
adds a newline. `ctrl+g` opens the line in `$VISUAL` or `$EDITOR`.

With a card open, letters go to the line. Arrows, page keys, `ctrl+x`,
`ctrl+t`, `ctrl+s`, `alt+1..9` and `shift+↑` `shift+↓` still act on the list.

`/` and `@` on the card's line offer the agent's own skills, commands, agents
and project files, the same as on the task line.

### Picking a choice

When a question's choices can be picked with one key, the line reads `1-2
picks` and a digit answers at once, with no `enter`. The line reads `press`
instead, and waits for `enter`, in three cases:

- a question that takes several choices
- a question whose choices carry a preview, where you may add a note
- a permission box, since an allowed tool call cannot be undone

Digits are only read as choices on an empty line. To answer with text that
starts with a digit, type another character first.

### Queued messages

A message sent while the agent is mid-turn waits in the vendor until the turn
can take it. The card shows it under the conversation as `❯ your message ·
queued` until the agent takes it. `amx status <id>` prints the same as
`queued` lines.

## Starting an agent

`n` opens the task line. Type the task and press `enter`: the agent starts
with the dials from the header, and the cursor moves to its new row.
`alt+n` starts it and takes you straight into its session.

```
TASK · letters are text until esc ┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈┈ vendor default ┈┈
❯ add a retry to the webhook█
enter starts it   alt+enter newline   shift+tab permission   esc cancels
```

The agent starts in the directory the view was opened in, or in the project
the cursor's heading names on a path axis. A task shorter than four
characters asks for confirmation: `y` starts it, any other key puts the line
back.

`ctrl+g` writes the task in your editor. Pressed on the list, it opens a task
line first.

### Changing the dials

These keys set the dials in the header for every agent you start next:

| Key | Dial |
| --- | --- |
| `alt+a` | vendor |
| `alt+m` | model |
| `alt+e` | effort, where the vendor has one |
| `alt+w` | worktree on or off |
| `shift+tab` | permission mode |

### Words for one agent

Words at the front of the task line change one spawn only:

| Word | Effect | Same as |
| --- | --- | --- |
| `agent:pi` | Run this vendor. | `--agent pi` |
| `m:opus` | Model. | `--model opus` |
| `p:plan` | Permission mode. | `--permission plan` |
| `e:high` | Effort. | `--effort high` |
| `w:on` `w:off` | Worktree or not. | `--no-worktree` for off |
| `w:changes` | New worktree, and move your uncommitted work into it. | `--with-changes` |
| `d:/srv/app` | Run in this directory. Relative names resolve against the view's directory. | `--dir` |
| `b:main` | Cut the worktree from this ref. | `--base main` |
| `pr:412` | Start on this pull request. | `--pr 412` |
| `on:spike` | Start on this existing branch. | `--branch spike` |

```
m:opus w:off port the importer
```

The same combinations `amx new` refuses are refused here: `pr:` with `b:`,
`w:off` or `w:changes`; `on:` with `b:`, `pr:` or `w:off`. Words further along
the line are part of the task.

### Commands as rows

A line starting with `!` runs a shell command instead of an agent, like
`amx new --exec`:

```
!cargo test --all
```

The rule over the line reads `COMMAND`. The command runs in the same directory
a task would, with no worktree and no dials. See
[Agents](agents.md#commands-as-rows).

### Completions

A word starting with `/` offers the vendor's skills and commands. On a codex
line the mark is `$` instead, since codex runs skills as `$name`. A word
starting with `@` offers the vendor's agents, then files and directories.
`agent:`, `m:`, `p:`, `e:`, `w:` and `d:` offer their values.

| Key | Does |
| --- | --- |
| `↑` `↓` | Move through the offers. |
| `tab` | Take the highlighted offer. On an empty line, open the `@` offers. |
| `enter` | Take the offer while the word is incomplete; start the agent once it is complete. |
| `esc` | Close the offers, keep the line. |

`@name` at the very front of the line, naming one of the vendor's agents,
starts the session as that agent (`--agent name` for claude). pi has no agents,
so `@` on a pi line is always a file.

### Editing keys

The line edits like a shell prompt:

| Key | Does |
| --- | --- |
| `←` `→` | A character. |
| `ctrl+←` `ctrl+→` | A word. |
| `home` `end` `ctrl+a` `ctrl+e` | Start and end of the line. |
| `backspace` `delete` | The character before or under the cursor. |
| `ctrl+w` `alt+backspace` | The word before the cursor. |
| `shift+enter` | A newline. Needs a terminal that reports it; see below. |
| `alt+enter` `ctrl+j` | A newline, on any terminal. |
| `alt+↑` `alt+↓` | Earlier tasks or replies. On a task line, `↑` `↓` work too. |

The last 50 tasks and 50 replies are kept in `~/.local/state/amx/view.json`.

A paste longer than 800 characters or three lines folds into
`[Pasted text #1]`. The whole paste is still sent. Backspace removes the
marker and the paste together; pasting the same text again unfolds it.

For `shift+enter` to arrive through tmux, turn on extended keys in your tmux
config (`assets/tmux.conf` in the repository has the lines):

```tmux
set -g extended-keys on
set -g extended-keys-format csi-u
```

## Finding and narrowing

`/` opens the find line. It narrows the list as you type, matching a name, a
piece of a task, or `#12` for a pull request number. `enter` keeps the
narrowing and closes the line; `esc` clears it.

A find line made only of `s:` words narrows by state:

```
s:waiting
s:waiting s:working
s:failed
```

`s:` takes the header's words (`pinned`, `review`, `waiting`, `working`,
`done`, `asleep`) and the record's states (`idle`, `failed`, `stopped`, and so
on).

## Arranging

| Key | Does |
| --- | --- |
| `ctrl+t` | Pin the agent at the top of the list. Again to unpin. |
| `z` | Put it to sleep at the bottom. Again to wake it. |
| `shift+↑` `shift+↓` | Move it within its group. |
| `ctrl+r` | Rename it on the wall, like `amx rename`. |

Pinning a sleeping agent wakes it, and sleeping a pinned one unpins it. Once
you reorder a group by hand, new agents join its bottom.

The arrangement is saved to `~/.local/state/amx/view.json` as you go. The next
view opens on it, and another view open at the same time picks it up within a
second.

## Going into an agent

`enter` (or `→` or `l`) puts the agent's session in front of you:

- Inside tmux, your client switches to the agent's session. The view keeps
  running in its own session.
- Outside tmux, the view hands the terminal to a tmux client and takes it
  back when you detach.

`ctrl+z` inside an agent's session brings you back. amx binds it on the tmux
server only for sessions named `amx-*`, so it keeps its usual meaning
elsewhere.

If the agent's pane has gone (parked, stopped, or lost with its tmux server),
`enter` resumes it on its recorded session first. An agent with nothing to
resume is refused with the reason.

Other ways to move:

| Key | Goes to |
| --- | --- |
| `alt+1` .. `alt+9` | The agent at that position on the wall. |
| `w` | The first agent waiting on a question, else the first ready for review, else the most recently finished. |
| `backspace` | The agent you were last in. |
| `gg` `G` | Top and bottom of the list. |

## Stopping and clearing

`ctrl+x` on a row stops a running agent (its pane goes, nothing else) and arms
the row: it reads `ctrl+x again forgets` for about five seconds. A second press
in that window forgets the agent: its record goes, and its worktree too if the
tree holds no uncommitted work. The branch stays. On an agent that has already
ended, the first press only arms.

`ctrl+x` on a heading arms every row under it without stopping anything. The
second press stops and forgets the whole group.

`c` marks every finished agent on the wall, each row saying why it is on the
list. A row whose worktree holds uncommitted work says so and will be kept.
A second `c` within five seconds clears them, the same as `amx clear`: agents
whose work has landed lose their branch too, the rest keep it.

While the view is open it runs `git fetch --prune` in the background, once
every five minutes per repository, so branches deleted on the forge show up.

`i` interrupts the agent's current turn, like `amx interrupt`. `f` starts a
fork of the agent on a task you type, like `amx fork`. `o` opens the row's pull
request in your browser.

## The keys screen

`?` replaces the list with every key, in five groups: walk, look, start,
arrange, dials. Keys you bound yourself in `[keys]` are listed under `yours`.
Scroll it with the list's keys. `/` narrows it by key or description, `esc`
returns to the list, `q` closes the view.

## Mouse

- Click a row to attach, like `enter`.
- Click a heading to open or shut it, or the `… N more` line to unfold it.
- The wheel scrolls the list, or the card when the pointer is over it.
- `ctrl+x` acts on the row under the pointer.
- Hold shift to select text with your terminal as usual.

## All keys

| Key | Does |
| --- | --- |
| `↑` `↓` `j` `k` | Move between agents. |
| `gg` `G` | Top, bottom. |
| `alt+1..9` | The agent at that position. |
| `w` | The first agent that needs you. |
| `backspace` | The agent you were last in. |
| `esc` | Close the card, leave a line. |
| `?` | The keys screen. |
| `q` `ctrl+c` | Close the view. |
| `space` | Open the card. |
| `v` | Show vendor, model and effort per row. |
| `enter` `→` `l` | Go into the agent; open or shut a group. |
| `d` | The diff card. |
| `o` | Open the pull request in the browser. |
| `alt+d` | The patch in your `diff` viewer. |
| `pgup` `ctrl+b` `pgdn` `ctrl+f` | Page the card. |
| `ctrl+u` `ctrl+d` | Half-page the card. |
| `ctrl+n` `ctrl+p` | Next and previous hunk of the patch. |
| `n` | Start an agent. |
| `alt+n` | Start it and go into it. |
| `f` | Fork the agent onto a task. |
| `!` | At the front of the task line: run a command. |
| `tab` | Take an offer; on an empty line, open `@` offers. |
| `ctrl+g` | Write the line in `$EDITOR`. |
| `alt+↑` `alt+↓` | Earlier lines. |
| `ctrl+s` | Group by state, directory, state, repository. |
| `ctrl+t` | Pin or unpin. |
| `z` | Sleep or wake. |
| `shift+↑` `shift+↓` | Move within the group. |
| `ctrl+r` | Rename. |
| `i` | Interrupt the turn. |
| `ctrl+x` | Stop; again to forget. On a heading, the group. |
| `c` | Mark finished agents; again to clear them. |
| `/` | Find. |
| `alt+a` `alt+m` `alt+e` `alt+w` `shift+tab` | Next agent's vendor, model, effort, worktree, permission. |
