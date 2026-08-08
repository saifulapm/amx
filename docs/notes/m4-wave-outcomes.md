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

## X04 — DR-19: the four recorded flakes

**One of the four was already paid, and one clause of a second was.** R-M4-6's
lesson applied to this row, and it applies twice.

`agent_verbs` was diagnosed and fixed in `aba0877` ("wait for the fact, not for
the call that starts it"), whose two named sites are exactly DR-19's "2 in ~12
runs" (`m3-wave-outcomes.md`, "Settling the load-sensitive reads"). That commit
is **not** an ancestor of `18c9261`, the register's re-verification — both
branch from `08a4257` — so the register could not have seen the fix, and
re-measuring at the reproduction that once gave 22/320 found nothing to fix.

The flood threshold is the sharper case. `f09a87c` had already replaced the
*fixed window* with a wait, and it **is** an ancestor of `18c9261` — so the
"fails under 8-way load every time" the register records was verified against a
tree where it no longer did. The fixed *quantity* it recommends replacing was
still there, and that is what X04 changed.

**Two failures the register does not name, in a file X04 owns, same
mechanism.** `flow_control.rs` holds three tests whose evidence is a wall clock
against the machine, not one. Reproduction throughout is the M3 shape —
`taskset -c 0`, eight copies of the binary, eight test threads each — because
repetition on an idle box reproduces none of them:

| Test | Before | What was really being measured |
|---|---|---|
| `two_clients_at_different_speeds_each_stay_consistent` | every round of 4 | a 20 ms-per-frame reader is only slow while the round is faster than the nap; both clients reported byte-identical stats |
| `no_client_grid_is_corrupted_after_coalescing` | 16 runs in 16 | the same reader, at 30 ms |
| `server_memory_is_bounded_under_a_stalled_client` | 1 run in 16, then 8 in 160 | `absorbed > 100` in a three-second window is a publication *rate*; at this load the pane manages ten a second and lands at 95 |

They are in the entry's owned files and share the named defect, so they were
fixed with it rather than left as a suite X04 could not call green. Recorded
here because §5 names only the first.

The third took two passes, and the first pass is the more useful record: moving
the window from three seconds to the suite's patience only made the constant
bigger, and it still failed at 95 publications against 100. What fixed it was
deleting the count — the watch now ends when publications outrun frames on the
wire by the margin the assertion states, which becomes true at whatever speed
the pane publishes and never becomes true if the stream keeps sending.

**The `_hook` race did not reproduce and was fixed anyway.** 18 runs at six
copies on one core produced none, which matches the register's "once in 115
runs". The mechanism is not in doubt — with no environment
`PaneIdentity::from_process` answers `None` before `read_payload` is called
(`crates/amx/src/cmd/hook.rs:112-118`), so the child can exit before the caller
writes — so the write tolerates `BrokenPipe` and a new case forces the race
every time by offering the payload after the process has been reaped.

**Verification, at the load each one was reproduced under.**

| Suite | Constraint | Before | After |
|---|---|---|---|
| `flow_control` | `-c 0`, 8 copies × 8 threads, 160 runs | see the table above | 0/160 |
| `agent_verbs` | `-c 0`, 8 copies × 8 threads, 160 runs | 22/320 at `aba0877`'s measurement | 0/160, unchanged tree |
| `adversarial` | `-c 0-1`, 6 copies × 8 threads | 0/18 (its window was already gone) | 0/24 |
| `hook` | `-c 0`, 6 copies × 8 threads | 0/18 (once in 115 elsewhere) | 0/24 |

The two zeroes in the "before" column are the point of the first paragraph:
neither reproduced on this tree, and both are recorded as verifications rather
than as fixes that were needed.

The entry's own bar — all four green under `nproc`-wide load — is 10 runs of
each suite against twelve spinners on a twelve-core box, 0 failures in 40. The
pinned figures above are the harsher measurement, and they are the ones the
fixes were judged on: `nproc`-wide load never reproduced any of these.
`cargo test --workspace` on this branch is 774 tests over 119 suites, 0 failed.

**Nothing outside the entry's files was touched.** `flow_control/drive.rs` was
considered for the paced-reader helper and left alone; the helper went into
`flow_control/harness.rs`, which the entry lists.

**For X00 — DR-19's row.** Four owners, three code changes, one verification
that found the work already done. The register's `agent_verbs` clause should be
struck as stale rather than as fixed here, with `aba0877` named; the flood
clause is half-stale the same way, with `f09a87c` named for the window and X04
for the quantity.
