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

import argparse
import json
import os
import sys
import time
import traceback

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "lib"))

import scratch  # noqa: E402
from ptydrive import CTRL_C, DOWN, ENTER, ESC, Pty, Timeout  # noqa: E402

HOOK = os.path.join(os.path.dirname(os.path.abspath(__file__)), "hook-log.sh")

# What the agent paints at the moments the driver has to recognise. Written as
# plain English with plain spaces: the driver matches whitespace loosely,
# because a TUI positions words with cursor moves rather than spaces.
UI = {
    "trust": r"Yes, I trust this folder",
    "ready": r"for shortcuts",
    "working": r"esc to interrupt",
    "dialog": r"(Do you want|Allow|Bash command)",
}

PLANTED_ENV = {
    "AMX_ENV": "1",
    "AMX_SESSION": "spike-session-01",
    "AMX_SOCKET": "/run/user/1000/amx/spike.sock",
    "AMX_PANE_ID": "b7c3f0de-1c9a-4f77-9a4e-0d2f2a1c5e01",
    "AMX_WORKSPACE_ID": "0f2b9c11-2a6d-4a1e-9d33-5c7b2e4a8801",
    "AMX_HOOK_TOKEN": "spike-token-8f31a0",
}


class Ctx:
    def __init__(self, out: str, scenario: str):
        self.out = out
        self.scenario = scenario
        self.log = os.path.join(out, "hooks.jsonl")
        self.dumps = os.path.join(out, "dumps")
        os.makedirs(self.dumps, exist_ok=True)
        self.pty: Pty | None = None

    # -- the shared timeline ---------------------------------------------

    def mark(self, name: str, ts: float | None = None, **extra):
        line = {
            "ts": ts if ts is not None else time.time(),
            "tag": "MARK",
            "scenario": self.scenario,
            "mark": name,
        }
        line.update(extra)
        with open(self.log, "a") as f:
            f.write(json.dumps(line) + "\n")

    def hooks(self):
        """Every hook line logged so far, oldest first."""
        out = []
        with open(self.log) as f:
            for raw in f:
                try:
                    out.append(json.loads(raw))
                except json.JSONDecodeError:
                    pass
        return out

    def wait_hook(self, event: str, timeout: float = 60.0, after: float = 0.0, tool: str = None):
        """Block until a hook line for `event` (logged after `after`) appears."""
        deadline = time.time() + timeout
        while time.time() < deadline:
            for h in self.hooks():
                pl = h.get("payload") or {}
                if h.get("tag", "").split("#")[0] != event or h["ts"] <= after:
                    continue
                if tool and pl.get("tool_name") != tool:
                    continue
                return h
            time.sleep(0.1)
        raise Timeout(f"no {event} hook within {timeout}s")

    # -- the agent under test --------------------------------------------

    def start_agent(self, root: str, args: str = "", timeout: float = 60.0):
        env = {
            k: v
            for k, v in os.environ.items()
            if not k.startswith(("CLAUDE_", "CLAUDECODE", "AI_AGENT"))
        }
        env.update(PLANTED_ENV)
        env.update({"TERM": "xterm-256color", "PS1": "SPIKE> ", "PS2": "> "})
        self.pty = Pty(
            ["/bin/bash", "--norc", "--noprofile", "-i"],
            env=env,
            cwd=root,
            record_dir=os.path.join(self.out, "rec"),
            name=self.scenario,
        )
        p = self.pty
        p.wait_for(r"SPIKE> ", timeout=15)
        self.mark("agent_launch", ts=p.send_line(f"claude {args}".strip()), args=args)
        hit = p.wait_for(f"({UI['trust']}|{UI['ready']})", timeout=timeout)
        if "trust" in hit[2].group(0):
            self.mark("trust_dialog_painted", ts=hit[1])
            time.sleep(0.6)
            self.mark("trust_accepted", ts=p.send(ENTER))
            p.wait_for(UI["ready"], timeout=timeout, since=hit[0])
        self.mark("agent_ready")
        time.sleep(1.0)
        return p

    def prompt(self, text: str, settle: float = 0.8):
        p = self.pty
        p.send(text)
        time.sleep(settle)
        self.mark("prompt_submitted", ts=p.send(ENTER), text=text)

    def wait_quiet(self, pattern: str, window: float = 2.5, timeout: float = 120.0):
        """Wait until `pattern` has not been repainted for `window` seconds."""
        deadline = time.time() + timeout
        while time.time() < deadline:
            if not self.pty.repainted_since(pattern, window):
                return True
            time.sleep(0.2)
        return False

    def stop_agent(self):
        p = self.pty
        if not p:
            return
        try:
            self.mark("agent_exit_requested", ts=p.send("/exit"))
            time.sleep(0.5)
            p.send(ENTER)
            p.wait_for(r"SPIKE> ", timeout=12, required=False)
            if p.proc.poll() is None:
                p.send(CTRL_C)
                time.sleep(0.3)
                p.send(CTRL_C)
                time.sleep(0.5)
        finally:
            with open(os.path.join(self.dumps, f"{self.scenario}.txt"), "w") as f:
                f.write(p.text)
            p.close()
            self.pty = None


