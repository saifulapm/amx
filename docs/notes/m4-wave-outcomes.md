# M4 wave outcomes

Written by each wave task as it lands, so X00 folds facts into
[11-m4-plan.md](../11-m4-plan.md) §6 rather than reconstructing them. Only
divergences and hand-offs belong here; a task that landed exactly as its §5
entry describes writes nothing.

---

## X01 — The mouse-path spike

Full record in [m4-mouse-path.md](m4-mouse-path.md). Four things there need an
owner outside X01's scope.

**Outcome (b), with one hop labelled a hypothesis.** The spike ran headless.
Everything except the last hop was observed: both installed emulators (foot
1.27.0, alacritty 0.17.0) take `?1006h`/`?1000h` and say so over DECRQM, the
terminfo grammar every emulator ships is the grammar `mouse::scan` recognises,
tmux requests exactly those two modes and relays reports byte-identical, and
amx's read path handles them on a real tty. *A wheel turn producing a report*
was not observed: the session is locked (`LockedHint=yes`, `hyprlock`), and a
lock surface takes every pointer event, so the empty result proves nothing. The
note carries a copy-pasteable by-hand procedure (§7) and `wheel-in-emulator.sh`
refuses to run on a locked session rather than producing a false negative.
**X13 does not merge before §7.3's dated heading exists.**

**Hand-off to X03 — 04 §7 and D9's "forwarded unchanged" cannot hold.** A
report's coordinates are viewport-absolute; a pane's application reads them as
pane-local. Observed: tmux rewrote row 20 to row 7 for a pane at `top=13`
(`tmux-relay.py`, `split` scenario). amx's own offsets are never zero even with
a single pane — the content area is the terminal minus a status line
(`crates/amx-client/src/model/mod.rs:364`,
`crates/amx-server/src/actor/core/view.rs:225`) and every pane is inset one
cell for its border (`view.rs:37-45`). So the promise in 04 §7 and the doc
comment at `crates/amx-client/src/input/mouse.rs:5-8` ("forwarded verbatim")
are both false as written, and correcting them is X03's file, not X01's. What
X13 actually does about it is X13's call; the narrow single-pane projection
(X12) is the only case a constant offset covers.

**Hand-off to X03 — 10 §D14 overstates what the wheel exception buys.** DEC
mode `1007` (alternate scroll) is *set by default* in both installed emulators,
observed by DECRQM before amx asked for anything, and amx runs on the alternate
screen. So a wheel turn today already produces cursor-up/cursor-down keys that
the client forwards to the focused pane. The exception buys *unambiguity*, not
*reachability*; 10 §D14's "the concession every trial user reaches for in the
first minute" describes a gap that is partly filled already. One design option
nobody has costed is named in the note (F-3) and left to 10's owner.

**Hand-off to X13 (with a note for X02) — `PaneState.mouse` cannot be a
boolean.** A pane picks an event mode and a report *format* independently
(`vendor/libghostty-vt/src/terminal/mouse.zig:7-13` and `:22-28`), and a pane
that enabled `1000` without `1006` expects the X10 encoding. Forwarding SGR
bytes to it delivers bytes it cannot parse. The field needs the format at
minimum; the honest first cut is to forward only to SGR panes and drop for the
rest, with the drop recorded.

**Note for X13 — restoring a mode you set is not restoring the terminal.** A
client that resets every mode it wrote clears `1007`, which the terminal had
set before amx started, leaving the user's terminal in a state they never
chose. Touch only `1006` and `1000`, in both directions.

**Nothing else diverged.** The scope stayed inside `docs/notes/m4-mouse-path.md`,
`scripts/spike/**` and `crates/amx-client/src/bin/mouse_probe.rs`.
