# The M3 live smoke: a real upgrade under real Claude Code

[09-m3-plan.md](../09-m3-plan.md) §7 states M3's other exit criterion in one
sentence — "Green CI plus this checklist is the exit — green CI alone is not,
three times proven" — and the count is four now: M2's smoke found two
non-working features behind a green suite, W06's found the export path's real
behaviour, W12's found that `workspace.create`'s `focus` field had no reader at
all, and this one found a client that comes back from a swap showing a screen
that is wrong forever (§4.2).

This is that run.

**Subject.** amx at `59f6ed8` (branch `worktree-w14-integration`, debug build)
handing itself over to **amx 0.1.1** — a `--version`-bumped build of the same
tree — while driving **Claude Code 2.1.226** on Arch Linux, kernel 7.1.5,
x86_64, on 2026-08-08.

**What makes this run different from every CI test of the same path.** The
successor is a *different version*. `tests/handoff_exit.rs`, the M0 skew harness
and W06's and W09's own smokes all hand a session to a build of the same tree,
because only one version of amx exists; here one was made, published through a
`file://` channel manifest, and installed by `amx update apply` over the running
binary. `0.1.0 → 0.1.1` is the first N→N+1 upgrade amx has ever done.

**Isolation**, following [m2-live-smoke.md](m2-live-smoke.md) §10 exactly:

- `XDG_RUNTIME_DIR`, `XDG_STATE_HOME` and `XDG_CONFIG_HOME` point at a scratch
  tree; the session is named `live`; the binary under test is a *copy* under
  that tree, because `amx update apply` replaces the file it is running from and
  a test that ran the build artifact would have installed over it.
- `CLAUDE_CONFIG_DIR` points at a scratch config directory whose
  `.credentials.json` is a **symlink** to the real one — the borrow-by-symlink
  the M2 smoke and the M2 spike both used — with `hasCompletedOnboarding` and a
  trusted project directory seeded so no first-run dialog owns a pane.
- Every `CLAUDE_CODE_*`, `CLAUDECODE`, `CLAUDE_PID`, `CLAUDE_EFFORT` and every
  `AMX_*` marker is unset before the server starts, so the agents under test are
  *top-level* Claude Code sessions rather than children of the harness. M2
  recorded this as a live-harness trap; it is still one.

**Method.** Everything the driver does goes through amx's own control surface —
`agent.start`, `agent.prompt`, `pane.read`, `session.state`, `session.report`,
`session.handoff`, `update apply` — over the real socket, so what is exercised is
the product. Where a person would be looking at a terminal, a real `amx attach`
runs on a real pseudoterminal at 200×50 and its bytes are read back.

---

## 1. Verdict

| # | What was to be proven | Result |
|---|---|---|
| 1 | five real Claude Code sessions, mid-conversation | **holds** — five started, five answered, five distinct conversation ids |
| 2 | a real staged binary and `amx update apply` | **holds** — 0.1.0 → 0.1.1, sha256 verified, installed, handed over, 3.2 s end to end |
| 3 | none die | **holds** — the same five child pids before and after, twice |
| 4 | no visible screen content lost | **holds, with one measured qualification** — a shell pane's grid is byte-identical across the swap; a Claude Code pane comes back with its own UI redrawn (§4.1) |
| 5 | every conversation answers its remembered-word probe | **holds** — five of five, and three of three on the second run |
| 6 | an interrupted upgrade aborts back to a working session | **holds for a pre-commit interruption** — and the literal "mid-restore" window is 24 ms wide on this machine, which no wall-clock kill can hit (§4.3) |
| 7 | one SSH attach from a genuinely different machine | **holds** — a second machine on the LAN, a different architecture and a non-POSIX login shell; the pane answered with a marker this machine cannot produce (§5) |

Two defects found and fixed, one bound measured, one window that cannot be aimed
at by hand. All of them are below.

---

## 2. The upgrade

```
$ amx update check
amx 0.1.0 at …/live/bin/amx
channel file://…/live/latest.json
0.1.1 is available
M3 live smoke: a version-bumped build of the same tree.
asset file://…/staged/amx-0.1.1
run `amx update apply` to install it

$ amx update apply
amx 0.1.0 at …/live/bin/amx
channel file://…/live/latest.json
downloading 0.1.1 for linux-x86_64
sha256 dca55508f92c8d9968d3b91cd99bf21ae25e9eb11f91c129a8ca119dbdd250be verified
installed 0.1.1 at …/live/bin/amx
handoff accepted; waiting for the successor
session live is now served by amx 0.1.1 (pid 2150713)
```

