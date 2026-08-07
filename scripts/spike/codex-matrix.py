#!/usr/bin/env python3
"""Drives the installed Codex CLI through the same scenarios as Claude Code.

Codex keeps its hooks in `$CODEX_HOME/hooks.json` and gates them behind an
interactive review, so this matrix runs against a throwaway CODEX_HOME that
borrows the real one's credentials by symlink. The gate itself is one of the
measurements: `untrusted-hooks` answers it by declining.

    python3 scripts/spike/codex-matrix.py --out /tmp/spike-codex
    python3 scripts/spike/codex-matrix.py --out /tmp/spike-codex --only clean-turn
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

UI = {
    "dir_trust": r"Do you trust the contents of this directory",
    "hook_review": r"Hooks need review",
    "ready": r"to change",
    "working": r"esc to interrupt",
    "approval": r"(Allow command|approve|Yes, proceed|1\. Yes)",
}


def start_agent(ctx: Ctx, root: str, home: str, args: str = "", trust_hooks: bool = True,
                timeout: float = 90.0):
    """Type `codex` at a pane's shell and walk its two startup gates."""
    p = ctx.spawn_shell(root, env_extra={"CODEX_HOME": home})
    ctx.mark("agent_launch", ts=p.send_line(f"codex {args}".strip()), args=args, home=home)
    since = 0
    for _ in range(3):
        hit = p.wait_for(f"({UI['dir_trust']}|{UI['hook_review']}|{UI['ready']})",
                         timeout=timeout, since=since)
        since = hit[2].end()  # never answer the same gate twice
        seen = "".join(hit[2].group(0).split())  # the paint log has no spaces
        if "trustthecontents" in seen:
            ctx.mark("dir_trust_painted", ts=hit[1])
            time.sleep(0.8)
            ctx.mark("dir_trust_accepted", ts=p.send(ENTER))
            continue
        if "Hooksneedreview" in seen:
            ctx.mark("hook_review_painted", ts=hit[1])
            time.sleep(1.0)
            if trust_hooks:
                p.send(DOWN)  # 2. Trust all and continue
                time.sleep(0.4)
                ctx.mark("hooks_trusted", ts=p.send(ENTER))
            else:
                p.send(DOWN)
                time.sleep(0.3)
                p.send(DOWN)  # 3. Continue without trusting
                time.sleep(0.4)
                ctx.mark("hooks_left_untrusted", ts=p.send(ENTER))
            continue
        break
    ctx.mark("agent_ready")
    time.sleep(3.0)
    return p


def stop_agent(ctx: Ctx):
    p = ctx.pty
    if not p:
        return
    try:
        ctx.mark("agent_exit_requested", ts=p.send("/quit"))
        time.sleep(0.6)
        p.send(ENTER)
        p.wait_for(r"SPIKE> ", timeout=12, required=False)
        if p.proc.poll() is None:
            p.send(CTRL_C)
            time.sleep(0.4)
            p.send(CTRL_C)
            time.sleep(0.8)
    finally:
        ctx.finish()


# ---------------------------------------------------------------------------
# Scenarios
# ---------------------------------------------------------------------------


def clean_turn(ctx, root):
    """A turn with no tools: which of the eleven events actually fire?"""
    start_agent(ctx, root, ctx.home)
    t = time.time()
    ctx.prompt("Reply with exactly: OK")
    ctx.wait_hook("Stop", timeout=120, after=t)
    time.sleep(4)
    stop_agent(ctx)


def tool_turn(ctx, root):
    """A turn that reads a file — the tool edges, inside the read-only sandbox."""
    start_agent(ctx, root, ctx.home)
    t = time.time()
    ctx.prompt("Read notes.txt in this directory and reply with only its third line.")
    ctx.wait_hook("Stop", timeout=180, after=t)
    time.sleep(4)
    stop_agent(ctx)


def esc_generation(ctx, root):
    """M1 for Codex: Esc while the model is working. Does anything fire?"""
    start_agent(ctx, root, ctx.home)
    p = ctx.pty
    since = p.mark()
    t = time.time()
    ctx.prompt("Count from 1 to 400, one number per line. Do not run any commands.")
    p.wait_for(UI["working"], timeout=90, since=since)
    time.sleep(5.0)
    ctx.mark("esc_pressed", ts=p.send(ESC))
    time.sleep(12)
    ctx.mark("observation_window_closed")
    stop_agent(ctx)


def esc_tool(ctx, root):
    """M1 for Codex, unambiguously: Esc while a shell command is still running."""
    release = os.path.join(root, "release-the-wait")
    if os.path.exists(release):
        os.remove(release)
    start_agent(ctx, root, ctx.home, args="--sandbox danger-full-access "
                                          "--ask-for-approval never")
    p = ctx.pty
    t = time.time()
    ctx.prompt("Run this shell command and nothing else: "
               f"until [ -f {release} ]; do sleep 2; done")
    ctx.wait_hook("PreToolUse", timeout=180, after=t)
    time.sleep(5.0)
    ctx.mark("esc_pressed", ts=p.send(ESC))
    time.sleep(6)
    ctx.mark("esc_pressed_again", ts=p.send(ESC))
    time.sleep(10)
    ctx.mark("observation_window_closed")
    open(release, "w").close()
    time.sleep(3)
    stop_agent(ctx)


