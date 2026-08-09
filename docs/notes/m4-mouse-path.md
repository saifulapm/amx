# X01 — the mouse-path spike

The record of [11-m4-plan.md](../11-m4-plan.md) §2: whether any mouse byte can
reach amx at all, what asking for one costs, how a pane's own mouse mode gets
to a client, and what the wheel exception is allowed to parse. X13 is built on
what is written here, so the discipline is stated first and kept throughout:

**every claim below is either an observation with the bytes quoted, or a
hypothesis labelled as one.** The run that would have closed the last gap could
not be made on this machine and the reason is recorded, not glossed;
[§7](#7-the-by-hand-procedure) is the exact procedure that closes it in a real
terminal, and [§5](#5-the-outcome) says which conclusion depends on it.

Everything was run on 2026-08-09, Arch Linux, kernel 7.1.5, under a `niri`
compositor, against `foot 1.27.0` and `alacritty 0.17.0` — the only terminal
emulators installed. `kitty`, `wezterm` and `xterm` are not installed here and
were not tested.

---

## 1. What was built

Three things, all re-runnable:

- `crates/amx-client/src/bin/mouse_probe/` — the probe (`main.rs` runs it,
  `modes.rs` is what it writes to the terminal, `decode.rs` is what it makes of
  what comes back). It asks the host
  terminal for a list of DEC private modes, prints every byte that comes back
  with its hex, its escapes and this spike's reading of it, and resets what it
  set on the way out. `--query` makes it issue DECRQM (`CSI ? Ps $ p`) three
  times — before setting the modes, after, and after resetting them — so a
  terminal states its own defaults in writing. It is the sibling of
  `raw_mode_probe.rs`, which is the precedent for a binary that takes a real
  terminal and reports what it sees.
- `scripts/spike/mouse/` — the harnesses. `probe-on-pty.py` and
  `ptyharness.py` own a pty and play the host terminal; `tmux-relay.py` runs
  tmux on that pty and watches what a shipped multiplexer asks for and relays;
  `terminfo-mouse.sh` reads the terminfo database; `emulator-decrqm.sh`
  interrogates a real emulator without needing a pointer; `virtual-pointer.py`
  and `wheel-in-emulator.sh` turn a wheel in a real emulator through the
  compositor's `zwlr_virtual_pointer_v1`.
- The wheel parse and its fence, implemented and unit-tested inside the probe
  (`wheel_of`, `mouse_probe/decode.rs`), so
  [§4](#4-the-wheel-parse-and-the-fence) is code that runs rather than a
  paragraph.

The probe and the harnesses are the spike's whole footprint. Nothing in
`amx-client/src/input/`, `amx-client/src/term.rs` or the server was changed:
X13 owns those.

---

## 2. What was observed

### 2.1 The terminfo database: what each installed terminal says it does

`scripts/spike/mouse/terminfo-mouse.sh`, verbatim for foot (alacritty, kitty,
wezterm, xterm, xterm-256color and vte-256color are byte-identical in the two
capabilities that matter):

```
foot               present
    kmous=\E[<,
    XM=\E[?1006;1000%?%p1%{1}%=%th%el%;,
    xm=\E[<%i%p3%d;%p1%d;%p2%d;%?%p4%tM%em%;,
```

Two facts fall straight out.

- **`XM` is the canonical enable string**, and it is
  `\E[?1006;1000h` / `\E[?1006;1000l` — one sequence, `1006` first, `1000`
  second, and no `1002`. This is what the terminal's own database entry says
  the terminal wants asked of it.
- **`xm` is the report grammar** — `ESC [ < btn ; col+1 ; row+1 (M|m)` — which
  is exactly what `mouse::scan` recognises
  (`crates/amx-client/src/input/mouse.rs:24,40-56`).

`tmux`, `tmux-256color`, `screen`, `screen-256color` and `linux` carry no `XM`
and only the legacy `kmous=\E[M`.

### 2.2 A real emulator, interrogated: modes, defaults, and one trap

`scripts/spike/mouse/emulator-decrqm.sh`. The probe ran inside a real foot
window and a real alacritty window, asked for `1007,1006,1000,1002`, and used
DECRQM to make each terminal state its own mode table. **foot 1.27.0 and
alacritty 0.17.0 gave byte-identical answers**; the foot transcript, verbatim:

```
-- DECRQM before any mode is set --
[  0.038s]  44B  ...  "\e[?1007;1$y\e[?1006;2$y\e[?1000;2$y\e[?1002;2$y"
             [DECRQM 1007=set][DECRQM 1006=reset][DECRQM 1000=reset][DECRQM 1002=reset]
-- DECRQM after the modes are set --
[  0.302s]  44B  ...  "\e[?1007;1$y\e[?1006;1$y\e[?1000;2$y\e[?1002;1$y"
             [DECRQM 1007=set][DECRQM 1006=set][DECRQM 1000=reset][DECRQM 1002=set]
-- DECRQM after the modes are reset --
[  2.605s]  44B  ...  "\e[?1007;2$y\e[?1006;2$y\e[?1000;2$y\e[?1002;2$y"
             [DECRQM 1007=reset][DECRQM 1006=reset][DECRQM 1000=reset][DECRQM 1002=reset]
```

Three findings, each of them load-bearing for X13.

1. **Both emulators implement the modes and take them.** `1006` and `1002` read
   back as set after being asked for. This is the closest thing to "a report
   can arrive" that a machine with no pointer can establish: the machinery the
   report comes out of is present, is addressable, and answers.

2. **`1000` and `1002` are mutually exclusive and the last one wins.** The
   probe asked for both; the terminal reports `1000` *reset* and `1002` set.
   That is not a bug — a terminal's mouse event mode is one enum, not a set of
   flags, and libghostty-vt spells the same enum out
   (`vendor/libghostty-vt/src/terminal/mouse.zig:7-13`: `none`, `x10` (9),
   `normal` (1000), `button` (1002), `any` (1003), "these are all mutually
   exclusive"). **amx must ask for exactly one of them**, and `XM` says which:
   `1000`.

3. **`1007` — alternate scroll — is already set, on both, before amx asks for
   anything.** foot's manual states the mechanism and the default in as many
   words: "When this mode is enabled, mouse scroll events are translated to
   up/down key events when displaying the alternate screen … Alternate
   scrolling is not used if the application enables native mouse support.
   Default: yes" (`man 5 foot.ini`, `alternate-scroll-mode`). alacritty carries
   the same mode in its private-mode table (`AlternateScroll`, in
   `strings /usr/bin/alacritty`) and reports it set.

   That third one has a consequence the plan did not anticipate: amx already
   enters the alternate screen (`ALT_SCREEN_ENTER = \x1b[?1049h\x1b[?25l`,
   `crates/amx-client/src/term.rs:21`), so **a wheel turned over a running
   `amx attach` today is already producing bytes** — cursor-up and cursor-down
   keys — which the client forwards to the focused pane as ordinary key input.
   "The wheel does nothing" is false. See [§6](#6-findings-to-raise), F-3.

   It also sets a trap. The probe reset `1007` on exit because it had set it,
   and the terminal was left with `1007` *reset* — a state the user never chose
   and that outlives the probe. **X13 must reset only what it actually
   changed**, which means reading the mode before setting it (DECRQM is right
   there) or never touching `1007` at all. The second is simpler and is the
   recommendation.

### 2.3 A shipped multiplexer, watched from the host side

`scripts/spike/mouse/tmux-relay.py` runs tmux on a pty this harness owns, so
the harness sees exactly what a host terminal would. tmux is the same shape as
`amx-client` — a program between a host terminal and a pane's application — and
it has been that shape for twenty years.

**Control: a pane application that never asks for the mouse.**

```
== scenario: control ==
  tracking modes tmux left ON: none
  every mouse-mode write, in order: [(1000,'l'),(1002,'l'),(1003,'l'),(1006,'l'),
    (1006,'l'),(1000,'l'),(1002,'l'),(1003,'l'),(1006,'l'),(1000,'l'),(1002,'l'),(1003,'l')]
```

Twelve writes, all resets. With `mouse off` and nothing inside wanting the
mouse, tmux **never takes the host terminal's mouse**.

**The pane application enables `?1006h ?1000h`.** The trailing pair is tmux
mirroring it upward, and it appears only after the pane asked:

```
  every mouse-mode write, in order: [ … (1006,'l'),(1000,'l'),(1002,'l'),(1003,'l'),
    (1006,'h'),(1000,'h')]
```

With tmux's own `mouse on`, it asks for one mode more — `1002`, for its own
drag-select — and the same `1006`/`1000` pair:

```
  … (1006,'h'),(1000,'h'),(1002,'h') …
```

**What reaches the pane, when the pane fills the screen.** Fed
`\e[<64;10;20M` at the outer terminal, the pane program logged:

```
[  0.403s]  12B  1b 5b 3c 36 34 3b 31 30 3b 32 30 4d  "\e[<64;10;20M"  [sgr-report 12B wheel=up]
```

Byte-identical, with `mouse off` and with `mouse on` alike.

**What reaches the pane when the pane is not at the origin.** Two stacked
panes, the probe in the lower one, the same report fed at viewport row 20:

```
  panes: 0 top=0 left=0 12x80 | 1 top=13 left=0 11x80
  fed at column 10, row 20 of the 80x24 viewport:
    [  0.705s]  11B  1b 5b 3c 36 34 3b 31 30 3b 37 4d  "\e[<64;10;7M"  [sgr-report 11B wheel=up]
```

**Row 20 became row 7.** tmux parsed the coordinate, hit-tested it against its
layout, and rewrote it into the pane's own frame — `20 − 13 = 7`. This is
[§6](#6-findings-to-raise) F-1, and it is the single most consequential thing
this spike found, because 04 §7 and D9 promise reports are "forwarded
unchanged" and `mouse.rs:5-8` says the client "never extracts a button or a
coordinate".

### 2.4 amx's own read path, on a real tty

`scripts/spike/mouse/probe-on-pty.py` drives the probe on a pty with the
harness playing the terminal. What the probe wrote on entry and exit, and what
it made of eleven fed reports:

```
== what the probe wrote to the terminal on entry ==
  modes: [(1006, 'h'), (1000, 'h')]
== what the probe wrote to the terminal on exit ==
  modes: [(1000, 'l'), (1006, 'l')]
  exit status: 0
== restoration ==
  set on entry:  [1000, 1006]
  reset on exit: [1000, 1006]
  balanced: yes
```

```
[  0.000s]  11B  …  "\e[<64;10;5M"   [sgr-report 11B wheel=up]
[  0.151s]  11B  …  "\e[<65;10;5M"   [sgr-report 11B wheel=down]
[  0.302s]  11B  …  "\e[<68;10;5M"   [sgr-report 11B wheel=up]     (shift held)
[  0.454s]  11B  …  "\e[<80;10;5M"   [sgr-report 11B wheel=up]     (ctrl held)
[  0.604s]  11B  …  "\e[<66;10;5M"   [sgr-report 11B]              (horizontal wheel)
[  0.755s]  10B  …  "\e[<0;10;5M"    [sgr-report 10B]              (left press)
[  0.906s]  10B  …  "\e[<0;10;5m"    [sgr-report 10B]              (left release)
[  1.057s]  11B  …  "\e[<32;11;5M"   [sgr-report 11B]              (drag)
[  1.208s]  12B  …  "\e[<128;10;5M"  [sgr-report 12B]              (button 8)
[  1.358s]   1B  …  "a"              [other "a"]
[  1.509s]   3B  …  "\e[A"           [other "\e[A"]
[  1.660s]   7B  …  "\e[<64;1"       [other "\e[<64;1"]            (split, first half)
[  1.761s]   4B  …  "0;5M"           [other "0;5M"]                (split, second half)
```

The split case is the one worth naming: the probe classifies per read and has
no carry, so a report split inside its parameters is not recognised. That is
precisely what `Scan::Partial` and `Input.carry`
(`crates/amx-client/src/input/mouse.rs:31-33`,
`crates/amx-client/src/input/mod.rs:159`) already exist for — X13 reuses them
and does not repeat the probe's shortcut.

### 2.5 The one hop that could not be observed here

A wheel event in a real emulator producing a real report needs a pointer.
`zwlr_virtual_pointer_v1` provides one, and it works: `virtual-pointer.py`
binds `zwlr_virtual_pointer_manager_v1` (advertised by the running compositor
at version 2), creates a pointer, and every request is accepted — each frame is
followed by a `wl_display.sync` round trip, and no protocol error was ever
raised. A real `foot --fullscreen` window mapped, and the probe inside it ran
and announced itself.

**Nothing arrived, and the reason is not the mouse path.** The session is
locked:

```
$ loginctl show-session 4 -p LockedHint -p Active -p State
Active=yes
State=active
LockedHint=yes
```

`hyprlock` holds the lock. A locked session routes every pointer event to the
lock surface and none to an ordinary window, so this run cannot distinguish "the
emulator sent nothing" from "the compositor delivered nothing". It is therefore
recorded as **not observed**, and `wheel-in-emulator.sh` now refuses to run on a
locked session and says why, so a later run cannot mistake the empty result for
a finding:

```
wheel-in-emulator: session 4 is locked (LockedHint=yes).
  A lock surface takes every pointer event, so nothing would reach
  the emulator and the empty result would be indistinguishable from
  "no report was sent". Unlock the session and run this again.
```

[§7](#7-the-by-hand-procedure) closes it in two commands.

---

## 3. Where a pane's own mouse mode is read, and how it reaches a client

§2 question 3. This is answerable from source alone and is answered here in
full.

**The server can read it, per pane, with no new mechanism.**
`Terminal::mode(Mode::dec(n))` (`crates/amx-vt/src/terminal.rs:280`) reads any
mode libghostty-vt knows, and the handoff manifest already reads `1000`,
`1002`, `1003`, `1006` and `1007` through it
(`crates/amx-server/src/handoff/manifest/modes.rs:111-117`, and
`modes.rs:49-57` is the loop). `Terminal::mouse_tracking()`
(`terminal.rs:380`) is a one-call "is any tracking on" and has no caller
anywhere in the tree.

**There is no callback, so it is polled — on the thread that already owns the
terminal.** libghostty-vt reports side effects through
`amx_vt::callbacks::Effect`, whose whole vocabulary is `Bell`, `TitleChanged`,
`PwdChanged` and `ClipboardWrite` (`crates/amx-vt/src/callbacks.rs:88-110`);
no variant announces a mode change. The parser thread must therefore read the
mode after a parse. That costs one FFI getter per parsed chunk, on the thread
holding `&self.terminal`, allocating nothing — the parse loop is
`parser.rs:322-334` and the effect drain beside it is `parser.rs:349-369`,
which is where a `Title` is read the same way today.

**The route to a client is the one `Core` already uses for exactly this
problem.** `PaneReport`'s module doc names it (`actor/core/report.rs:25-28`):

> The history window is the fold worth naming: `session.state` answers
> synchronously, so it cannot ask a pane where its scrollback starts and ends.
> `Core` keeps the pair per pane instead, moving it as commits, invalidations
> and evictions come in.

A pane's mouse mode is the same shape of fact and takes the same four steps:

1. the parser thread notices the mode changed and sends a new `HostEvent`
   variant (`actor/pane_host/mailbox.rs:94`);
2. the pane actor turns it into a `PaneReport` (`pane_host/actor.rs:278` is
   the match, `actor.rs:348` the send);
3. `Core::handle_pane_report` folds it into a per-pane map
   (`actor/core/report.rs:55`) and absorbs no `Effect` and publishes no
   `Event` — the report is a fact, not a transition;
4. `session_state` answers `PaneState.mouse` out of that map
   (`actor/core/view.rs:49`), synchronously, with **no parser round trip on
   the query path**.

That is the answer to question 3, and it costs one enum variant in three
places and one map in `Core`.

**One correction to the field's shape.** `PaneState.mouse` cannot be a boolean.
The pane's application chooses an event mode *and* a report format — the two
enums are `mouse.zig:7-13` (`x10`/`normal`/`button`/`any`) and
`mouse.zig:22-28` (`x10`/`utf8`/`sgr`/`urxvt`/`sgr_pixels`) — and a pane that
asked for `1000` without `1006` expects the X10 encoding, not SGR. Forwarding
an SGR report to it delivers bytes it cannot parse. The field must carry enough
to answer "would this pane understand the bytes I am about to send it", which
is the format at minimum. The honest first cut is: **forward only to panes
whose format is SGR, and drop for the rest**, with the drop recorded rather
than silent. libghostty-vt ships an encoder for the general case
(`ghostty_mouse_encoder_setopt_from_terminal`,
`vendor/libghostty-vt/include/ghostty/vt/mouse.h:33-60`) and it takes a decoded
event with coordinates — which is the fence again, and why the first cut is the
recommendation.

---

## 4. The wheel parse, and the fence

§2 question 4. Written down as code, in `mouse_probe/decode.rs`'s `wheel_of`,
under five of the probe's nine unit tests. The specification it is written
against is in the tree:
`vendor/libghostty-vt/src/input/mouse_encode.zig`.

**The parse.** The report is `ESC [ < btn ; col ; row (M|m)`
(`mouse_encode.zig:151-156`). The wheel exception reads `btn` and stops at the
first `;`. It requires:

- the final byte is `M`. `m` is a release; the wheel does not send one.
- `btn & 0b1100_0000 == 0b0100_0000` — the wheel bank (bit 6), not the bank
  above it. `mouse_encode.zig:219-220` gives `.four => 64` and `.five => 65`;
  `:223-224` gives buttons 8 and 9 as 128 and 129. `buttonCode` itself is
  `mouse_encode.zig:200-239`.
- `btn & 0b0010_0000 == 0` — not a motion report. `mouse_encode.zig:237`:
  motion adds 32.
- then `btn & 0b0000_0011`: `0` is wheel-up, `1` is wheel-down. `2` and `3` are
  the horizontal wheel (66/67) and are recognised as a wheel bank and *not*
  interpreted — a sideways scroll has no meaning in a scrollback.

**Modifiers ride the same field and must not defeat it.**
`mouse_encode.zig:231-233`: shift adds 4, alt 8, ctrl 16. So shift+wheel-up is
68 and ctrl+wheel-up is 80, and an equality test against 64 would miss both.
The masks above catch every one of the 28 modifier combinations; the tests
assert 64, 68, 72, 80, 92 and 93 all decode.

**The fence, asserted rather than described.** The parse stops at the first
`;`, so the column and the row are not read — at all, ever. The test that says
so feeds four reports with the same button and wildly different coordinates
(`1;1`, `999;999`, `0;0`, `;`) and asserts the same answer from each: a parse
that cannot see a coordinate cannot depend on one. This is 03 §1's boundary and
D14's ("any *positional* mouse interpretation … remains out, permanently") in
executable form, and X13 should carry the test with the code.

**Which pane the wheel scrolls is not a coordinate question.** amx's copy mode
is a single `Scrollback` browsed in stable row ids
(`crates/amx-client/src/copy.rs:1-24`), and the client already routes every
forwarded report to the *focused* pane rather than to the pane under the
pointer (`crates/amx-client/src/app/actions.rs:72-87`). So wheel-up scrolls the
focused pane's cache, and no hit-rect appears anywhere. That is the design that
keeps the fence, and X13 should not be tempted off it.

---

## 5. The outcome

**Outcome (b)**: the mechanism is there and the chain is buildable, and asking
for it costs the host terminal's own selection in a way that should not be the
default for a keyboard-only tool. `mouse.enabled` defaults **off**; the phone
profile (X20) is where it is turned on, which is the right place because the
people who need touch-scroll are exactly the people not selecting text with a
mouse.

**The evidence that forced it.**

- The cost is documented by both installed emulators, in their own manuals, and
  it is not small. foot: `selection-override-modifiers` "are used to enable
  selecting text with the mouse irrespective of whether a client application
  currently has the mouse grabbed … Default: Shift" (`man 5 foot.ini`).
  alacritty: "When an application running within Alacritty captures the mouse,
  the `Shift` modifier can be used to suppress mouse reporting"
  (`man 5 alacritty`). So with tracking on, every ordinary drag-select in the
  user's own terminal becomes shift-drag.
- And the cost is paid **all the time**, not just where it buys something. The
  wheel exception exists for panes that did *not* ask for the mouse, so the
  tracking cannot be scoped to panes that did. That is the difference between
  amx and tmux: §2.3 observed tmux holding no tracking at all until a pane
  asked, and mirroring the pane's request when one did. A wheel exception
  cannot do that.
- The thing bought is smaller than D14 assumes, because §2.2 found `1007` set
  by default: the wheel is not dead today, it is ambiguous. Turning tracking on
  trades an ambiguous wheel for an unambiguous one and a lost selection.
- Nothing in the chain looks unbuildable. Both emulators take the modes; the
  grammar in every terminfo entry is the grammar `mouse::scan` recognises; a
  real multiplexer requests the same two modes and relays reports byte for
  byte; amx's own read path handles them on a real tty.

**The hypothesis this rests on, labelled.** *That a wheel event in foot or
alacritty, with `?1006h ?1000h` set, emits `ESC [ < 64 ; col ; row M`* was
**not observed here** — §2.5 says why. It is supported by the terminfo `xm`
grammar, by both emulators accepting and reporting the modes, and by tmux's
users exercising the same path daily; it is not a fact until someone watches it
arrive. [§7](#7-the-by-hand-procedure) is that observation, and it is a
prerequisite on X13, not a nicety:

- if the by-hand run sees the reports, X13 builds outcome (b) as above;
- if it sees nothing, or something materially different, the outcome is **(c)**
  — X13 shrinks to the honest half (§3's chain, so D9's forwarding works for
  the first time) and the wheel exception is deferred with this note as its
  written revisit condition.

**What X13 should request, and how.** `\x1b[?1006h\x1b[?1000h` on entry and
`\x1b[?1000l\x1b[?1006l` on exit — the modes terminfo's `XM` names, `1006`
first, reset in reverse so tracking stops before the encoding it reported in.
Never `1002` (motion traffic amx has no use for, and §2.2 shows it displaces
`1000`). Never `1007`, in either direction: it is already set and it is not
amx's to change. The enter/exit pair belongs beside `ALT_SCREEN_ENTER` /
`ALT_SCREEN_LEAVE` (`crates/amx-client/src/term.rs:21,27`) so it is restored by
the same guard on the same three paths — `Drop`, panic unwind, and the
`SIGTERM` arm's explicit `restore()` — which is what makes "leaving amx leaves
the terminal as it was found" a property of one seam rather than of four call
sites.

---

## 6. Findings to raise

Recorded here, and in `m4-wave-outcomes.md`, rather than fixed: the files they
touch are X03's and X13's.

**F-1 — "forwarded unchanged" cannot be true, and it is not a split-layout
edge case.** 04 §7 and D9 promise SGR reports are forwarded to the pane
unchanged, and `crates/amx-client/src/input/mouse.rs:5-8` says the client
"never extracts a button or a coordinate … it only finds the report's extent so
the bytes can be forwarded verbatim". A report's coordinates are
viewport-absolute; the pane's application reads them as pane-local. §2.3
observed tmux rewriting row 20 to row 7 for a pane at `top=13`. amx's offsets
are never zero even with one pane: the content area is the terminal minus a
status line (`crates/amx-client/src/model/mod.rs:364`,
`crates/amx-server/src/actor/core/view.rs:225`) and every pane is inset one
cell on every side for its border (`view.rs:37-45`). So verbatim forwarding
delivers a coordinate that is wrong by at least one row and one column, always.
Either D9's letter changes or the client translates — and translating for a
multi-pane layout is a hit-test, which is what D14 fences out. The narrow
single-pane projection (X12) is the case where a constant offset suffices.
Raised for X03 (the 04 §7 wording and the `mouse.rs` doc comment) and X13 (what
it actually does).

**F-2 — `PaneState.mouse` is not a boolean.** [§3](#3-where-a-panes-own-mouse-mode-is-read-and-how-it-reaches-a-client)'s
last paragraph. The pane picks an event mode and a report format independently,
and forwarding SGR bytes to a pane that asked for X10 encoding is a wire error
dressed as a feature. X02 froze the field; X13 is its reader and should carry
the format.

**F-3 — the wheel already produces bytes, and the plan's premise that it does
not is wrong.** §2.2: mode `1007` is set by default in both installed
emulators, amx runs on the alternate screen, and so a wheel turn today arrives
as cursor-up/cursor-down and is forwarded to the focused pane as key input.
This does not change the recommendation — arrow keys from a wheel are
indistinguishable from arrow keys from a keyboard, which is exactly the
ambiguity a keyboard-only tool cannot live with — but it does change two
sentences. 10 §D14's "on a desktop it is the concession every trial user
reaches for in the first minute" describes a gap that is partly already filled,
and the honest statement of what the wheel exception buys is *unambiguity*, not
*reachability*. There is also a design option nobody has costed: request
tracking only while copy mode is open, entered by keyboard, which buys
unambiguous wheel scrolling inside copy mode at zero selection cost outside it,
and gives up "wheel-up enters copy mode". Raised, not taken — D14 is 10's, and
X03 owns 10.

**F-4 — restoring a mode you set is not the same as restoring the terminal.**
§2.2's third finding. A client that resets every mode it wrote will clear
`1007`, which the terminal had set before amx started. X13's acceptance says
"leaving amx leaves the host terminal's mouse state exactly as it was found";
meeting it means touching only `1006` and `1000`, or reading each mode with
DECRQM first. The probe demonstrates both the trap and the query.

---

## 7. The by-hand procedure

**What it closes.** [§5](#5-the-outcome)'s one hypothesis: that a wheel event
in a real terminal emitting into a real tty produces an SGR report. Ten
minutes, a terminal, and a mouse.

**Prerequisite.** An *unlocked* graphical session. On a locked session every
pointer event goes to the lock surface and the run produces an empty transcript
that looks exactly like a negative result. Check with:

```sh
loginctl show-session "$(loginctl list-sessions --no-legend | awk '$4=="seat0"{print $1;exit}')" -p LockedHint
```

`LockedHint=no` before starting.

### 7.1 The five-minute version, by hand

Open the terminal under test. Then:

```sh
cd <amx checkout>
AMX_ZIG="$PWD/vendor/toolchain/zig-x86_64-linux-0.15.2/zig" \
  cargo build -p amx-client --bin mouse_probe
./target/debug/mouse_probe --modes 1006,1000 --seconds 30 --query \
  --log /tmp/wheel-$(uname -n).log
```

While it is listening, in this order:

1. **Scroll the wheel up three clicks, then down three clicks.**
2. **Click the left button once**, then the right button once.
3. **Press and drag across the printed text**, release.
4. **Hold Shift and drag across the printed text again**, release.
5. Press `q`.

**What to look for**, line by line in the transcript:

- **The DECRQM block before anything is set.** Record what the terminal says
  its defaults are, `1007` in particular.
- **A wheel-up line.** Success is a line whose classification reads
  `[sgr-report 11B wheel=up]` and whose bytes begin `1b 5b 3c 36 34 3b` —
  `ESC [ < 6 4 ;`. Wheel-down is `36 35` (`65`). If instead you see
  `[other "\e[A"]` / `[other "\e[B"]`, the terminal ignored the tracking
  request and is still doing alternate-scroll: that is **outcome (c)** for this
  terminal and it must be written down as such.
- **The button number under a modifier.** Repeat step 1 with ctrl held. Expect
  `80`/`81`, not `64`/`65` — this is the check that `wheel_of`'s masking is
  needed and correct against a real emitter.
- **The drag at step 3.** Expect press and release reports and *no* selection
  highlight in the terminal. That is the cost, seen: with tracking on, the
  terminal's own selection is gone.
- **The drag at step 4.** Expect a selection highlight and *no* reports. That
  is the escape hatch, seen: shift-drag still selects. Record whether it does,
  because the recommendation in [§5](#5-the-outcome) rests on it.
- **After `q`.** The terminal must behave exactly as it did before the probe
  ran: drag-select without Shift, and wheel-scroll doing whatever it did
  before. If it does not, F-4 has bitten and X13's restore is wrong.

Run the whole thing in **at least two** emulators, and, if one is reachable, in
a phone SSH client (Termius, Blink, Termux) — that is the client D14 exists
for, and its wheel is a touch-scroll gesture, which is the one input this whole
exception is meant to serve.

### 7.2 The scripted version, if the session is unlocked and wlroots-based

```sh
./scripts/spike/mouse/wheel-in-emulator.sh ./target/debug/mouse_probe foot
./scripts/spike/mouse/wheel-in-emulator.sh ./target/debug/mouse_probe alacritty
```

It runs the probe twice per emulator — once asking for nothing, which is what
`amx attach` does today, and once asking for `1006,1000` — and prints both
transcripts. The difference between the two runs *is* the finding. It exits 3
with an explanation if the session is locked, and 2 if the emulator is missing
or there is no Wayland session.

### 7.3 Where the answer goes

Append the transcripts to this section under a dated heading, name the
emulators and their versions, and then either confirm outcome (b) or record
outcome (c) with the bytes that forced it. X13 does not merge before that
heading exists.

#### 2026-08-09 — the wheel was turned, and outcome (b) is confirmed

The session that was locked when §2.5 was written is unlocked, so
`wheel-in-emulator.sh` ran as designed: a `zwlr_virtual_pointer_v1` pointer
turning a real wheel in a real emulator, with the probe reading the tty.
`foot 1.27.0` and `alacritty 0.17.0`, two runs each — baseline (the probe asks
for nothing, as `amx attach` does today) and sgr (`?1006h ?1000h`).

**foot 1.27.0**, verbatim:

```
== baseline: modes requested "" ==
   [  0.514s]   9B  1b 5b 41 1b 5b 41 1b 5b 41  "\e[A\e[A\e[A"  [other "\e[A"]…
   [  0.998s]   9B  1b 5b 42 1b 5b 42 1b 5b 42  "\e[B\e[B\e[B"  [other "\e[B"]…
== sgr: modes requested "1006,1000" ==
   [  0.522s]  13B  1b 5b 3c 36 34 3b 31 39 30 3b 33 34 4d  "\e[<64;190;34M"  [sgr-report 13B wheel=up]
   [  1.005s]  13B  1b 5b 3c 36 35 3b 31 39 31 3b 33 34 4d  "\e[<65;191;34M"  [sgr-report 13B wheel=down]
   [  1.491s]  12B  1b 5b 3c 30 3b 31 39 31 3b 33 34 4d   "\e[<0;191;34M"  [sgr-report 12B]
   [  1.612s]  12B  1b 5b 3c 30 3b 31 39 31 3b 33 34 6d   "\e[<0;191;34m"  [sgr-report 12B]
```

**alacritty 0.17.0**, verbatim:

```
== baseline: modes requested "" ==
   [  0.499s]   9B  1b 4f 41 1b 4f 41 1b 4f 41  "\eOA\eOA\eOA"  [other "\eOA"]…
   [  0.982s]   9B  1b 4f 42 1b 4f 42 1b 4f 42  "\eOB\eOB\eOB"  [other "\eOB"]…
== sgr: modes requested "1006,1000" ==
   [  0.496s]  13B  1b 5b 3c 36 34 3b 32 31 32 3b 33 34 4d  "\e[<64;212;34M"  [sgr-report 13B wheel=up]
   [  0.979s]  13B  1b 5b 3c 36 35 3b 32 31 32 3b 33 34 4d  "\e[<65;212;34M"  [sgr-report 13B wheel=down]
```

**Outcome (b) is confirmed, and §5's hypothesis is now an observation.** Wheel
up is button `64`, wheel down is `65`, in the SGR grammar `mouse::scan` already
recognises, from both emulators, at the byte level. The press/release pair at
the end of foot's transcript (`\e[<0;…M` then `\e[<0;…m`) is the button-1 click
the scripted sequence makes, and it is the same grammar — which is what the
button-only parse and its fence in §4 are written against.

**Two things the run adds that §2 could not.**

1. **The baseline confirms `1007` empirically, and the two emulators disagree
   about how.** With amx asking for nothing, a wheel turn already produces
   cursor keys — but foot sends CSI (`\e[A`/`\e[B`) and alacritty sends SS3
   (`\eOA`/`\eOB`), the application-cursor-key form. So today's wheel is not
   merely ambiguous in the abstract: what a pane receives depends on which
   emulator the user runs. That strengthens the case §5 makes for the
   exception buying *unambiguity*, and it is one more reason the interpreted
   path cannot be inferred from what arrives.
2. **`wheel-in-emulator.sh` only ever worked on foot.** It passed
   `--fullscreen`, which alacritty rejects outright, so the alacritty half of
   the spike's own harness had never run. Fixed here — the launcher now spells
   fullscreen and title per emulator — which is the difference between a
   re-runnable harness and one that was only ever run one way.

Method note: the wheel was turned by a virtual pointer, not by a hand. The
emulator cannot distinguish the two (it sees a `wl_pointer` axis event either
way), so this is an observation of the emulator's behaviour and not of a
human's; nothing in the chain under test is downstream of which device
generated the axis event.