Exit 0, 3.2 s wall clock for the whole verb — fetch, digest, atomic rename over
the running executable, `session.handoff`, and the reconnect-poll that watches
for a *different process* on the same socket.

The swap itself, from the exporter's own log, with six panes in the session:

```
05:58:25.403  the session is frozen and captured   panes=6 successor=0.1.1
05:58:25.426  the session socket is retired; the successor may bind it
05:58:25.427  the session has been handed over; this server is done  acked=true
05:58:25.427  the successor owns the snapshot; not writing a final capture
```

**24 milliseconds frozen, for six panes.** R-M3-11 asks for this number: the
per-pane `sendmsg` design trades herdr's 64-pane cliff for a syscall per pane,
and at this size the trade costs nothing measurable. The exporter exited **0**
afterwards, inside its drain.

`amx ping` against the socket immediately after:

```
{"seq": 220, "server": {"name": "amx-server", "version": "0.1.1"},
 "session": "c53b568d-7ef6-4378-bb6b-b75b75b0cc5a"}
```

The version moved and the `SessionId` did not, which is the whole claim: a
client that reconnects sees the session it was talking to, served by a different
build.

---

## 3. The five agents

```
a1: pane 13084c4f readiness=ready
a2: pane 737f859d readiness=ready
a3: pane 288550b2 readiness=ready
a4: pane 2a0ad369 readiness=ready
a5: pane 96ebe763 readiness=ready

distinct conversation refs: 5
  13084c4f -> 753bba61-874b-428a-b12d-10aa43dcd14d
  737f859d -> d5fb2c10-9fa6-4e49-974c-b2d0c397f0b4
  288550b2 -> 49ec9a45-db2f-46cf-9023-5af64ed5c6fc
  2a0ad369 -> 40aab4ea-e2ab-4616-a4a0-c920662905f4
  96ebe763 -> 9711a11b-604e-4c83-b0cb-2ae42658296a
```

Five separately authenticated conversations, each told a word only it heard, and
each having answered with that word before the swap. Then the upgrade, and:

```
claude child pids before: [2149852, 2149999, 2150130, 2150258, 2150419]
claude child pids after:  [2149852, 2149999, 2150130, 2150258, 2150419]
same children: True
```

Not a count — the same five process identities. Every pane came back with its
own `session_ref` intact, and every one of the five answered its remembered word
when asked again *after* the swap:

```
a1: expected ALPHA,   answered = True
a2: expected BRAVO,   answered = True
a3: expected CHARLIE, answered = True
a4: expected DELTA,   answered = True
a5: expected ECHO,    answered = True
```

That is the roadmap's sentence, met against the real agent: five running agents,
upgraded, none died, and every conversation is still the one it was.

---

## 4. What was found

### 4.1 The measured bound: an agent redraws itself, and a shell does not

§7 asks for "no visible screen content lost". Measured two ways, because the two
answers differ and only one of them is about amx.

**A shell pane is byte-identical across the swap.** Sixty lines printed into a
24-row pane, so the top of the grid is a row that has already scrolled once:

```
before: head=38 floor=0 rows=24  first='LINE-38'  last='sh-5.3$'
after : head=38 floor=0 rows=24  first='LINE-38'  last='sh-5.3$'
identical: True
```

Head and floor unmoved, every row the same. The styled-cell half of that claim
is CI's — `tests/handoff_exit.rs` paints a bold, underlined, 24-bit-coloured
sentinel in five panes and compares the attached client's own cells either side
of the swap — and W04's property tests are the per-cell proof.

**A Claude Code pane comes back with its UI redrawn.** With three agents settled
and quiet for eight seconds, screens captured immediately before and after:

```
a1: identical = False  (1502 vs 1504 characters)
  row 0: before '│                                      │ Tips for getting started              │'
  row 0: after  '╭─── Claude Code v2.1.226 ─────────────────────────────────────────────────────╮'
  row 1: before '│      Welcome back Saiful Islam!      │ Ask Claude to create a new app or cl… │'
  row 1: after  '│                                      │ Tips for getting started              │'
  …
```