# ---------------------------------------------------------------------------
# Scenarios. Each is `fn(ctx, root)`; the runner handles setup and teardown.
# ---------------------------------------------------------------------------


def clean_turn(ctx, root):
    """A turn with no tools: the baseline SessionStart / UserPromptSubmit / Stop."""
    ctx.start_agent(root)
    t = time.time()
    ctx.prompt("Reply with exactly: OK")
    ctx.wait_hook("Stop", timeout=90, after=t)
    time.sleep(3)  # anything late (a trailing SubagentStop) lands here
    ctx.stop_agent()


def tool_turn(ctx, root):
    """A turn with a pre-approved tool call: the Pre/Post tool edges."""
    ctx.start_agent(root)
    t = time.time()
    ctx.prompt("Read notes.txt and reply with only its third line.")
    ctx.wait_hook("Stop", timeout=90, after=t)
    time.sleep(3)
    ctx.stop_agent()


def esc_generation(ctx, root):
    """M1: Esc while the model is streaming. Does anything fire?"""
    ctx.start_agent(root)
    p = ctx.pty
    since = p.mark()
    ctx.prompt("Count from 1 to 400, one number per line. Do not use any tools.")
    p.wait_for(UI["working"], timeout=60, since=since)
    time.sleep(4.0)  # let generation get properly under way
    ctx.mark("esc_pressed", ts=p.send(ESC))
    time.sleep(10)
    ctx.mark("observation_window_closed")
    ctx.stop_agent()


def esc_tool(ctx, root):
    """M1: Esc while a tool call is running (`sleep 40`, pre-approved)."""
    ctx.start_agent(root)
    p = ctx.pty
    t = time.time()
    ctx.prompt("Use the Bash tool to run exactly: sleep 40")
    ctx.wait_hook("PreToolUse", timeout=90, after=t, tool="Bash")
    time.sleep(3.0)
    ctx.mark("esc_pressed", ts=p.send(ESC))
    time.sleep(10)
    ctx.mark("observation_window_closed")
    ctx.stop_agent()


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
    ctx.start_agent(root)
    p = ctx.pty
    _raise_dialog(ctx)
    time.sleep(2.5)
    # The deny option is the last one; walk down to it rather than assuming a
    # count, then confirm. A follow-up feedback box is submitted empty.
    for _ in range(4):
        p.send(DOWN)
        time.sleep(0.15)
    ctx.mark("deny_selected", ts=p.send(ENTER))
    time.sleep(2.0)
    ctx.mark("feedback_submitted", ts=p.send(ENTER))
    time.sleep(12)
    ctx.mark("observation_window_closed")
    ctx.stop_agent()


def permission_esc(ctx, root):
    """M2: raise the dialog, dismiss it with Esc."""
    ctx.start_agent(root)
    p = ctx.pty
    _raise_dialog(ctx)
    time.sleep(2.5)
    ctx.mark("esc_pressed", ts=p.send(ESC))
    time.sleep(12)
    ctx.mark("observation_window_closed")
    ctx.stop_agent()


def subagent(ctx, root):
    """M4: subagent events, and their order against the parent's Stop."""
    ctx.start_agent(root)
    t = time.time()
    ctx.prompt(
        "Use the Task tool to launch exactly one general-purpose subagent whose "
        "whole job is to reply with the word PONG. Then reply with only: DONE"
    )
    ctx.wait_hook("Stop", timeout=180, after=t)
    time.sleep(8)  # a late SubagentStop after the parent Stop is the finding
    ctx.mark("observation_window_closed")
    ctx.stop_agent()


def clear_command(ctx, root):
    """M8: /clear — a new session id? a SessionStart with source=clear?"""
    ctx.start_agent(root)
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
    ctx.stop_agent()


def compact_command(ctx, root):
    """M8: /compact — PreCompact/PostCompact, and what SessionStart says."""
    ctx.start_agent(root)
    p = ctx.pty
    t = time.time()
    ctx.prompt("Reply with exactly: ONE")
    ctx.wait_hook("Stop", timeout=90, after=t)
    time.sleep(1)
    ctx.mark("compact_sent", ts=p.send("/compact"))
    time.sleep(0.6)
    p.send(ENTER)
    time.sleep(45)
    ctx.mark("observation_window_closed")
    ctx.stop_agent()


