#!/usr/bin/env python3
"""Drives the installed Claude Code through the M2 spike's scenarios.

Each scenario gets a fresh terminal, a fresh agent process, and the same
scratch project: `.claude/settings.json` subscribes every lifecycle event to
`hook-log.sh`, which appends one JSON line per hook invocation. The driver
appends its own lines (`tag: "MARK"`) to the same log, so a keystroke and the
hook it provoked sit on one timeline and `analyze.py` can subtract them.

    python3 scripts/spike/claude-matrix.py --out /tmp/spike
    python3 scripts/spike/claude-matrix.py --out /tmp/spike --only esc-generation
    python3 scripts/spike/claude-matrix.py --list

Everything the agent is asked to do happens inside the scratch project, and
nothing outside it is read or written.
"""

from __future__ import annotations

import os
import sys
import time

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "lib"))

import scratch  # noqa: E402
from ptydrive import CTRL_C, DOWN, ENTER, ESC  # noqa: E402
from runner import Ctx, run_matrix  # noqa: E402

HOOK = os.path.join(os.path.dirname(os.path.abspath(__file__)), "hook-log.sh")

# What the agent paints at the moments the driver has to recognise. Written as
# plain English with plain spaces: the driver matches whitespace loosely,
# because a TUI positions words with cursor moves rather than spaces.
UI = {
    "trust": r"Yes, I trust this folder",
    "ready": r"for shortcuts",
    "working": r"esc to interrupt",
    "dialog": r"Do you want to proceed\?",
}


def start_agent(ctx: Ctx, root: str, args: str = "", timeout: float = 60.0,
                trust_hold: float = 0.6):
    """Launch `claude` by typing it at a pane's shell, and clear the folder-trust
    dialog if this workspace has never been trusted."""
    p = ctx.spawn_shell(root)
    ctx.mark("agent_launch", ts=p.send_line(f"claude {args}".strip()), args=args)
    hit = p.wait_for(f"({UI['trust']}|{UI['ready']})", timeout=timeout)
    if "trust" in hit[2].group(0):
        ctx.mark("trust_dialog_painted", ts=hit[1])
        # The hold is the measurement: does anything fire while the workspace
        # is still untrusted?
        time.sleep(trust_hold)
        ctx.mark("trust_accepted", ts=p.send(ENTER))
        p.wait_for(UI["ready"], timeout=timeout, since=hit[0])
    ctx.mark("agent_ready")
    time.sleep(1.0)
    return p


def stop_agent(ctx: Ctx):
    p = ctx.pty
    if not p:
        return
    try:
        ctx.mark("agent_exit_requested", ts=p.send("/exit"))
        time.sleep(0.5)
        p.send(ENTER)
        p.wait_for(r"SPIKE> ", timeout=12, required=False)
        if p.proc.poll() is None:
            p.send(CTRL_C)
            time.sleep(0.3)
            p.send(CTRL_C)
            time.sleep(0.5)
    finally:
        ctx.finish()


# ---------------------------------------------------------------------------
# Scenarios. Each is `fn(ctx, root)`; the runner handles setup and teardown.
# ---------------------------------------------------------------------------


def clean_turn(ctx, root):
    """A turn with no tools: the baseline SessionStart / UserPromptSubmit / Stop."""
    start_agent(ctx, root)
    t = time.time()
    ctx.prompt("Reply with exactly: OK")
    ctx.wait_hook("Stop", timeout=90, after=t)
    time.sleep(3)  # anything late (a trailing SubagentStop) lands here
    stop_agent(ctx)


def tool_turn(ctx, root):
    """A turn with a pre-approved tool call: the Pre/Post tool edges."""
    start_agent(ctx, root)
    t = time.time()
    ctx.prompt("Read notes.txt and reply with only its third line.")
    ctx.wait_hook("Stop", timeout=90, after=t)
    time.sleep(3)
    stop_agent(ctx)


def esc_generation(ctx, root):
    """M1: Esc while the model is streaming. Does anything fire?"""
    start_agent(ctx, root)
    p = ctx.pty
    since = p.mark()
    ctx.prompt("Count from 1 to 400, one number per line. Do not use any tools.")
    p.wait_for(UI["working"], timeout=60, since=since)
    time.sleep(4.0)  # let generation get properly under way
    ctx.mark("esc_pressed", ts=p.send(ESC))
    time.sleep(10)
    ctx.mark("observation_window_closed")
    stop_agent(ctx)