Every row after is the row before it, shifted down by one, with the welcome
box's top border — which had scrolled off the exporter's grid — back on screen.
Nothing is missing; the agent repainted its own frame. Claude Code redraws its
whole UI when its terminal announces a size, and the successor's commit resizes
every pane it takes over.

So the honest statement of the fidelity claim is two statements. **amx's own
transfer loses nothing**: a pane whose program does not repaint comes back
cell-for-cell identical, including its scrollback ids. **A pane whose program
repaints on resize will repaint**, and what is on screen afterwards is that
program's answer rather than a copy of the exporter's grid. The conversation
content was intact through every one of these repaints; the remembered-word
probes above are the evidence.

### 4.2 Fixed: a reconnected client that stayed wrong forever

Found by `tests/handoff_exit.rs` rather than by hand, and recorded here because
it is the fourth time a green suite has hidden a non-working path.

W08's resync opened a re-bound grid stream **delta-only** when the generation
the client presented matched the pane's — the reading being that a matching
generation means the client's cells are current. It does not. The generation
moves on resize and reset only, so what agrees is the *geometry*, and the pane
can paint as much as it likes between a client's last applied delta and its
re-bind. Across a live upgrade that window is not a corner case at all: the
successor resumes every pane at the commit, and a reconnecting client lands
after whatever they painted.

Observed, with five agents in the session: the client came back, repainted from
its own cache, and then showed a pane's *pre-swap* screen forever. No error, no
second repaint, nothing for anyone to notice. Fixed by making a resumed bind
repaint — 04 §6's "keyframes for stale grids", where without evidence to the
contrary every grid is stale — with the generation now buying the keyframe's
*reason* rather than its absence, and with the sound version of the optimization
named where it would go (`KeyframeReason::Resumed`).

### 4.3 The interruption, and the window that cannot be hit by hand

§7 asks for "an interrupted upgrade (kill the importer mid-restore) aborts back
to a working session". Half of that is verified by hand and half of it is not
reachable by hand, and the difference is worth stating precisely.

**A pre-commit interruption aborts cleanly, with three real Claude Code sessions
running.** A staged binary that answers `_handoff-caps` and is then killed before
it authenticates:

```
handoff reply: {"accepted": true, "seq": 147}

$ amx ping
{"seq": 154, "server": {"version": "0.1.0"}, "session": "3a3d8c38-…"}

$ amx session report
handoff aborted at quiesce  …/bin/killed-importer
  the handoff token was refused
no losses to report

claude children: [2151807, 2151963, 2152108]   (unchanged)
a1 still answering: True
a2 still answering: True
a3 still answering: True
```

The session went on serving, every pane resumed, and `amx session report` names
the outcome, the stage and the reason — which is a line W14 had to add, because
until now `session report` carried the row and printed "no losses to report".

