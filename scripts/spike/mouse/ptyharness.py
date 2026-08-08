"""A pty the harness owns, so the harness can play the host terminal.

X01 needs two different observations and they are the same mechanism from two
sides:

  * what a program *asks* its host terminal for — the bytes it writes to the
    pty when it wants mouse reporting; and
  * what a program *does* with a report — the bytes it reads back when the
    host terminal answers.

Owning the master end of a pty gives both. Nothing here interprets a mouse
report: it records bytes and injects bytes, and every conclusion drawn from a
run is drawn in docs/notes/m4-mouse-path.md from the recorded bytes.
"""

import os
import pty
import select
import signal
import struct
import fcntl
import termios
import time


def set_winsize(fd, rows, cols):
    """Give the pty a size, so a full-screen program lays itself out."""
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))


class Pty:
    """A forked child on a pty whose master end this process holds."""

    def __init__(self, argv, env=None, rows=24, cols=80, cwd=None):
        self.argv = argv
        self.output = bytearray()
        self.pid, self.fd = pty.fork()
        if self.pid == 0:  # child
            try:
                if cwd:
                    os.chdir(cwd)
                os.execvpe(argv[0], argv, env if env is not None else os.environ)
            except BaseException:
                os._exit(127)
        set_winsize(self.fd, rows, cols)

    def pump(self, seconds):
        """Read whatever the child writes for `seconds`, and keep it."""
        deadline = time.monotonic() + seconds
        while True:
            left = deadline - time.monotonic()
            if left <= 0:
                return
            ready, _, _ = select.select([self.fd], [], [], left)
            if not ready:
                return
            try:
                chunk = os.read(self.fd, 65536)
            except OSError:
                return
            if not chunk:
                return
            self.output += chunk

    def wait_for(self, needle, seconds):
        """Pump until `needle` appears in the output, or the time is up.

        Returns True if it appeared. A deadline expiring is a failure the
        caller reports, never a green path — the rig's own rule.
        """
        deadline = time.monotonic() + seconds
        while needle not in bytes(self.output):
            left = deadline - time.monotonic()
            if left <= 0:
                return False
            ready, _, _ = select.select([self.fd], [], [], min(left, 0.05))
            if not ready:
                continue
            try:
                chunk = os.read(self.fd, 65536)
            except OSError:
                return needle in bytes(self.output)
            if not chunk:
                return needle in bytes(self.output)
            self.output += chunk
        return True

    def write(self, data):
        """Send bytes to the child as the host terminal's keyboard would."""
        os.write(self.fd, data)

    def close(self, grace=1.0):
        """Signal, reap, and return the child's exit status."""
        try:
            os.kill(self.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        deadline = time.monotonic() + grace
        while time.monotonic() < deadline:
            pid, status = os.waitpid(self.pid, os.WNOHANG)
            if pid:
                try:
                    os.close(self.fd)
                except OSError:
                    pass
                return status
            self.pump(0.02)
        try:
            os.kill(self.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        _, status = os.waitpid(self.pid, 0)
        try:
            os.close(self.fd)
        except OSError:
            pass
        return status


def escape(data):
    """C-style escapes, printable ASCII kept as itself."""
    out = []
    for byte in data:
        if byte == 0x1B:
            out.append("\\e")
        elif byte == 0x0A:
            out.append("\\n")
        elif byte == 0x0D:
            out.append("\\r")
        elif byte == 0x09:
            out.append("\\t")
        elif byte == 0x5C:
            out.append("\\\\")
        elif 0x20 <= byte <= 0x7E:
            out.append(chr(byte))
        else:
            out.append("\\x%02x" % byte)
    return "".join(out)


def private_modes(data):
    """Every DEC private mode set/reset in `data`, in the order written.

    Returns a list of (number, "h"|"l") pairs. `CSI ? 1006 ; 1000 h` counts as
    two, which is how terminfo's `XM` writes it.
    """
    found = []
    at = 0
    while True:
        at = data.find(b"\x1b[?", at)
        if at < 0:
            return found
        end = at + 3
        while end < len(data) and (data[end : end + 1].isdigit() or data[end] == 0x3B):
            end += 1
        if end >= len(data):
            return found
        final = data[end : end + 1]
        if final in (b"h", b"l"):
            for part in data[at + 3 : end].split(b";"):
                if part.isdigit():
                    found.append((int(part), final.decode()))
        at = end + 1