def esc_tool(ctx, root):
    """M1: Esc while a tool call is running (a wait loop, pre-approved)."""
    release = os.path.join(root, "release-the-wait")
    if os.path.exists(release):
        os.remove(release)
    start_agent(ctx, root)
    p = ctx.pty
    t = time.time()
    ctx.prompt(
        "Use the Bash tool to run exactly this and nothing else: "
        f"until [ -f {release} ]; do sleep 2; done"
    )
    ctx.wait_hook("PreToolUse", timeout=90, after=t, tool="Bash")
    time.sleep(4.0)
    ctx.mark("esc_pressed", ts=p.send(ESC))
    time.sleep(10)
    ctx.mark("observation_window_closed")
    open(release, "w").close()  # release anything the interrupt left running
    time.sleep(3)
    stop_agent(ctx)


def _raise_dialog(ctx):
    t = time.time()
    ctx.prompt("Use the Bash tool to run exactly: echo spike-permission-probe")
    hook = ctx.wait_hook("PermissionRequest", timeout=90, after=t)
    hit = ctx.pty.find(UI["dialog"], since=0)
    if hit:
        ctx.mark("dialog_painted", ts=hit[1], text=hit[2].group(0))
    return hook


def permission_deny(ctx, root):
    """M2/M3: raise the dialog, answer it with a human 'no'."""
    start_agent(ctx, root)
    p = ctx.pty
    _raise_dialog(ctx)
    time.sleep(2.5)
    # The dialog measured on 2.1.224 offers exactly "1. Yes" / "2. No", with
    # "Yes" selected. One step down, then confirm: the human "no".
    ctx.mark("deny_moved_to_no", ts=p.send(DOWN))
    time.sleep(0.6)
    ctx.mark("deny_confirmed", ts=p.send(ENTER))
    time.sleep(14)
    ctx.mark("observation_window_closed")
    stop_agent(ctx)


def permission_esc(ctx, root):
    """M2: raise the dialog, dismiss it with Esc."""
    start_agent(ctx, root)
    p = ctx.pty
    _raise_dialog(ctx)
    time.sleep(2.5)
    ctx.mark("esc_pressed", ts=p.send(ESC))
    time.sleep(12)
    ctx.mark("observation_window_closed")
    stop_agent(ctx)


def fresh_trust(ctx, root):
    """M7: in a workspace Claude Code has never seen, do freshly written
    project hooks run — and when? Builds its own never-trusted scratch."""
    fresh = os.path.join(ctx.out, f"fresh-{int(time.time())}")
    scratch.build(fresh, HOOK, ctx.log)
    start_agent(ctx, fresh, trust_hold=8.0)  # marks trust_dialog_painted / accepted
    t = time.time()
    ctx.prompt("Reply with exactly: OK")
    ctx.wait_hook("Stop", timeout=90, after=t)
    time.sleep(2)
    stop_agent(ctx)


def deny_rule(ctx, root):
    """M2: a call a `permissions.deny` rule refuses — does PermissionDenied fire
    for anything, now that the human 'no' is measured silent?"""
    start_agent(ctx, root)
    t = time.time()
    ctx.prompt("Use the Bash tool to run exactly: whoami")
    ctx.wait_hook("Stop", timeout=120, after=t)
    time.sleep(5)
    ctx.mark("observation_window_closed")
    stop_agent(ctx)


def tool_error(ctx, root):
    """A tool call that fails on its own — the PostToolUseFailure control, to
    tell 'the event never fires' apart from 'the event exists but Esc is not a
    failure it reports'."""
    start_agent(ctx, root)
    t = time.time()
    ctx.prompt("Use the Bash tool to run exactly: cat /no/such/file/amx-spike")
    ctx.wait_hook("Stop", timeout=120, after=t)
    time.sleep(5)
    ctx.mark("observation_window_closed")
    stop_agent(ctx)


def subagent(ctx, root):
    """M4: subagent events, and their order against the parent's Stop."""
    start_agent(ctx, root)
    t = time.time()
    ctx.prompt(
        "Use the Task tool to launch exactly one general-purpose subagent whose "
        "whole job is to reply with the word PONG. Then reply with only: DONE"
    )
    ctx.wait_hook("Stop", timeout=180, after=t)
    time.sleep(8)  # a late SubagentStop after the parent Stop is the finding
    ctx.mark("observation_window_closed")
    stop_agent(ctx)


