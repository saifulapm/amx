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

## X05 — ShortNumbers

**A shipped test asserted the stand-in's behaviour, and was rewritten.**
`a_new_workspace_after_a_restore_takes_the_next_free_short`
(`crates/amx-server/tests/restore.rs`) asserted that a create after restoring
workspace `4` and pane `9` answers `5` and `10` — which is what a monotonic
counter does and not what 04 §6 specifies. It is now
`a_new_workspace_after_a_restore_takes_the_lowest_free_short` and asserts
`1, 2, 3, 5`: the holes below the restored number, then past it. The property
the old test was defending — a create must not collide with a restored number —
is still asserted, and is the reason the fourth create skips `4`.

**A collision the stand-in could produce, found while porting restore.** A
layout naming a pane the snapshot has no row for was given a number *during*
the rebuild, so a pane whose row came later in the file could adopt the same
number and two panes then held one. Restore now settles every number in one
pass after the tree is final (`Core::restore_shorts`), recorded numbers before
leftovers. Test: `a_pane_the_snapshot_has_no_row_for_takes_no_other_panes_number`.

**Release is swept at assignment, not at the close.** A short number is held
exactly while its object is in `Core::state`, and the sweep runs immediately
before an assignment (`Core::release_departed_shorts`, called from the two
`next_*_short` helpers). Deferring it to the end of the batch — the other
obvious place — makes what the next number is depend on which batch a close
landed in, which is a mapping nobody can learn. **The close, kill and prune
sites in `actor/core/{pane,workspace}.rs` are deliberately untouched**: they are
outside X05's file scope, and with the sweep they need no edit. A later task
adding a way for an object to leave the tree needs to add nothing either.

**Two helper names still read `next_*`.** `Core::next_workspace_short` and
`next_pane_short` assign the lowest free number now, not the next one; their
call sites are in `actor/core/{pane,workspace}.rs`, which X05 does not own, so
the rename was not made. Whoever next owns those two files can rename them to
`assign_*_short` for free.

**Behaviour change: an all-digits agent name is refused.** `check_new_name`
(`amx-server/src/agent/address.rs`) now rejects `agent start 3` the way it
already rejected a UUID-shaped name, because short numbers resolve before
labels and such a label could never win. Worth a line in X20's docs if agent
naming is documented there.

**Hand-off to X07 — `attach --pane <number>` has no end-to-end test.**
`crates/amx/tests/session_cli.rs` has the tty harness this wants
(`spawn_on_tty`, and `attach_pane_renders_full_screen_with_no_chrome` is the
shape), but `crates/amx/tests/**` is X07's in wave 2, so X05 left it alone. The
parse is covered inline in `cmd/attach.rs`; the resolution rule is covered over
a real socket in `crates/amx-server/tests/short_numbers.rs`. What is uncovered
is the ten lines between them — `resolve_pane` asking `session.state` and
handing a pane id to `viewport::one_pane`.

**Hand-off to X02 (or X00) — the seam ledger can now assert it.** §7's exit
item 8 wants "no `todo!()` in `crates/*/src`"; there are none as of this branch,
and `tests/hygiene.rs` is where a check would live. X05 does not own that file.

**Nothing else diverged.** `amx-core/src/id.rs`,
`amx-server/src/actor/core/{mod,view,restore,import}.rs`,
`amx-server/src/agent/address.rs`, `crates/amx/src/cmd/attach.rs` and server
tests, as §5 says, plus X02's declared hand-off: `actor/core/agents.rs` is
planted empty and declared from `actor/core/mod.rs`, so X02 never touches that
file. The declaration is `mod agents;` and not `pub mod agents;` as §5 spells
it — every other module in `actor/core/` is private and reaches `Core` through
`impl` blocks and `pub(super)` functions, and X10 needs nothing wider.
