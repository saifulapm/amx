#!/usr/bin/env python3
"""Reads the spike's log against the scenario script.

    python3 scripts/spike/analyze.py /tmp/amx-spike            # timelines
    python3 scripts/spike/analyze.py /tmp/amx-spike --latency  # the M9 numbers
    python3 scripts/spike/analyze.py /tmp/amx-spike --payloads UserPromptSubmit

One line per hook invocation and one per driver mark, ordered by the clock they
share, grouped by scenario. Everything the findings doc claims about ordering
comes out of this view.
"""

from __future__ import annotations

import argparse
import json
import os
import statistics
import sys


def load(out: str):
    rows = []
    with open(os.path.join(out, "hooks.jsonl")) as f:
        for raw in f:
            try:
                rows.append(json.loads(raw))
            except json.JSONDecodeError:
                pass
    rows.sort(key=lambda r: r["ts"])
    return rows


def by_scenario(rows):
    """Split at scenario_begin marks; hooks fall into the scenario running."""
    groups, current, name = [], [], "(before first scenario)"
    for r in rows:
        if r.get("mark") == "scenario_begin":
            if current:
                groups.append((name, current))
            name, current = r.get("scenario", "?"), [r]
        else:
            current.append(r)
    if current:
        groups.append((name, current))
    return groups


def describe(r) -> str:
    if r.get("tag") == "MARK":
        extra = {k: v for k, v in r.items() if k not in ("ts", "tag", "scenario", "mark")}
        note = " " + json.dumps(extra) if extra else ""
        return f"MARK  {r['mark']}{note[:160]}"
    pl = r.get("payload") or {}
    bits = [f"HOOK  {r['tag']}"]
    for k in ("tool_name", "source", "reason", "agent_id", "agent_type", "is_interrupt",
              "stop_hook_active", "trigger", "message", "notification_type"):
        if k in pl and pl[k] not in (None, ""):
            bits.append(f"{k}={json.dumps(pl[k])[:60]}")
    if "session_id" in pl:
        bits.append(f"sid={pl['session_id'][:8]}")
    if "prompt_id" in pl:
        bits.append(f"pid={str(pl['prompt_id'])[:8]}")
    if r.get("env", {}).get("AMX_ENV") == "1":
        bits.append("amx_env=yes")
    else:
        bits.append("amx_env=NO")
    return "  ".join(bits)


def timelines(groups, only=None):
    for name, rows in groups:
        if only and name not in only:
            continue
        t0 = rows[0]["ts"]
        print(f"\n===== {name} =====")
        for r in rows:
            print(f"  +{r['ts'] - t0:7.3f}  {describe(r)}")


def latency(rows):
    """M9: how long after the driver's keystroke does the hook process start,
    and how far apart are two hooks dispatched for the same event."""
    marks = [r for r in rows if r.get("tag") == "MARK"]
    hooks = [r for r in rows if r.get("tag") != "MARK"]

    def after(mark_name, event, window=90.0):
        out = []
        for m in marks:
            if m.get("mark") != mark_name:
                continue
            for h in hooks:
                if h["tag"].split("#")[0] == event and 0 <= h["ts"] - m["ts"] <= window:
                    out.append(h["ts"] - m["ts"])
                    break
        return out

    print("\n=== keystroke -> hook process start (seconds) ===")
    for mark_name, event in (
        ("prompt_submitted", "UserPromptSubmit"),
        ("prompt_submitted", "PreToolUse"),
        ("prompt_submitted", "Stop"),
        ("agent_launch", "SessionStart"),
    ):
        d = after(mark_name, event)
        if d:
            print(f"  {mark_name:20} -> {event:18} n={len(d):2}  "
                  f"min={min(d):.3f}  median={statistics.median(d):.3f}  max={max(d):.3f}")

    print("\n=== spread between two hooks on the same event (seconds) ===")
    pairs = {}
    for h in hooks:
        base = h["tag"].split("#")[0]
        if "#" not in h["tag"] and base not in pairs:
            pass
        key = (base, (h.get("payload") or {}).get("prompt_id"), round(h["ts"], 0))
        pairs.setdefault(key, []).append(h)
    spreads = []
    for (base, _pid, _t), hs in pairs.items():
        if len(hs) >= 2:
            spreads.append((base, max(x["ts"] for x in hs) - min(x["ts"] for x in hs)))
    if spreads:
        vals = [s for _, s in spreads]
        print(f"  n={len(vals)}  min={min(vals):.4f}  median={statistics.median(vals):.4f}  "
              f"max={max(vals):.4f}")
        for base in sorted({b for b, _ in spreads}):
            v = [s for b, s in spreads if b == base]
            print(f"    {base:20} n={len(v):2} median={statistics.median(v):.4f} max={max(v):.4f}")

    print("\n=== dialog paint vs PermissionRequest hook (seconds; negative = hook first) ===")
    for m in marks:
        if m.get("mark") != "dialog_painted":
            continue
        near = [h for h in hooks if h["tag"].startswith("PermissionRequest")
                and abs(h["ts"] - m["ts"]) < 30]
        for h in near:
            print(f"  {m.get('scenario')}: paint - hook = {m['ts'] - h['ts']:+.3f}")


def payloads(rows, event):
    seen = 0
    for r in rows:
        if r.get("tag", "").split("#")[0] != event:
            continue
        print(json.dumps({"ts": r["ts"], "env": r.get("env"), "tree": r.get("tree"),
                          "payload": r.get("payload")}, indent=2))
        seen += 1
        if seen >= 3:
            break
    if not seen:
        print(f"(no {event} hooks recorded)")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("out")
    ap.add_argument("--only", help="comma-separated scenario names")
    ap.add_argument("--latency", action="store_true")
    ap.add_argument("--payloads", help="dump up to three payloads for this event")
    args = ap.parse_args()

    rows = load(args.out)
    if args.payloads:
        payloads(rows, args.payloads)
    elif args.latency:
        latency(rows)
    else:
        timelines(by_scenario(rows), args.only.split(",") if args.only else None)
    return 0


if __name__ == "__main__":
    sys.exit(main())
