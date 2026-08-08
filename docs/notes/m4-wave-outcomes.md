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

## X02 — M4 contracts

### Hand-offs, in the order the later waves meet them

**X05 still owes `actor/core/agents.rs` and its `pub mod agents;` line.** §5's
declared hand-off stands unchanged and X02 did not pre-empt it: the module and
its declaration ride X05's commit, because X05 owns `core/mod.rs` whole this
wave. Nothing X02 landed depends on the file existing, and X10 is its filler.

**X10 owes one arm in `actor/core/route.rs`, not in `dispatch/agent/`.** The
plan put the `agent.list` seam stub in `dispatch/agent.rs`; it is one file over
from there, and the reason is worth stating because it changes what X10 edits.
The dispatch arm is *finished* — it routes the call to the `Core` that D-M4-2
puts the answer in — so the whole path (table, decode, mailbox, reply channel)
is exercised from wave 1 and only the answer is owed. The refusal therefore
lives beside the arm that replaces it, in `Core::absorb`. `route.rs` is in no
wave-1 or wave-3 task's file list, so this is a declared sequential edit rather
than a contested one; `tests/hygiene.rs`'s `SEAM_LEDGER` names the file and the
owner.

**X17 reads `NextParams.workspace`, and the flag is already built.** The field
is on the wire *and* on the generated CLI leaf — `amx agent next --workspace
<UUID>` parses today, because `amx-proto`'s flag rows are the only place a
typed flag can live and the coverage test in `crates/amx-proto/tests/flags.rs`
demands a fully-populated fixture for every flagged row. X17's §5 entry says
"`crates/amx/src/cmd/**` for the CLI flag"; there is nothing left to do there.
The handler currently destructures the field and ignores it
(`dispatch/agent/mod.rs`), which is the one line X17 replaces.

**X06 inherits four empty fields on two events, not two.** `Event::Attention*`
grew `workspace`, `name`, `reason` and `since` as flat optional fields rather
than a nested block, because D15 lists `pane` inside the same identity block and
`pane` was already flat. The construction sites in
`actor/agent_hub/commit.rs` and the `AgentSnapshot` in `agent_hub/mod.rs` carry
`None` with a comment naming X06; those are minimum-to-compile edits in X06's
wave-2 files, made sequentially in wave 1.

**X09 inherits two one-line edits in `amx-client/src/app/`.** The declared arm
in `wired.rs`'s exhaustive `Method` match, plus `actions.rs`, whose
`NextParams {}` literal no longer compiles. Both are wave-1 minimums in a
wave-2 file, as §5 anticipated for the first of them.

**X12/X13 inherit `mouse: None` in `core/view.rs`.** One line, with the fold
X13 owes named beside it. `view.rs` is X05's file this wave; the line is the
minimum `PaneState.mouse` costs and could not wait.

### Divergences from §5

**The seam code moved from `-32000` to `-32099`.** M1, M2 and M3 all spelled an
unwired row's refusal `-32000`, which was free while amx had no permanent code
in JSON-RPC's implementation-defined range. It is not free now:
`RpcError::WAIT_ABANDONED` is `-32000` and a client that recognises it *redials
and asks the same question again*, so an unwired row answering that number puts
a caller in a loop. The permanent codes fill the range from the top and the
temporary one takes the bottom. `tests/skew.rs`'s constant moved with it.

**`agent.list` is `--params`-only, with no flag row.** D-M4-11 makes it the
machine surface and `amx agents` the human one, and `flags.rs`'s own law is that
"which verbs got flags" is a product decision named literally rather than
derived. So `amx agent list --params '{"workspace":"…"}'` is the machine
spelling and `amx agents --workspace api` is X16's.

**`workspace` parameters are `WorkspaceId`, not a name-or-id target.** Both
`ListParams.workspace` and `NextParams.workspace` take an id, like every other
`workspace` parameter on the table. A label is resolved by whoever holds
`session.state`, which the CLI already does; a `WorkspaceTarget` would have put
a second resolver in the server for a namespace that has exactly one method of
resolution today. X16's `--workspace api` therefore resolves client-side.

**Four splits, not six, plus two the plan did not name.** §5 lists six budget
splits. `crates/amx/src/cli.rs` (→ `cli/{mod,verbs}.rs`),
`amx-proto/src/control/agent.rs` (→ `agent/{mod,hook,verbs,list}.rs`),
`amx-server/src/agent/fusion/tracker.rs` (→ `tracker/{mod,inputs}.rs`) and
`amx-server/src/actor/pane_host/parser.rs` (→ `parser/{mod,frames}.rs`) landed
as listed. `amx-server/src/dispatch/agent.rs` split into
`dispatch/agent/{mod,waits}.rs` and `amx-server/src/actor/pane_host/mod.rs` into
`pane_host/{mod,config,feed}.rs` — also as listed. Every move is mechanical: no
behaviour changed, and the only edits inside the moved code are visibility
(`fn` → `pub(super) fn`) and imports.

**`amx-core/src/lib.rs` did not gain re-exports, deliberately.** The new types —
`AgentWorkspace`, `EpochMillis`, `ClientConfig`, `KeysConfig`,
`DEFAULT_NARROW_COLS`, and the two section-name constants — are public at their
module paths (`amx_core::agent::…`, `amx_core::config::…`) and every consumer in
this tree reaches them there. `lib.rs` is in no wave-1 task's file list, and a
convenience re-export is not worth an unowned edit. Whoever next owns that file
may add them.

### Two things the next waves should not re-litigate

**`reason` is a `String`, not an enum.** D-M4-3's decision, implemented
literally: the field carries the detector's own identifier — the winning
manifest rule's name, or the hook event's — and nothing translates it. A new
manifest rule is self-describing on the wire the day it is written. The
`method_agent_list` and `method_session_state` goldens both carry
`permission_dialog`, which is the *shipped* rule name from
`crates/amx-server/assets/manifests/claude.toml`, so a golden reader sees a real
value rather than an invented vocabulary.

**`since` is `Option`, and the absence is load-bearing.** A pane whose status
was re-derived rather than observed entering has no entry edge to report, and a
zero would render as 1970. R-M4-4's honest fallback ("since this server started
tracking it") is X06's to choose; the type does not force a lie either way.