def approval_deny(ctx, root):
    """M2/M3 for Codex: a command the sandbox must ask about, answered 'no'."""
    # `untrusted` escalates anything outside its trusted command set, which is
    # the only way to make the approval dialog deterministic: measured on
    # 0.147.0, the default policy ran an out-of-workspace write without asking.
    start_agent(ctx, root, ctx.home, args="--ask-for-approval untrusted")
    p = ctx.pty
    t = time.time()
    ctx.prompt("Run the shell command: echo spike-codex-probe > /tmp/amx-spike-codex.txt")
    hook = ctx.wait_hook("PermissionRequest", timeout=180, after=t)
    ctx.mark("permission_request_seen", ts=hook["ts"])
    hit = p.find(UI["approval"], since=0)
    if hit:
        ctx.mark("approval_dialog_painted", ts=hit[1], text=hit[2].group(0)[:40])
    time.sleep(2.0)
    # Measured on 0.147.0 the dialog is "1. Yes, proceed / 2. Yes, and don't ask
    # again / 3. No, and tell Codex what to do differently (esc)". Two steps
    # down, not one: one step lands on the *broadest* approval there is.
    p.send(DOWN)
    time.sleep(0.3)
    p.send(DOWN)
    time.sleep(0.5)
    ctx.mark("deny_confirmed", ts=p.send(ENTER))
    time.sleep(14)
    ctx.mark("observation_window_closed")
    stop_agent(ctx)


def approval_esc(ctx, root):
    """M2 for Codex: the same dialog, dismissed with Esc."""
    start_agent(ctx, root, ctx.home, args="--ask-for-approval untrusted")
    p = ctx.pty
    t = time.time()
    ctx.prompt("Run the shell command: echo spike-codex-probe > /tmp/amx-spike-codex.txt")
    hook = ctx.wait_hook("PermissionRequest", timeout=180, after=t)
    ctx.mark("permission_request_seen", ts=hook["ts"])
    time.sleep(2.0)
    ctx.mark("esc_pressed", ts=p.send(ESC))
    time.sleep(14)
    ctx.mark("observation_window_closed")
    stop_agent(ctx)


def untrusted_hooks(ctx, root):
    """V10's honesty test: decline the review gate, then run a whole turn and
    show that not one hook fires."""
    home = os.path.join(ctx.out, f"codex-home-untrusted-{int(time.time())}")
    scratch.build_codex_home(home, HOOK, ctx.log)
    start_agent(ctx, root, home, trust_hooks=False)
    ctx.prompt("Reply with exactly: OK")
    time.sleep(25)
    ctx.mark("observation_window_closed")
    stop_agent(ctx)


def session_end(ctx, root):
    """Does SessionEnd fire on a clean /quit, and what does it carry?"""
    start_agent(ctx, root, ctx.home)
    t = time.time()
    ctx.prompt("Reply with exactly: OK")
    ctx.wait_hook("Stop", timeout=120, after=t)
    time.sleep(1)
    stop_agent(ctx)
    time.sleep(3)


def resume_session(ctx, root):
    """Is the session id the resume ref, and what does a resumed start report?"""
    launched = time.time()
    start_agent(ctx, root, ctx.home)
    t = time.time()
    ctx.prompt("Remember the word BANANA. Reply with exactly: STORED")
    ctx.wait_hook("Stop", timeout=120, after=t)
    started = ctx.wait_hook("SessionStart", timeout=5, after=launched)
    sid = (started.get("payload") or {}).get("session_id")
    ctx.mark("first_session", session_id=sid)
    time.sleep(2)
    stop_agent(ctx)

    time.sleep(2)
    ctx.scenario = "resume-session-2"
    start_agent(ctx, root, ctx.home, args=f"resume {sid}")
    t = time.time()
    ctx.prompt("Reply with only the word you were asked to remember.")
    ctx.wait_hook("Stop", timeout=120, after=t)
    time.sleep(3)
    stop_agent(ctx)


SCENARIOS = {
    "clean-turn": clean_turn,
    "tool-turn": tool_turn,
    "esc-generation": esc_generation,
    "esc-tool": esc_tool,
    "approval-deny": approval_deny,
    "approval-esc": approval_esc,
    "untrusted-hooks": untrusted_hooks,
    "session-end": session_end,
    "resume-session": resume_session,
}

_HOME = {}


def setup(args, log):
    root = os.path.join(args.out, "scratch")
    scratch.build(root, HOOK, log)  # the project files; its .claude/ is unused here
    home = os.path.join(args.out, "codex-home")
    scratch.build_codex_home(home, HOOK, log)
    _HOME["path"] = home
    return root


def _with_home(fn):
    def wrapped(ctx, root):
        ctx.home = _HOME["path"]
        return fn(ctx, root)

    wrapped.__doc__ = fn.__doc__
    return wrapped


if __name__ == "__main__":
    SCENARIOS = {k: _with_home(v) for k, v in SCENARIOS.items()}
    sys.exit(run_matrix(SCENARIOS, setup, "/tmp/amx-spike-codex"))
