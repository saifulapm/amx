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

---

## X00 — the wave-1 baseline smoke

Full record in [m4-live-smoke.md](m4-live-smoke.md) §1. All six of §6's items
hold. Three facts it measured need an owner outside X00's scope.

**Hand-off to X10, X14 and X16 — D15's table costs 161 ms today.** Assembling
it the only way that currently exists — one `session.state` plus one `pane.read`
per pane — takes 161 ms for 25 agents, against 8 ms for the state read alone
(m4-live-smoke §1.4). That is the number `agent.list` has to beat, and it is
also the arithmetic behind R-M4-7: a surface refreshing per pane at 4 Hz with 25
agents would spend most of a core on it. The three `last_line` strings the run
recorded are the literal values X10's extraction has to reproduce for those
panes.

**Hand-off to X12 — a pane whose slot insets to zero keeps a departed client's
size.** Observed: with a 45-column client holding size authority over a
five-pane workspace, four panes resized to 21/9/4/1 columns and the fifth stayed
at the 47×10 the 200-column client had given it. That is deliberate and
documented — "a pane squeezed out of visible space keeps its last size: a 0x0
PTY starves the process for nothing"
(`crates/amx-server/src/actor/core/view.rs:192-198`) — so it is not a defect to
fix. It is a case D-M4-7's single-pane sizing rule has to answer, because a
viewport declaring one pane leaves every other pane in the layout in exactly
this state, and X12 is where that is decided rather than inherited.

**Hand-off to X16 — a cold restart is announced on stderr and is invisible on
stdout.** `amx events --json` survived a server restart in the run: it redialled,
resubscribed and kept printing, which is the contract `--watch` is meant to
package. But the sequence space begins again at a cold restart, and the relay
says so on *stderr* (`crates/amx/src/cmd/events.rs:144-147`) while stdout goes
straight from seq 1093 to seq 122 with nothing between. It is correctly not a
`gap` — a gap is loss, and nothing was lost — and no change is proposed here.
X16 reads its own stream, so X16 is where the question of whether a full-screen
`--watch` needs more than that gets answered.

**Confirmed rather than diverged, for the two tasks that build on it.** Seam 4's
two halves are as D-M4-7 describes them, checked against source during the run:
the client declares `Viewport { rows, cols, panes }` with every pane of the
focused workspace (`crates/amx-client/src/app/binds.rs:131-147`), and
`handle_viewport` reads `rows` and `cols` and never `panes`
(`crates/amx-server/src/actor/core/view.rs:144-157`). The letterbox that follows
is measured in m4-live-smoke §1.3 — a 21×17 grid centred inside a 98×47 pane
box — rather than argued.

**DR-11's watch has its first entry.** Two `session stop`s, both exit 0, no
`drain-census` file and no census log line either time (m4-live-smoke §1.7).
