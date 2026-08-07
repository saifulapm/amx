"""What the per-agent matrices share: one log, one clock, one scenario loop.

`Ctx` owns the hook log both the agent's hooks and the driver's marks append to,
the terminal the agent runs in, and the small vocabulary a scenario needs —
mark, prompt, wait for a hook, wait for the screen to go quiet. Agent-specific
startup (trust dialogs, readiness patterns) lives in the matrix that drives it.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import time
import traceback

from ptydrive import CTRL_C, ENTER, Pty, Timeout  # noqa: F401  (re-exported)

# The identity amx plants on a pane before the agent starts (D-M2-4). Whether it
# survives into hook processes is M6.
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
        out = []
        if not os.path.exists(self.log):
            return out
        with open(self.log) as f:
            for raw in f:
                try:
                    out.append(json.loads(raw))
                except json.JSONDecodeError:
                    pass
        return out

    def wait_hook(self, event: str, timeout: float = 60.0, after: float = 0.0, tool: str = None):
        """Block until a hook line for `event`, logged after `after`, appears."""
        deadline = time.time() + timeout
        while time.time() < deadline:
            for h in self.hooks():
                pl = h.get("payload") or {}
                tag = h.get("tag", "").split("#")[0]
                if tag != event or h["ts"] <= after:
                    continue
                if tool and pl.get("tool_name") != tool:
                    continue
                return h
            time.sleep(0.1)
        raise Timeout(f"no {event} hook within {timeout}s")

    # -- the terminal ----------------------------------------------------

    def spawn_shell(self, cwd: str, env_extra: dict = None, cols=120, rows=40) -> Pty:
        """An interactive shell with amx's identity planted, exactly as a pane
        would have it — the agent is then typed at it, so the inheritance chain
        under test is the real one: shell -> agent -> hook."""
        env = {
            k: v
            for k, v in os.environ.items()
            if not k.startswith(("CLAUDE_", "CLAUDECODE", "AI_AGENT"))
        }
        env.update(PLANTED_ENV)
        env.update({"TERM": "xterm-256color", "PS1": "SPIKE> ", "PS2": "> "})
        env.update(env_extra or {})
        self.pty = Pty(
            ["/bin/bash", "--norc", "--noprofile", "-i"],
            env=env,
            cwd=cwd,
            record_dir=os.path.join(self.out, "rec"),
            name=self.scenario,
            cols=cols,
            rows=rows,
        )
        self.pty.wait_for(r"SPIKE> ", timeout=15)
        return self.pty

    def prompt(self, text: str, settle: float = 0.8):
        p = self.pty
        p.send(text)
        time.sleep(settle)
        self.mark("prompt_submitted", ts=p.send(ENTER), text=text)

    def wait_quiet(self, pattern: str, window: float = 2.5, timeout: float = 120.0) -> bool:
        """Wait until `pattern` has not been repainted for `window` seconds."""
        deadline = time.time() + timeout
        while time.time() < deadline:
            if not self.pty.repainted_since(pattern, window):
                return True
            time.sleep(0.2)
        return False

    def finish(self):
        """Save what the terminal painted and let the process go."""
        p = self.pty
        if not p:
            return
        with open(os.path.join(self.dumps, f"{self.scenario}.txt"), "w") as f:
            f.write(p.text)
        p.close()
        self.pty = None


def run_matrix(scenarios: dict, setup, default_out: str, extra_args=None):
    """The shared entry point: parse args, build the scratch, run scenarios.

    `setup(args, log)` prepares the scratch and returns the project root.
    """
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default=default_out)
    ap.add_argument("--only", help="comma-separated scenario names")
    ap.add_argument("--list", action="store_true")
    ap.add_argument("--keep-log", action="store_true", help="append to an existing log")
    for add in extra_args or []:
        add(ap)
    args = ap.parse_args()

    if args.list:
        for name in scenarios:
            print(name)
        return 0

    os.makedirs(args.out, exist_ok=True)
    log = os.path.join(args.out, "hooks.jsonl")
    if not args.keep_log:
        open(log, "w").close()
    root = setup(args, log)

    names = args.only.split(",") if args.only else list(scenarios)
    results = {}
    for name in names:
        fn = scenarios[name]
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
                ctx.finish()
            except Exception:
                pass
        ctx.scenario = name
        ctx.mark("scenario_end", ok=results[name] == "ok", seconds=round(time.time() - started, 1))
        print(f"    {results[name]}  ({time.time() - started:.0f}s)", flush=True)

    with open(os.path.join(args.out, "results.json"), "w") as f:
        json.dump(results, f, indent=2)
    print(json.dumps(results, indent=2))
    return 0 if all(v == "ok" for v in results.values()) else 1
