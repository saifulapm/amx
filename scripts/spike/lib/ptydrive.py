"""A small PTY driver: spawn a program on a real terminal, watch what it
paints, type at it, and keep every observation on one clock.

The spike needs three things from a terminal that `subprocess` cannot give it:
a program that only runs under a tty (the agent TUI), keystrokes that reach it
as keystrokes (Esc, Ctrl-C), and a timestamp for the moment a piece of text was
painted — so a hook's timestamp can be compared against the UI it belongs to.

Timestamps are `time.time()` (CLOCK_REALTIME) everywhere, because the hook
logger stamps itself with bash's $EPOCHREALTIME, which is the same clock. That
is what makes "the dialog painted 240 ms before the hook process started" a
sentence about one timeline rather than two.
"""

from __future__ import annotations

import errno
import fcntl
import json
import os
import pty
import re
import select
import signal
import struct
import subprocess
import termios
import threading
import time
from bisect import bisect_left

# CSI/OSC/single-character escapes. Stripping these turns the byte stream into
# a log of everything the program painted, in paint order — not a screen. First
# appearance of a string is therefore exact; "is it on screen now" is not a
# question this buffer can answer, so callers ask "was it repainted recently".
_ANSI = re.compile(
    rb"\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)"  # OSC ... BEL/ST
    rb"|\x1b[\[\?][0-9;\?]*[ -/]*[@-~]"  # CSI
    rb"|\x1b[@-Z\\-_]"  # single-char escapes
    rb"|[\x00-\x08\x0b\x0c\x0e-\x1f\x7f]"  # control bytes except \t and \n
)


class Timeout(Exception):
    """A wait_for whose pattern never arrived."""


class Pty:
    def __init__(self, argv, env=None, cwd=None, cols=120, rows=40, record_dir=None, name="pty"):
        self.argv = argv
        self.name = name
        self.text = ""  # everything painted, escapes stripped
        self._stamps: list[tuple[int, float]] = []  # (end index in .text, arrival time)
        self._lock = threading.Lock()
        self.closed = False

        self._raw = None
        self._chunks = None
        if record_dir:
            os.makedirs(record_dir, exist_ok=True)
            self._raw = open(os.path.join(record_dir, f"{name}.raw"), "wb")
            self._chunks = open(os.path.join(record_dir, f"{name}.chunks.jsonl"), "w")

        self.master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
        self.proc = subprocess.Popen(
            argv,
            stdin=slave,
            stdout=slave,
            stderr=slave,
            cwd=cwd,
            env=env,
            preexec_fn=os.setsid,
            close_fds=True,
        )
        os.close(slave)
        self._reader = threading.Thread(target=self._pump, daemon=True)
        self._reader.start()

    # -- reading ---------------------------------------------------------

    def _pump(self):
        offset = 0
        while True:
            try:
                r, _, _ = select.select([self.master], [], [], 0.2)
            except (OSError, ValueError):
                break
            if not r:
                if self.closed:
                    break
                continue
            try:
                chunk = os.read(self.master, 65536)
            except OSError as e:
                if e.errno in (errno.EIO, errno.EBADF):
                    break
                raise
            if not chunk:
                break
            ts = time.time()
            if self._raw:
                self._raw.write(chunk)
                self._raw.flush()
                self._chunks.write(json.dumps({"ts": ts, "off": offset, "len": len(chunk)}) + "\n")
                self._chunks.flush()
            offset += len(chunk)
            painted = _ANSI.sub(b"", chunk).decode("utf-8", "replace")
            with self._lock:
                self.text += painted
                self._stamps.append((len(self.text), ts))

    def _time_at(self, index: int) -> float:
        """When the character at `index` of .text arrived."""
        ends = [e for e, _ in self._stamps]
        i = bisect_left(ends, index + 1)
        return self._stamps[min(i, len(self._stamps) - 1)][1]

    @staticmethod
    def _flex(pattern: str) -> str:
        """A TUI positions words with cursor moves, not spaces, so the paint log
        has no spaces between them. Every literal run of whitespace in a pattern
        therefore matches any amount of whitespace, including none."""
        return re.sub(r"(?<!\\)\s+", r"\\s*", pattern)

    def find(self, pattern: str, since: int = 0, flex: bool = True):
        """(index, arrival time, match) of the first match at or after `since`."""
        rx = re.compile(self._flex(pattern) if flex else pattern)
        with self._lock:
            m = rx.search(self.text, since)
            if not m:
                return None
            return m.start(), self._time_at(m.start()), m

    def wait_for(self, pattern: str, timeout: float = 60.0, since: int = 0, required=True,
                 flex: bool = True):
        """Block until `pattern` is painted. Returns (index, arrival ts, match)."""
        deadline = time.time() + timeout
        while time.time() < deadline:
            hit = self.find(pattern, since, flex)
            if hit:
                return hit
            if self.proc.poll() is not None:
                time.sleep(0.2)  # drain
                hit = self.find(pattern, since, flex)
                if hit:
                    return hit
                break
            time.sleep(0.05)
        if required:
            raise Timeout(f"{self.name}: never saw /{pattern}/ in {timeout}s")
        return None

    def repainted_since(self, pattern: str, window: float = 1.5, flex: bool = True) -> bool:
        """Was `pattern` painted within the last `window` seconds? The closest a
        paint log gets to 'it is on screen'."""
        cutoff = time.time() - window
        rx = re.compile(self._flex(pattern) if flex else pattern)
        with self._lock:
            for m in rx.finditer(self.text):
                if self._time_at(m.start()) >= cutoff:
                    return True
        return False

    def mark(self) -> int:
        """Current end of the paint log, for `since=`."""
        with self._lock:
            return len(self.text)

    def tail(self, n=2000) -> str:
        with self._lock:
            return self.text[-n:]

    # -- writing ---------------------------------------------------------

    def send(self, data: str) -> float:
        ts = time.time()
        os.write(self.master, data.encode())
        return ts

    def send_line(self, line: str) -> float:
        return self.send(line + "\r")

    # -- lifecycle -------------------------------------------------------

    def close(self, grace: float = 3.0):
        self.closed = True
        if self.proc.poll() is None:
            try:
                os.killpg(os.getpgid(self.proc.pid), signal.SIGHUP)
            except OSError:
                pass
            deadline = time.time() + grace
            while time.time() < deadline and self.proc.poll() is None:
                time.sleep(0.05)
            if self.proc.poll() is None:
                try:
                    os.killpg(os.getpgid(self.proc.pid), signal.SIGKILL)
                except OSError:
                    pass
        self._reader.join(timeout=2)
        for f in (self._raw, self._chunks, None):
            if f:
                f.close()
        try:
            os.close(self.master)
        except OSError:
            pass


# Keys the agent TUI cares about.
ESC = "\x1b"
CTRL_C = "\x03"
CTRL_D = "\x04"
ENTER = "\r"
DOWN = "\x1b[B"
UP = "\x1b[A"
