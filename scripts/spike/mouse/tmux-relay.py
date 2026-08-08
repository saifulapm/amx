#!/usr/bin/env python3
"""X01, observation 2: what a shipped multiplexer asks for, and what it relays.

tmux occupies exactly the position `amx-client` does — a program between a host
terminal and a pane's application — and it has occupied it for twenty years.
Running it on a pty this harness owns answers three questions from observation
rather than from folklore:

  * whether it holds the host terminal's mouse open all the time, or asks for
    tracking only while something wants it (scenarios `control` and `basic`);
  * what reaches a pane application that enabled mouse reporting (`basic`); and
  * what reaches a pane application that is *not* at the viewport origin
    (`split`) — the case D9's "forwarded unchanged" does not survive.

The pane application is `mouse_probe` itself, which is the honest choice: it is
the program whose behaviour X13 has to reproduce.

Usage: tmux-relay.py <path-to-mouse_probe> [scenario ...]
       scenarios: control basic split   (default: all three)
"""

import os
import subprocess
import sys
import tempfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from ptyharness import Pty, escape, private_modes  # noqa: E402

SOCKET = "amx-x01-spike"
ROWS = 24
COLS = 80

# Reports as a host terminal would deliver them. The row is what matters in
# the split scenario: 20 is well below a horizontal split of a 24-row screen.
FEED = [
    ("wheel up", b"\x1b[<64;10;20M"),
    ("wheel down", b"\x1b[<65;10;20M"),
    ("left press", b"\x1b[<0;10;20M"),
    ("left release", b"\x1b[<0;10;20m"),
]


def config(tmp, mouse):
    path = os.path.join(tmp, "tmux.conf")
    with open(path, "w") as handle:
        handle.write("set -g mouse %s\n" % mouse)
        handle.write("set -g status off\n")
        handle.write("set -g default-terminal 'tmux-256color'\n")
    return path


def env():
    out = dict(os.environ)
    out["TERM"] = "xterm-256color"
    out["TMUX"] = ""
    return out


def kill_server():
    subprocess.run(
        ["tmux", "-L", SOCKET, "kill-server"],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        check=False,
    )


def modes_after(data, needle):
    """The private modes written after `needle` first appears in `data`."""
    at = data.find(needle)
    return private_modes(data if at < 0 else data[at:])


def scenario_control(probe, tmp):
    """A pane application that never asks for the mouse."""
    conf = config(tmp, "off")
    child = Pty(
        ["tmux", "-f", conf, "-L", SOCKET, "new-session", "sh -c 'echo READY; cat'"],
        env=env(),
        rows=ROWS,
        cols=COLS,
    )
    try:
        if not child.wait_for(b"READY", 15.0):
            return "the pane never announced itself", 1
        child.pump(0.5)
        modes = private_modes(bytes(child.output))
        enabled = [m for m, action in modes if action == "h" and m in (1000, 1002, 1003, 1006)]
        print("  tracking modes tmux left ON: %s" % (enabled or "none"))
        print("  every mouse-mode write, in order: %s"
              % [(m, a) for m, a in modes if m in (1000, 1002, 1003, 1006)])
        return None, 0 if not enabled else 1
    finally:
        child.write(b"\x03")
        child.pump(0.2)
        child.close()


def scenario_basic(probe, tmp, mouse):
    """One full-screen pane whose application enables mouse reporting."""
    conf = config(tmp, mouse)
    log = os.path.join(tmp, "basic-%s.log" % mouse)
    child = Pty(
        [
            "tmux", "-f", conf, "-L", SOCKET, "new-session",
            "%s --modes 1006,1000 --seconds 25 --log %s" % (probe, log),
        ],
        env=env(),
        rows=ROWS,
        cols=COLS,
    )
    try:
        if not child.wait_for(b"listening", 15.0):
            return "the pane program never announced itself", 1
        child.pump(0.4)
        startup = bytes(child.output)
        print("  every mouse-mode write, in order: %s"
              % [(m, a) for m, a in private_modes(startup) if m in (1000, 1002, 1003, 1006)])
        for label, data in FEED:
            child.write(data)
            child.pump(0.25)
        child.pump(0.4)
        print("  fed at column 10, row 20 of an 80x24 viewport with one pane:")
        report_lines(log)
        return None, 0
    finally:
        child.write(b"q")
        child.pump(0.3)
        child.close()


def scenario_split(probe, tmp):
    """Two stacked panes; the probe is in the lower one, away from the origin."""
    conf = config(tmp, "off")
    log = os.path.join(tmp, "split.log")
    child = Pty(
        ["tmux", "-f", conf, "-L", SOCKET, "new-session", "sh -c 'echo READY; cat'"],
        env=env(),
        rows=ROWS,
        cols=COLS,
    )
    try:
        if not child.wait_for(b"READY", 15.0):
            return "the first pane never announced itself", 1
        split = subprocess.run(
            [
                "tmux", "-L", SOCKET, "split-window", "-v",
                "%s --modes 1006,1000 --seconds 25 --log %s" % (probe, log),
            ],
            env=env(),
            capture_output=True,
            check=False,
        )
        if split.returncode != 0:
            return "split-window failed: %s" % split.stderr.decode(), 1
        child.pump(0.6)
        geometry = subprocess.run(
            ["tmux", "-L", SOCKET, "list-panes", "-F",
             "#{pane_index} top=#{pane_top} left=#{pane_left} #{pane_height}x#{pane_width}"],
            env=env(),
            capture_output=True,
            check=False,
        )
        print("  panes: %s" % geometry.stdout.decode().strip().replace("\n", " | "))
        deadline_ok = False
        for _ in range(60):
            child.pump(0.1)
            if os.path.exists(log) and "listening" in open(log).read():
                deadline_ok = True
                break
        if not deadline_ok:
            return "the probe in the lower pane never announced itself", 1
        for label, data in FEED:
            child.write(data)
            child.pump(0.25)
        child.pump(0.4)
        print("  fed at column 10, row 20 of the 80x24 viewport:")
        report_lines(log)
        return None, 0
    finally:
        child.write(b"q")
        child.pump(0.3)
        child.close()


def report_lines(log):
    try:
        with open(log, "r") as handle:
            body = handle.read()
    except OSError as err:
        print("    no transcript: %s" % err)
        return
    for line in body.splitlines():
        if line.startswith("["):
            print("    " + line)


def main():
    if len(sys.argv) < 2:
        sys.exit("usage: tmux-relay.py <path-to-mouse_probe> [control|basic|split ...]")
    probe = os.path.abspath(sys.argv[1])
    wanted = sys.argv[2:] or ["control", "basic", "split"]
    failures = 0
    with tempfile.TemporaryDirectory() as tmp:
        for name in wanted:
            print("== scenario: %s ==" % name)
            kill_server()
            if name == "control":
                message, bad = scenario_control(probe, tmp)
            elif name == "basic":
                message, bad = scenario_basic(probe, tmp, "off")
                print("  -- the same, with tmux's own `mouse on` --")
                kill_server()
                message2, bad2 = scenario_basic(probe, tmp, "on")
                message, bad = message or message2, bad + bad2
            elif name == "split":
                message, bad = scenario_split(probe, tmp)
            else:
                message, bad = "unknown scenario", 1
            if message:
                print("  FAILED: %s" % message)
            failures += bad
            kill_server()
            print()
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