**"Mid-restore" is 24 ms wide, and a `sleep`-and-`kill` cannot hit it.** A
killer script was run at 50 ms, 300 ms, 600 ms and 1200 ms after the importer
started; every one of them landed *after* the commit, which is not an
interruption of the handoff at all — it is killing the server that now owns the
session, and the recorded outcome is the ordinary one for that: the session is
gone until an `amx` restores it from the snapshot. The stages in between —
manifest, descriptors, restore, retire, ready — are reachable only by a peer
that speaks §3's protocol and then stops, which is what W05 and W06 drive in
`crates/amx-server/tests/handoff_protocol.rs` and `handoff_export.rs` (all seven
rows of §3's crash table), and what `tests/handoff_exit.rs` drives over the real
binary for the two stages a *binary* can fail at.

Recorded rather than papered over: this step's by-hand half is the pre-commit
abort, and the rest is CI's because a human cannot aim at a 24-millisecond
window.

---

## 5. SSH

**Verified: an attach from a genuinely different machine.** A second machine was
made available on 2026-08-08 and the roadmap's "attach to the home machine from
a laptop over SSH" was run end to end. It is §7's third tier, the one that is
explicitly not a CI resource, and it earned that status on the first attempt:
reaching a rendered pane found a bug that made `--remote` unusable against a
whole class of hosts, and neither tier below it could see it.

**The two machines.**

- **Client** — this machine: Arch Linux 7.1.5, x86_64, amx built from the merged
  M3 tree.
- **Host** — a second physical machine on the LAN at `192.168.0.105`: Fedora
  Linux Asahi Remix 44, aarch64 (Apple Silicon), 8 cores, login shell
  `/usr/bin/fish`.

That pair is worth more than "two machines": it is a different **architecture**
and a **non-POSIX login shell**, and both mattered before any pane rendered.

**Seeding refused, correctly and with the reason.** D-M3-9's rule, met against a
live host rather than a fixture:

```
amx: cannot seed saiful@192.168.0.105: it is Linux aarch64 and this machine is
Linux x86_64. The only amx available to install is the one running now, and it
is built for Linux x86_64. Cross-platform seeding needs published binaries to
download, and no release channel exists yet (R-M3-4) — install amx on
saiful@192.168.0.105 the way you installed it here.
```

That is `a_missing_remote_amx_offers_seeding_only_on_matching_uname_and_refuses_cross_platform_with_the_reason`
happening for real, with the `uname` pair read off the far side.

**So the far side's binary was cross-compiled here — the first time amx has been
built for a second architecture.** `crates/amx-vt/build.rs` already had
`aarch64-unknown-linux-gnu` in its `zig_target` map, so the whole of it was a
`zig cc -target aarch64-linux-gnu` wrapper on
`CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER` and then
`cargo build --release --target aarch64-unknown-linux-gnu -p amx`: a working
12 MB aarch64 binary in 1m42s, first attempt. The vendored zig is what made a
first cross-build a non-event.

**The bug this criterion exists to find.** With that binary installed at
`~/.local/bin/amx` on the host and answering `amx --version` → `amx 0.1.0`,
`amx --remote` still said:

```
saiful@192.168.0.105 has no amx on PATH or in ~/.local/bin.
  fish: Missing end to balance this if statement
  if command -v amx >/dev/null 2>&1; then
  ^^
```

`ssh host <command>` hands the command to the remote user's **login shell**, and
W11's bridge script was POSIX `sh` syntax. Against fish it is a parse error, so
the probe never ran and amx reported the opposite of the truth about the user's
own machine — then offered to seed a host that already had it. `--remote` was
unusable against any host whose login shell is fish, csh or tcsh. **Neither
existing tier could see it**: the loopback sshd test runs as the same user on the
same machine under a POSIX shell, and the bridge-as-child tier has no login shell
at all. Fixed on `worktree-remote-login-shell` (merged) by making every command
amx sends over ssh a single simple command — `/bin/sh -c '<script>'` — which fish
executes happily because it is a plain command rather than syntax it must parse.
The audit is in [m3-wave-outcomes.md](m3-wave-outcomes.md)'s "Remote login
shells".

**The attach, after the fix**, driven on a real 100×30 pseudoterminal:

- `amx --remote saiful@192.168.0.105 --session livecheck` attached; the client's
  chrome and a live pane rendered.
- A command typed into the pane — `uname -m; hostname; echo MARKER-$(uname -m)`
  — answered **`MARKER-aarch64`**, which this x86_64 machine cannot produce. The
  shell is demonstrably on the far side.
- Prefix-`d` detached; the client process exited **0**.
- The session kept running on the host: `amx --session livecheck ping` answered,
  and `session state` showed one workspace and one pane at 98×27 with
  `"cwd": "/home/saiful"` — incidentally the `PaneState::cwd` field W14 added,
  working over a real network.
- **Re-attaching from the same laptop showed `MARKER-aarch64` still on screen**:
  the grid survived the detach and the reconnect.
- `amx --remote <host> session list` refuses by name, as W11 designed —
  `--remote` attaches, it does not carry verbs.

The loopback tier below it still runs, and it is the one CI has:

```
$ AMX_TEST_SSHD=1 cargo test -p amx-rig --test remote_ssh
running 1 test
test loopback_ssh_attach_renders_a_real_pane ... ok
```

A real `sshd` on 127.0.0.1, a real `ssh` connection with its own key and config,
a real remote `amx server`, `ssh … exec amx _bridge` as the transport, a pane
that renders its prompt, a typed line whose output comes back, and a detach that
leaves the session running. That is tier 2 of D-M3-9 and it is the whole of the
amx code in the path; what it cannot exercise is a network, a different kernel,
a different amx build on the far side, or an ssh configuration nobody wrote for
a test.

All three of D-M3-9's tiers now have a run behind them, and the third one is no
longer waiting on a human.

**Still not proven.** The far side ran a binary cross-compiled on the near side
from the same tree, so this is current-vs-current across two architectures, not
two independently built or differently versioned amxes. And no handoff or
`update apply` was exercised over the remote link — the upgrade evidence in this
note is all local.

---

## 6. The checklist

| # | Step | Result |
|---|---|---|
| 1 | scratch roots, borrowed credentials, markers unset | **pass** — five top-level Claude Code sessions, no first-run dialog |
| 2 | `amx integration install claude`, then `status` | **pass** — `current`, naming the binary it wrote |
| 3 | five real Claude Code sessions mid-conversation | **pass** — five distinct conversation ids, each holding a word only it heard |
| 4 | a real staged binary, `--version`-bumped from the same tree | **pass** — 0.1.1, built and published through a `file://` manifest |
| 5 | `amx update apply` | **pass** — verified, installed, handed over, exit 0 in 3.2 s |
| 6 | none die | **pass** — the same five child pids, by identity |
| 7 | no visible screen content lost | **pass, qualified** — byte-identical for a pane that does not repaint; an agent redraws its own UI (§4.1) |
| 8 | every conversation answers its remembered-word probe | **pass** — 5/5, and 3/3 on the second run |
| 9 | the old server exits cleanly | **pass** — exit 0, inside its drain |
| 10 | an interrupted upgrade aborts back to a working session | **partial** — the pre-commit abort is verified with real agents; the mid-restore window is 24 ms and is CI's (§4.3) |
| 11 | `amx session report` explains an aborted upgrade | **pass** — outcome, stage, binary and reason, in the human output |
| 12 | one SSH attach from a genuinely different machine | **pass** — Fedora Asahi Remix 44 aarch64 on the LAN, attached from this x86_64 machine, and a bug found on the way (§5) |

| Date | amx | Successor | Claude Code | Platform | Steps not passed |
|---|---|---|---|---|---|
| 2026-08-08 | `59f6ed8` (0.1.0) | 0.1.1, same tree | 2.1.226 | Arch Linux 7.1.5 x86_64 | 10 partial |

---

## 7. Re-running this

The harness is the product plus a Python driver's worth of glue, and it is not
checked in for the same reason M2's was not: every part of it that is not
disposable is already a verb.

1. **Make a successor.** Bump `[workspace.package] version` in the root
   `Cargo.toml`, `cargo build -p amx`, copy `target/debug/amx` somewhere, put the
   version back and rebuild. Two builds and a `cp`; the result is a genuine
   N→N+1 upgrade, which is the one thing no test in this repository can do.
2. **Isolate.** Scratch `XDG_RUNTIME_DIR`/`XDG_STATE_HOME`/`XDG_CONFIG_HOME`, a
   scratch `CLAUDE_CONFIG_DIR` with `.credentials.json` symlinked in from the
   real one, `.claude.json` seeded with `hasCompletedOnboarding` and a trusted
   project directory, and every `CLAUDE_CODE_*`/`CLAUDECODE`/`CLAUDE_PID`/
   `CLAUDE_EFFORT`/`AMX_*` variable unset.
3. **Run the binary under test from a copy.** `amx update apply` replaces the
   file it is executing; pointed at the build artifact it would install over
   your own build.
4. **Publish a channel.** A `latest.json` with the successor's version, a
   `file://` asset URL and its sha256, and `[update] channel` in the scratch
   `config.toml`.
5. `amx integration install claude`; `amx server`; `agent.start --kind claude`
   per conversation; a remembered word each; `amx update apply`; ask again.
6. **Capture screens immediately either side of the swap**, with the agents
   quiet — anything else measures the agent's own repainting rather than the
   handoff (§4.1). A shell pane is the control.

§5 is the one part of this that no single machine can run. What it takes is a
second machine, a binary it can execute — `cargo build --release --target
<triple> -p amx` with `CARGO_TARGET_<TRIPLE>_LINKER` pointed at a
`zig cc -target <target>` wrapper, since `crates/amx-vt/build.rs` already knows
the target map — and one `amx --remote`. Choose a host whose login shell is not
POSIX if there is one; that is what the run above was worth.
