# Configuration reference

Everything amx reads out of `config.toml`, what reads it, and when a change
takes effect. Every section and key below exists in
`amx_core::config::SECTIONS` (`crates/amx-core/src/config/mod.rs:58-65`) and is
read by code this document names; nothing is documented ahead of its reader
([11-m4-plan.md](11-m4-plan.md) D-M4-10). What amx does *not* let you configure
is [§11](#11-what-is-not-configurable), written down rather than left for you to
discover.

The phone profile D14 exists for is [`examples/keys-phone.toml`](../examples/keys-phone.toml),
explained in [§10](#10-the-phone-profile).

---

## 1. Where the file is

`$XDG_CONFIG_HOME/amx/config.toml`, or `$HOME/.config/amx/config.toml` when
`XDG_CONFIG_HOME` is unset (`crates/amx-core/src/ctx.rs:220,327-341`). The path
must be absolute; a relative `XDG_CONFIG_HOME` is refused rather than resolved
against the current directory (`ctx.rs:335-339`).

**One file per user, not per session.** It carries no session name, because a
session is a runtime instance and configuration outlives all of them
(`ctx.rs:194-199`). Every server this user runs reads the same file and reloads
it on its own.

**There is no file to create.** A missing file says exactly what an empty one
says — every section absent, so every section at its defaults — which is why a
fresh machine needs no configuration at all
(`crates/amx-server/src/config_rt.rs:212-224`, and the same equivalence in the
client at `crates/amx-client/src/config/mod.rs:94-96`).

---

## 2. How the file is read

Reload is two-tier and lenient (D-M1-8), and the leniency is structural rather
than a caller convention: `config::reload` takes the *running* configuration and
returns the next one, so "keep what was running" is what a failure does
(`crates/amx-core/src/config/mod.rs:8-23,303`).

| What happened | What amx does |
|---|---|
| The file is not valid TOML | Keeps the **entire** running configuration and files one diagnostic |
| One section does not deserialize | Keeps **that section's** running values; every other section still applies |
| A section is absent | That section resets to its defaults, so the running configuration is always reconstructable from the file |
| An unknown key, or an unknown section | Ignored silently — the same tolerance the wire applies in both directions (04 §4) |

A typo while editing therefore costs you the thing you were editing, never a
live session.

### When a change takes effect

There is no single answer, and the differences are deliberate. Nothing a reload
does ever restarts a running process (`config_rt.rs:19-21`).

| Section | Read by | When |
|---|---|---|
| `[persist]` | the `Persist` actor | per save, and immediately on reload for the off direction — see [§3](#3-persist) |
| `[terminal]` | `Core`, when it spawns a pane | per spawn: the next pane, never a running one |
| `[update]` | `amx update` | per invocation |
| `[work]` | `amx work` | per invocation |
| `[client]` | `amx attach` | at attach: a change reaches the **next** attach, not the client you are looking at |
| `[keys]` | `amx attach` and `amx keys` | the same — `amx keys` re-reads the file every time you run it |

The server half is hot because the server watches the file: an `inotify` watch
on Linux and a `kqueue` vnode filter on darwin, over the containing *directory*
so an editor's write-and-rename is not missed, coalesced behind a 500 ms quiet
window so one save is one reload (`crates/amx-server/src/platform/watch/mod.rs:1-24,49-55`).
Each reload publishes `Event::ConfigReloaded` carrying the number of sections
that kept their running values (`config_rt.rs:129-141`).

The client half is not hot. A client reads the file once, before it touches the
terminal, so that anything it could refuse it refuses while a person can still
read the refusal (`crates/amx/src/cmd/attach.rs:191-206`). Re-attach to pick up
an edit.

---

## 3. `[persist]`

| Key | Type | Default |
|---|---|---|
| `history` | boolean | `false` |

Whether per-pane scrollback sidecars are written beside the session snapshot
(D-M1-6). Off by default because scrollback holds secrets (04 §6).

```toml
[persist]
history = true
```

The toggle is hot in both directions and **asymmetric on purpose**
(`crates/amx-server/src/actor/persist/actor.rs:265-285`). Turning it on arms the
debounce, so the dump rides the next ordinary save: reading a full pane's
scrollback costs parser time and no config edit should buy itself a burst of it.
Turning it off acts immediately and wipes `history/`, because what is on disk is
your scrollback and a session's worth of secrets does not get to sit there until
the next structural change comes along.

---

## 4. `[terminal]`

| Key | Type | Default |
|---|---|---|
| `shell` | string | unset — see below |

The shell a pane with no command of its own runs.

```toml
[terminal]
shell = "/usr/bin/fish"
```

Three sources in order: `[terminal] shell`, then `$SHELL`, then `/bin/sh`
(`crates/amx-server/src/config_rt.rs:189-210`). Configuration wins because it is
the one of the three you stated on purpose. An empty value counts as unset at
either of the first two sources: `shell = ""` is a half-finished edit, not a
request to exec the empty string.

Consulted per spawn, so an edit reaches the next pane without a restart. Panes
already running keep the process they started with.

---

## 5. `[update]`

| Key | Type | Default |
|---|---|---|
| `channel` | string (URL) | the releases of the repository this binary was built from |

Where `amx update` looks for a newer amx (D-M3-8).

```toml
[update]
channel = "https://mirror.example.com/amx/latest.json"
```

An override rather than a value: the default channel is a fact about the binary,
so it lives beside the code that fetches it
(`crates/amx/src/update/manifest.rs:32-41`) rather than in a file every user
would have to write out to get the ordinary behaviour. What belongs here is the
*other* answer — a private mirror, an air-gapped path, a `file://` URL.

`https://` and `file://` only. Cleartext `http://` is refused with its reason: a
manifest carries the checksums that make a download trustworthy
(`crates/amx/src/update/fetch.rs:108-119`). The `--channel` flag beats the file,
and the file beats the built-in default (`crates/amx/src/cmd/update.rs:322-345`).

**No release has published this asset yet** and no pipeline exists to publish
one, so the built-in channel answers 404 today and `amx update check` says so
plainly rather than treating it as an error (`manifest.rs:34-40`).

---

## 6. `[work]`

| Key | Type | Default |
|---|---|---|
| `dir` | string (template) | `{repo_parent}/{repo_name}--{branch}` |

Where `amx work <branch>` puts the worktree it adds (D-M3-10).

```toml
[work]
dir = "{repo_parent}/trees/{repo_name}/{branch}"
```

The substitutions are `{repo_parent}`, `{repo_name}` and `{branch}`, and nothing
else: a `{token}` amx does not know is a typo in someone's configuration, and
quietly making a directory named after it would be worse than saying so
(`crates/amx/src/work.rs:57-101`). A rendered path that is not absolute, or that
walks upwards through `..`, is refused for the same reason.

The default puts the tree beside the repository rather than inside it, so
repo-internal tooling — a test that walks the tree, a linter with a glob, a build
that hashes every file — never trips over a second checkout (`work.rs:27-35`).
The `--` separator is unambiguous because git refuses `a..b` as a ref.

Read per invocation, and **read twice**: `amx work done` recomputes the template
to check that the tree it is about to remove is the one this template derives,
which is the pin that keeps a destructive verb off a path you typed
(`work.rs:1-19`, `crates/amx/src/cmd/work.rs:274-291`). Editing the template
therefore does not move a tree that already exists — the path a workspace's
worktree actually sits at is remembered on the workspace — but it does change
where the next `amx work` puts one, and it changes what `amx work done` will
agree to remove.

---

## 7. `[client]`

The first section a *client* reads rather than the server
(`crates/amx-core/src/config/mod.rs:159-166`).

| Key | Type | Default |
|---|---|---|
| `narrow_cols` | integer (columns) | `60` |
| `mouse` | boolean | `false` |

```toml
[client]
narrow_cols = 60
mouse = false
```

### `narrow_cols` — the narrow threshold

Below this width the client stops tiling and shows one pane, filling the content
area with no border at all (D14). Above it, nothing changes.

The threshold is a **client projection**, not a layout mutation: crossing it in
either direction closes no pane and splits nothing, and the server's layout tree
is identical on both sides. What does change is the declaration the client
sends — a narrow client declares the one pane it is drawing, and the server sizes
that pane to the whole content area rather than to its slot in a layout nobody
is drawing (`crates/amx-client/src/app/narrow.rs:71-95`, and
[10-attention-surfaces.md](10-attention-surfaces.md) §D14's amendment).

Two cases where a narrow terminal still tiles, both deliberate: a workspace
holding a single pane is already what the tiled projection draws, and the rule
only fires where the client has said, by leaving panes out, that it is not
tiling (`narrow.rs:86-92`).

`narrow_cols = 0` disables the policy — the comparison is `cols < narrow_cols`
(`crates/amx-client/src/config/mod.rs:75-81`) — which is also why the default
lives in the type rather than in a bare integer: "the settings nobody
configured" must not mean a threshold of zero.

### `mouse` — asking the host terminal for mouse reports

Off by default. With it on, the client asks its host terminal for SGR mouse
reporting on the way in (`\e[?1006h\e[?1000h`) and releases it on the way out,
through the same guard that restores the alternate screen — normal exit, panic
unwind and `SIGTERM` alike (`crates/amx-client/src/term.rs:40-64,204,229`).

What arrives is then decided in one place
(`crates/amx-client/src/app/actions.rs:200-244`):

1. **The pane's program asked for SGR reports** — the report is relayed to it,
   with the coordinates moved into that pane's own frame. Which pane it goes to
   was decided by focus long before any number was read.
2. **The pane's program asked, but not for SGR** — the report is dropped. It is
   expecting a different encoding and SGR bytes would be noise to it.
3. **The pane's program asked for nothing** — D14's one exception: a wheel-up
   opens copy mode over that pane's cached scrollback and scrolls it three rows
   per click (`crates/amx-client/src/copy.rs:43-48`); a wheel-down at the live
   edge closes it again. Every other report — click, drag, release, sideways
   wheel — is dropped.

No position is ever interpreted as chrome input: nothing on screen means
anything to a pointer, and the wheel path reads the button and stops before the
first coordinate (03 §Design principles 1, and the fence in
[notes/m4-mouse-path.md](notes/m4-mouse-path.md) §4).

**Why the default is off here and on in the phone profile.** Asking for mouse
reporting costs you your own terminal's selection: while an application holds
the mouse, ordinary drag-select becomes shift-drag — foot's
`selection-override-modifiers` and alacritty's `Shift` suppression, both quoted
from their own manuals in [notes/m4-mouse-path.md](notes/m4-mouse-path.md) §5.
And the cost is paid all the time rather than where it buys something: the wheel
exception exists for panes that did *not* ask for the mouse, so the request
cannot be scoped to the panes that did — which is exactly what a multiplexer
mirroring a pane's own request can do and this cannot.

What it buys is also smaller than it looks. DEC mode `1007` (alternate scroll)
is set by default in both emulators the spike measured, and amx runs on the
alternate screen, so a wheel turn over `amx attach` already produces cursor
keys today. Observed, in both directions, on 2026-08-09 (`m4-mouse-path.md`
§7.3): with amx asking for nothing, foot 1.27.0 sends CSI `\e[A`/`\e[B` and
alacritty 0.17.0 sends SS3 `\eOA`/`\eOB` — so what a pane receives from a wheel
today depends on which emulator you happen to run. With `?1006h ?1000h` set,
both emulators report wheel-up as button `64` and wheel-down as `65`, in the
grammar amx already recognises. The setting therefore buys an **unambiguous**
wheel, not a working one, and it costs a selection to do it.

On a phone there is no drag-select to lose and no other way to scroll: a
touch-scroll gesture arrives as a wheel event and nothing else. That asymmetry
is the whole reason the option exists — the people who need it are exactly the
people not paying its price — and it is why the shipped default is off and
[`examples/keys-phone.toml`](../examples/keys-phone.toml) turns it on
(`crates/amx-core/src/config/mod.rs:190-212` carries the same reasoning where
the default is set).

---

## 8. `[keys]`

The prefix key and the prefix layer's table, as data (D-M4-8). 04 §7 promised
configurable, introspectable keybindings; before M4 the prefix was a constant
and the table was a `match` on byte literals.

| Key | Type | Default |
|---|---|---|
| `prefix` | key name | `ctrl+a` |
| `bind` | table of key name → action name | the shipped table below |

```toml
[keys]
prefix = "`"

[keys.bind]
"n" = "attention-here"
```

Both halves are overrides, and both are per row: a `bind` entry replaces exactly
one row of the shipped table and leaves the rest standing, so a profile that
rebinds the prefix and nothing else is two lines
(`crates/amx-client/src/config/keys.rs:246-309`).

### The shipped table

What `amx keys` prints on a machine with no `[keys]` section:

```
prefix ctrl+a, from shipped

key     action            source
ctrl+a  literal           prefix escape
A       attention-here    shipped
a       next-attention    shipped
d       detach            shipped
g       agents            shipped
p       picker            shipped
v       split-vertical    shipped
w       navigate          shipped
x       split-horizontal  shipped
z       zoom              shipped
```

### The actions

The names a `bind` row may name, in the order `amx keys` lists them
(`crates/amx-client/src/config/keys.rs:24-98`):

| Action | What the key does |
|---|---|
| `literal` | Forward the key to the pane uninterpreted |
| `navigate` | Enter the sticky navigate layer |
| `split-horizontal` | Split the focused pane left/right |
| `split-vertical` | Split the focused pane top/bottom |
| `zoom` | Toggle zoom on the focused pane |
| `detach` | Detach this client, leaving the session running |
| `picker` | Open the picker |
| `agents` | Open the agents view (D15's board) |
| `next-attention` | Focus the head of the attention queue, wherever it is |
| `attention-here` | Focus the oldest blocked agent in the workspace this client is showing |

A row naming anything else loses that row and lists the ten in its diagnostic.

### How a key is spelled

Combos are spelled the way `pane.send_keys` spells them — `ctrl+a`, `esc`, or a
bare character — so what you learned in one part of the docs works in the other
(`crates/amx-client/src/config/name.rs:1-19,53-107`).

- **Modifiers**: `ctrl` (or `control`) only, and it makes a control byte out of
  `@`–`_`, `a`–`z`, `?` and space. `shift+a` is refused with "spell the
  character it produces, like `A`".
- **Named keys that are one byte**: `enter`/`return`, `tab`, `esc`/`escape`,
  `space`, `backspace` (`name.rs:140-151`).
- **Bare characters**: any printable ASCII, `0x20`–`0x7e`.
- **The plus key**: `+` alone is `+`, and `ctrl++` is Ctrl with `+` — the same
  rule `pane.send_keys` reads combos by.

**A prefix-layer key is one byte, and that is a design constraint rather than an
omission.** The input machine is a byte-stream state machine and deliberately
not a key decoder: it recognises the prefix byte, the mode keys of the layer it
is in and the extent of a mouse report, and passes everything else through as
opaque runs, so encodings amx has never heard of reach the pane intact
(`crates/amx-client/src/input/mod.rs:1-17`, and `name.rs:1-19`). `f1`, `up`, `alt+x`
and `€` are therefore refused **by name, with the reason** and with what to write
instead — not silently dropped. `amx keys` is where you read that.

### The prefix-twice escape

The prefix key is always bound to `literal`: pressing it twice sends it to the
pane like any other byte. It follows the prefix wherever you bind it, and it is
the one row a file cannot take away — a table that could remove the way out would
let a configuration file lock you out of sending the key you moved the prefix
onto, which is exactly the case a phone profile creates
(`crates/amx-client/src/config/keys.rs:203-220`). A `bind` row aimed at the
prefix key is refused with a diagnostic rather than silently losing.

### `amx keys`

Prints the resolved table, the prefix, and where every row came from —
`shipped`, `config.toml`, or `prefix escape`. `--json` prints the same thing
plus the path of the file it read, because "which file did you read" is the
second question anybody asks when a binding did not take
(`crates/amx/src/cmd/keys.rs`).

It talks to no server and needs no session: the bindings are resolved entirely
client-side, so this answers from the config file alone. That is also what makes
it the thing to run when a rebound prefix has left you unable to reach the prefix
layer.

---

## 9. When something does not resolve

`amx keys` is the **only** surface a `[keys]` diagnostic reaches a person
through (`crates/amx/src/cmd/keys.rs:15-20`). An attaching client is a keystroke
away from the alternate screen, so a warning printed there is a warning nobody
sees: `amx attach` logs the diagnostics and runs the shipped bindings, and this
command prints them where they can be read. Exit status is zero either way — a
rejected row is news about a file, not a failed command.

Given this `config.toml`:

```toml
[client]
narrow_cols = "wide"

[keys]
prefix = "f1"

[keys.bind]
"up" = "picker"
"q" = "quit"
```

`amx keys` prints the shipped table, and then:

```
client: invalid type: string "wide", expected u16
in `narrow_cols`

keys: prefix = "f1": `f1` does not reach a client as a single byte (it arrives as an escape sequence), and a prefix-layer key is one byte
keys: bind."q" = "quit": no such action. The actions are literal, navigate, split-horizontal, split-vertical, zoom, detach, picker, agents, next-attention, attention-here
keys: bind."up": `up` does not reach a client as a single byte (it arrives as an escape sequence), and a prefix-layer key is one byte
```

Four things went wrong and four things were lost: the `[client]` section kept its
running values (so the threshold is the default and the mouse stays off), the
prefix stayed `ctrl+a`, and the two bad rows lost themselves. Everything else in
the file would still have applied.

Server-side sections are the same, one level up: each reload logs what it
rejected and publishes the count on the bus (`config_rt.rs:143-186`).

---

## 10. The phone profile

[`examples/keys-phone.toml`](../examples/keys-phone.toml) is a file to copy:

```sh
cp examples/keys-phone.toml ~/.config/amx/config.toml
amx keys
```

It changes two things, and both are settings the defaults get right on a laptop
and wrong on a phone.

**A prefix that needs no modifier.** `ctrl+a` on a phone keyboard is Ctrl on a
modifier strip, then `a` — two taps, in a place your thumb is not. The profile
moves the prefix to `` ` ``: one tap on the symbol layer every soft keyboard has,
and the least missed printable byte in a terminal. Pressing it twice still sends
a literal backtick, so nothing becomes unreachable. `esc` and `ctrl+space` are
the other two spellings worth considering, and both are one byte; `esc` is a
poor prefix if you live in vi keys, because every `Esc` then costs two taps.

**Mouse reporting on.** [§7](#7-client) has the whole argument. In one line: a
touch-scroll gesture arrives as a wheel event and there is no other way to reach
a pane's scrollback with a thumb, and the price the setting charges on a desktop
— your terminal's own drag-select — is not a price a phone pays.

It leaves `narrow_cols` alone, because a phone is always below the default 60
columns and the single-pane projection is what you want there anyway.

The profile resolves clean — `amx keys` reports no rejected rows, the prefix
reads `` ` ``, and both `bind` rows show `config.toml` as their source. **It has
not been tried on a real phone SSH client**: no phone was reachable from the
machine this was written on. The mouse spike behind it has the same gap — its
by-hand procedure asks for a phone client "if one is reachable"
(`m4-mouse-path.md` §7.1) and its dated record names foot 1.27.0 and alacritty
0.17.0 and no phone (§7.3). What is verified is the resolution and the key
grammar, on this machine, against the shipped binary; what is not is the tap
count on any particular terminal app.

---

## 11. What is not configurable

Written down because the alternative is discovering it.

- **Only the prefix layer is data.** The navigate layer's keys — `hjkl` focus,
  `HJKL` resize, `x`/`v` splits, `s`+direction swap, `m` move, `d` close, digits
  jump, `c` copy mode, `Esc` back — are a `match` in the input machine
  (`crates/amx-client/src/input/mod.rs:365-395`), as are copy mode's own keys.
  04 §7 describes them; `[keys]` does not reach them.
- **There is no unbind.** A `bind` row replaces one key's action; it cannot
  remove one. Binding `attention-here` to `n` leaves `A` bound to it as well. To
  free a key, bind it to `literal` — which is not the same thing, since it then
  sends that byte to the pane.
- **`[[keys.command]]` does not exist.** 04 §7 describes key bindings that run
  an arbitrary argv; nothing in the tree reads such a table, so writing one is
  ignored under the unknown-key rule — verified against the shipped binary,
  which resolves the rest of the section and says nothing about it. It is
  unbuilt, not removed.
- **`amx attach --pane` runs neither section.** The single-pane viewport reads no
  configuration at all: its prefix and detach chord are two constants, `ctrl+a`
  and `q` (`crates/amx/src/cmd/detach.rs:11-15`, read by
  `crates/amx/src/cmd/viewport.rs:60,182`), and it does not ask the host terminal
  for mouse reports. So a rebound prefix rebinds `amx attach` and not
  `amx attach --pane`, where `ctrl+a q` still detaches. Small, real, and recorded
  in [notes/m4-wave-outcomes.md](notes/m4-wave-outcomes.md) rather than left to
  be found.
- **Client settings are not hot.** [§2](#2-how-the-file-is-read). Re-attach.
- **Keybindings are client-side by construction.** They are resolved from your
  own `config.toml` by your own client and never travel to the server, which is
  why `amx keys` needs no session. The wire's `client::Keybindings` enum
  (`crates/amx-proto/src/control/client.rs:34-46`) is documentary: its `Server`
  variant would mean "use the session's bindings" and there is no such thing —
  no server-side binding table exists and none is designed. See 04 §7.
- **Themes are not configurable.** 04 §3 gives the client theme authority and
  the six-colour theme is roadmap work (05 §M4), not a section this file has.

---

## 12. The other file in this directory

`~/.config/amx/agents.toml` — the agent registry override, derived from this
file's path so the two always live in the same directory
(`crates/amx-server/src/agent/registry/mod.rs:63,180-190`). It is a different
document with a different merge rule: a stanza whose `id` matches a builtin
**replaces that agent whole**, keeping its position, and anything else appends;
half a stanza from the file and half from the builtin would pair a resume
template with an executable list it was never measured against
(`registry/mod.rs:163-177`). A rejected stanza leaves the builtin exactly as it
was, which is the same leniency `config.toml` gets, one document over.

04 §5 is its design; it is out of scope for this reference.
