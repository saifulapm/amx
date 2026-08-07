---
name: amx
description: Drive the amx terminal multiplexer from inside one of its panes — open panes, run commands in them, start and prompt sibling agents, wait for a pane to block or go idle, and read back what it printed.
---

# Driving amx

amx is a terminal multiplexer whose whole surface is one API: the keys a human
presses, the status line, and everything below are the same control calls this
document describes. There is no separate automation mode to enter.

## First: are you inside amx?

Every process amx starts in a pane has these variables, and nothing else does:

| Variable | What it is |
|---|---|
| `AMX_ENV` | `1`, and only ever `1`. Its presence is the gate. |
| `AMX_SESSION` | The named session this pane belongs to. |
| `AMX_SOCKET` | The absolute path of that session's socket. |
| `AMX_PANE_ID` | This pane's UUID. |
| `AMX_WORKSPACE_ID` | The UUID of the workspace holding it. |
| `AMX_HOOK_TOKEN` | The pane's hook token. Nothing here needs it. |

If `AMX_ENV` is unset you are in an ordinary terminal: none of this works, and
guessing at a session name will either fail or reach somebody else's. Check it
once, and say so plainly rather than trying.

`AMX_SESSION` is read automatically, so commands below need no `--session`
flag. A pane addresses its own session and no other.

## The shape of every command

One verb, one JSON object, one JSON reply on stdout:

```
amx pane read --params '{"target":"<pane>"}'
```

`--params` is always a single object and always optional-if-empty. Replies are
pretty-printed JSON; errors go to stderr with a non-zero exit. Nothing here is
interactive and nothing here waits on a poll interval — the waits below return
the moment their condition holds.

Panes are addressed by UUID or by **label** — the name a pane was given, which
is also what the picker and the status line show. A label must be unique to be
used as an address; when it is not, the error names the candidates.
`$AMX_PANE_ID` is your own pane, which is the one address you always have.

## Look around

```
amx session state --params '{}'
```

The whole session in one reply: every workspace, every pane with its label,
size and history bounds, the bus sequence the snapshot was taken at, and two
fields worth knowing about —

- `attention`: the panes whose agents are waiting on a human, in the order they
  started waiting. This is the same queue the status line counts and
  `amx agent next` walks; there is no second way to ask.
- each pane's `agent`: `kind` (which agent, once identified), `state`
  (`idle`, `working`, `blocked`, or `busy`/`quiet` for a program amx does not
  recognise), and `transition_seq`, the bus sequence that state was entered at.

## Open a pane and run something in it

```
amx pane split --params "{\"pane\":\"$AMX_PANE_ID\",\"direction\":\"vertical\"}"
amx pane run   --params '{"target":"<pane>","text":"cargo test --workspace"}'
amx pane read  --params '{"target":"<pane>","lines":40}'
```

`amx pane split` answers with the new pane's UUID. `direction` is
`vertical` (side by side) or `horizontal` (one above the other), and an
optional `command` array starts something other than the user's shell. Note
the key: a split says `pane` and takes a UUID, because it cuts a pane you
already have; the driving verbs say `target` and take a UUID or a label.

`amx pane run` types text into the pane and submits it, wrapped in bracketed
paste when the program there asked for it — the same path a human's keystrokes
take, so the command lands in shell history and the user can see what you did.
`amx pane send-text` types without submitting, and `amx pane send-keys` sends
key combinations by name (`ctrl+c`, `f1`, `alt+enter`) for programs that want
keys rather than text.

`amx pane read` returns the pane's **visible screen** as text rows, newest at
the bottom, optionally the last `lines` of it. It is the screen, not a
transcript: output that scrolled past between two reads is in the pane's
history, not here.

To wait for something to appear rather than reading until it does:

```
amx pane wait-output --params '{"target":"<pane>","match":"tests passed","timeout_ms":120000}'
```

`match` is a literal substring, `regex` a regular expression; use one. The
reply says whether it matched and carries the whole line that did. This also
watches the screen, so a pattern that flashes past between two repaints can be
missed — for output that must not be missed, have the command write a file.

## Work with sibling agents

Panes running a coding agent are addressed the same way, with verbs that know
what an agent is doing:

```
amx agent start  --params '{"name":"review","kind":"claude"}'
amx agent prompt --params '{"target":"review","text":"Summarise the failing test","wait":"blocked"}'
amx agent next   --params '{}'
```

`amx agent start` opens a pane, launches the agent, names the pane, and
returns when the agent actually owns the terminal and has been seen idle — not
when the process exists. `kind` is a registry id: `claude` or `codex` today.
If the timeout expires first the reply says so and the pane is left running,
so you can look at it.

`amx agent prompt` submits text the way a human would and, with `wait`,
returns when the agent next `blocked`s (it needs an answer) or goes `idle` (it
finished). The wait is anchored after your submission: an agent that was
already blocked when you called does not satisfy it.

`amx agent next` focuses the head of the attention queue and reports how many
are still waiting. Empty queue is an empty reply, not an error.

`amx agent explain` reports what amx thinks the pane's agent is doing and
why — every detection rule, whether it matched, and the evidence. Reach for it
when a status looks wrong, and quote it rather than guessing.

## Wait for a pane

```
amx wait --params '{"until":"idle","target":"review","timeout_ms":600000}'
```

`until` is `blocked`, `idle` or `exited`. The call returns the instant the
condition holds — it is an await on the session's event bus, not a poll — and
the reply says whether it was satisfied or timed out. A condition that is
already true returns immediately. There is no "done" status: an agent that
finished is `idle`.

Give every wait a `timeout_ms`. Without one it waits forever, which in a script
is indistinguishable from a hang.

## Watch everything

```
amx events --json
```

One JSON delivery per line, forever: pane and workspace changes, agent status
transitions, and the attention queue moving. A line whose `delivery` is `gap`
means this consumer fell behind and events were dropped — re-query
`amx session state` and carry on from the sequence it reports. A gap is not an
error, and skipping it is the one way to silently miss a transition.

## Working alongside a human

The person who started this session is watching it. That shapes a few things:

- **Panes are shared, not yours.** Type into a pane the user is working in and
  you are typing over their shoulder. Open your own with `amx pane split`, or
  name one and stay in it.
- **Leave evidence, not side effects.** `amx pane run` puts what you ran in
  the user's shell history where they can read it; a command smuggled in some
  other way is one they cannot audit.
- **Do not close what you did not open.** `amx pane close` is theirs.
- **Blocked means a human is needed.** If a sibling agent blocks on a
  permission prompt, that is a question for the user, not something to answer
  by sending keys.