def clear_command(ctx, root):
    """M8: /clear — a new session id? a SessionStart with source=clear?"""
    start_agent(ctx, root)
    p = ctx.pty
    t = time.time()
    ctx.prompt("Reply with exactly: ONE")
    ctx.wait_hook("Stop", timeout=90, after=t)
    time.sleep(1)
    ctx.mark("clear_sent", ts=p.send("/clear"))
    time.sleep(0.6)
    p.send(ENTER)
    time.sleep(5)
    t = time.time()
    ctx.prompt("Reply with exactly: TWO")
    ctx.wait_hook("Stop", timeout=90, after=t)
    time.sleep(3)
    stop_agent(ctx)


def compact_command(ctx, root):
    """M8: /compact — PreCompact/PostCompact, and what SessionStart says."""
    start_agent(ctx, root)
    p = ctx.pty
    # "Not enough messages to compact" is its own dead end: give it a
    # conversation first, then compact that.
    for n in ("ONE", "TWO", "THREE", "FOUR", "FIVE", "SIX"):
        t = time.time()
        ctx.prompt(f"Reply with exactly: {n}")
        ctx.wait_hook("Stop", timeout=90, after=t)
        time.sleep(0.5)
    ctx.mark("compact_sent", ts=p.send("/compact"))
    time.sleep(0.6)
    p.send(ENTER)
    time.sleep(75)
    ctx.mark("observation_window_closed")
    stop_agent(ctx)


def resume_session(ctx, root):
    """M8: does --resume keep the session id? what source does it report?"""
    launched = time.time()
    start_agent(ctx, root)
    t = time.time()
    ctx.prompt("Remember the word BANANA. Reply with exactly: STORED")
    ctx.wait_hook("Stop", timeout=90, after=t)
    started = ctx.wait_hook("SessionStart", timeout=5, after=launched)
    sid = (started.get("payload") or {}).get("session_id")
    ctx.mark("first_session", session_id=sid)
    time.sleep(2)
    stop_agent(ctx)

    time.sleep(2)
    ctx.scenario = "resume-session-2"
    start_agent(ctx, root, args=f"--resume {sid}")
    t = time.time()
    ctx.prompt("Reply with only the word you were asked to remember.")
    ctx.wait_hook("Stop", timeout=90, after=t)
    time.sleep(3)
    stop_agent(ctx)


def session_end(ctx, root):
    """M8: SessionEnd's reason on a clean /exit."""
    start_agent(ctx, root)
    t = time.time()
    ctx.prompt("Reply with exactly: OK")
    ctx.wait_hook("Stop", timeout=90, after=t)
    time.sleep(1)
    stop_agent(ctx)
    time.sleep(2)


def idle_notification(ctx, root):
    """M5: does Notification fire on a long idle wait?"""
    start_agent(ctx, root)
    t = time.time()
    ctx.prompt("Reply with exactly: OK")
    ctx.wait_hook("Stop", timeout=90, after=t)
    ctx.mark("idle_watch_begins")
    time.sleep(90)
    ctx.mark("observation_window_closed")
    stop_agent(ctx)


def blocked_notification(ctx, root):
    """M5: does Notification fire while a permission dialog waits unanswered?"""
    start_agent(ctx, root)
    _raise_dialog(ctx)
    ctx.mark("dialog_watch_begins")
    time.sleep(90)
    ctx.mark("observation_window_closed")
    ctx.pty.send(ESC)
    time.sleep(2)
    stop_agent(ctx)


SCENARIOS = {
    "clean-turn": clean_turn,
    "tool-turn": tool_turn,
    "esc-generation": esc_generation,
    "esc-tool": esc_tool,
    "permission-deny": permission_deny,
    "permission-esc": permission_esc,
    "fresh-trust": fresh_trust,
    "deny-rule": deny_rule,
    "tool-error": tool_error,
    "subagent": subagent,
    "clear-command": clear_command,
    "compact-command": compact_command,
    "resume-session": resume_session,
    "session-end": session_end,
    "idle-notification": idle_notification,
    "blocked-notification": blocked_notification,
}


def setup(args, log):
    root = os.path.join(args.out, "scratch")
    scratch.build(root, HOOK, log, all_events=args.all_events)
    return root


def add_args(ap):
    ap.add_argument("--all-events", action="store_true",
                    help="subscribe all 31 hook events, not just the lifecycle set")


if __name__ == "__main__":
    sys.exit(run_matrix(SCENARIOS, setup, "/tmp/amx-spike", extra_args=[add_args]))