def resume_session(ctx, root):
    """M8: does --resume keep the session id? what source does it report?"""
    ctx.start_agent(root)
    t = time.time()
    ctx.prompt("Remember the word BANANA. Reply with exactly: STORED")
    ctx.wait_hook("Stop", timeout=90, after=t)
    started = ctx.wait_hook("SessionStart", timeout=5, after=t - 3600)
    sid = (started.get("payload") or {}).get("session_id")
    ctx.mark("first_session", session_id=sid)
    time.sleep(2)
    ctx.stop_agent()

    time.sleep(2)
    ctx.scenario = "resume-session-2"
    ctx.start_agent(root, args=f"--resume {sid}")
    t = time.time()
    ctx.prompt("Reply with only the word you were asked to remember.")
    ctx.wait_hook("Stop", timeout=90, after=t)
    time.sleep(3)
    ctx.stop_agent()


def session_end(ctx, root):
    """M8: SessionEnd's reason on a clean /exit."""
    ctx.start_agent(root)
    t = time.time()
    ctx.prompt("Reply with exactly: OK")
    ctx.wait_hook("Stop", timeout=90, after=t)
    time.sleep(1)
    ctx.stop_agent()
    time.sleep(2)


def idle_notification(ctx, root):
    """M5: does Notification fire on a long idle wait?"""
    ctx.start_agent(root)
    t = time.time()
    ctx.prompt("Reply with exactly: OK")
    ctx.wait_hook("Stop", timeout=90, after=t)
    ctx.mark("idle_watch_begins")
    time.sleep(90)
    ctx.mark("observation_window_closed")
    ctx.stop_agent()


def blocked_notification(ctx, root):
    """M5: does Notification fire while a permission dialog waits unanswered?"""
    ctx.start_agent(root)
    _raise_dialog(ctx)
    ctx.mark("dialog_watch_begins")
    time.sleep(90)
    ctx.mark("observation_window_closed")
    ctx.pty.send(ESC)
    time.sleep(2)
    ctx.stop_agent()


SCENARIOS = {
    "clean-turn": clean_turn,
    "tool-turn": tool_turn,
    "esc-generation": esc_generation,
    "esc-tool": esc_tool,
    "permission-deny": permission_deny,
    "permission-esc": permission_esc,
    "subagent": subagent,
    "clear-command": clear_command,
    "compact-command": compact_command,
    "resume-session": resume_session,
    "session-end": session_end,
    "idle-notification": idle_notification,
    "blocked-notification": blocked_notification,
}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default="/tmp/amx-spike")
    ap.add_argument("--only", help="comma-separated scenario names")
    ap.add_argument("--list", action="store_true")
    ap.add_argument("--all-events", action="store_true",
                    help="subscribe all 31 hook events, not just the lifecycle set")
    ap.add_argument("--keep-log", action="store_true", help="append to an existing log")
    args = ap.parse_args()

    if args.list:
        for name in SCENARIOS:
            print(name)
        return 0

    os.makedirs(args.out, exist_ok=True)
    log = os.path.join(args.out, "hooks.jsonl")
    root = os.path.join(args.out, "scratch")
    scratch.build(root, HOOK, log, all_events=args.all_events)
    if not args.keep_log:
        open(log, "w").close()

    names = args.only.split(",") if args.only else list(SCENARIOS)
    results = {}
    for name in names:
        fn = SCENARIOS[name]
        ctx = Ctx(args.out, name)
        print(f"--- {name}", flush=True)
        ctx.mark("scenario_begin", doc=(fn.__doc__ or "").strip())
        started = time.time()
        try:
            fn(ctx, root)
            results[name] = "ok"
        except Exception as e:  # a scenario that fails is data, not a stop
            results[name] = f"{type(e).__name__}: {e}"
            traceback.print_exc()
            ctx.mark("scenario_error", error=results[name])
            try:
                ctx.stop_agent()
            except Exception:
                pass
        ctx.scenario = name
        ctx.mark("scenario_end", ok=results[name] == "ok", seconds=round(time.time() - started, 1))
        print(f"    {results[name]}  ({time.time() - started:.0f}s)", flush=True)

    with open(os.path.join(args.out, "results.json"), "w") as f:
        json.dump(results, f, indent=2)
    print(json.dumps(results, indent=2))
    return 0 if all(v == "ok" for v in results.values()) else 1


if __name__ == "__main__":
    sys.exit(main())
