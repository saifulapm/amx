#!/usr/bin/env python3
"""X01, observation 1: what `mouse_probe` asks for, and what it makes of a reply.

The harness holds the master end of a pty and plays the host terminal. It
records, verbatim, every byte the probe writes on entry and on exit — the
DECSET/DECRST request that `amx-client` does not make today — then feeds it a
series of reports and prints the probe's own transcript back.

This observes amx's half of the path on a real tty with real byte splits. It
does *not* observe an emulator emitting a report from a real wheel: that is
`nested-emulator.sh`, and where it could not be run the note says so.

Usage: probe-on-pty.py <path-to-mouse_probe>
"""

import os
import subprocess
import sys
import tempfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from ptyharness import Pty, escape, private_modes  # noqa: E402

# Each case is (label, bytes written to the probe as if by the terminal).
CASES = [
    ("wheel up", b"\x1b[<64;10;5M"),
    ("wheel down", b"\x1b[<65;10;5M"),
    ("wheel up, shift held", b"\x1b[<68;10;5M"),
    ("wheel up, ctrl held", b"\x1b[<80;10;5M"),
    ("horizontal wheel", b"\x1b[<66;10;5M"),
    ("left press", b"\x1b[<0;10;5M"),
    ("left release", b"\x1b[<0;10;5m"),
    ("drag (motion+button)", b"\x1b[<32;11;5M"),
    ("button 8 (thumb)", b"\x1b[<128;10;5M"),
    ("a plain keypress", b"a"),
    ("an arrow key", b"\x1b[A"),
]

# One report delivered in two reads, split inside the parameters — the split
# `mouse::scan` documents as the one it holds back.
SPLIT = (b"\x1b[<64;1", b"0;5M")


def main():
    if len(sys.argv) != 2:
        sys.exit("usage: probe-on-pty.py <path-to-mouse_probe>")
    probe = os.path.abspath(sys.argv[1])
    if not os.access(probe, os.X_OK):
        sys.exit("not executable: %s" % probe)

    with tempfile.TemporaryDirectory() as tmp:
        log = os.path.join(tmp, "transcript.log")
        child = Pty(
            [probe, "--modes", "1006,1000", "--seconds", "30", "--log", log],
            rows=24,
            cols=80,
        )
        if not child.wait_for(b"listening", 10.0):
            child.close()
            sys.exit("probe never announced itself:\n%s" % escape(bytes(child.output)))

        entry = bytes(child.output)
        print("== what the probe wrote to the terminal on entry ==")
        print("  raw:   %s" % escape(entry))
        print("  modes: %s" % private_modes(entry))
        print()

        print("== reports fed in, and what the probe made of them ==")
        for label, data in CASES:
            child.write(data)
            child.pump(0.15)
            print("  %-22s <- %s" % (label, escape(data)))
        print("  %-22s <- %s  then  %s" % ("split report", escape(SPLIT[0]), escape(SPLIT[1])))
        child.write(SPLIT[0])
        child.pump(0.10)
        child.write(SPLIT[1])
        child.pump(0.15)

        before_exit = len(child.output)
        child.write(b"q")
        child.pump(0.5)
        exit_bytes = bytes(child.output[before_exit:])
        status = child.close()

        print()
        print("== what the probe wrote to the terminal on exit ==")
        print("  raw:   %s" % escape(exit_bytes))
        print("  modes: %s" % private_modes(exit_bytes))
        print("  exit status: %d" % status)
        print()

        print("== the probe's transcript ==")
        with open(log, "r") as handle:
            for line in handle:
                print("  " + line.rstrip("\n"))

        # The terminal is left the way it was found only if every mode the
        # probe set it also reset. Stated here as a check rather than left to
        # the reader of the transcript.
        opened = {mode for mode, action in private_modes(entry) if action == "h"}
        closed = {mode for mode, action in private_modes(exit_bytes) if action == "l"}
        print()
        print("== restoration ==")
        print("  set on entry:  %s" % sorted(opened))
        print("  reset on exit: %s" % sorted(closed))
        print("  balanced: %s" % ("yes" if opened == closed and opened else "NO"))
        return 0 if opened == closed and opened else 1


if __name__ == "__main__":
    sys.exit(main())
