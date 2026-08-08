#!/usr/bin/env python3
"""A synthetic pointer, so a headless run can still make a wheel turn.

X01's first question — "does anything arrive?" — is the one a box with no
human at it cannot normally answer: it needs a real terminal emulator and a
real wheel event. A wlroots-style compositor closes half of that gap. The
`zwlr_virtual_pointer_v1` protocol lets a client create a pointer device the
compositor treats as real, so the emulator under test sees an ordinary wheel
event and has no way to know it was not a hand.

This speaks the Wayland wire protocol directly rather than through bindings,
because there are none installed here. The wire format is four bytes of sender
id, then two bytes of size and two of opcode, then the arguments; strings carry
their NUL and pad to four bytes. Request opcodes are declaration order, taken
from wlr-protocols' `wlr-virtual-pointer-unstable-v1.xml`.

Usage:
  virtual-pointer.py --at X,Y --extent W,H [--scroll-up N] [--scroll-down N]
                     [--click] [--drag X2,Y2] [--display wayland-1]

Every action is separated by a frame, which is what tells the compositor one
logical event has ended.
"""

import argparse
import os
import socket
import struct
import sys
import time

WL_DISPLAY = 1
REGISTRY = 2

# Every other object id is allocated in strictly increasing order as it is
# created. That is not a style preference: a client id map rejects an id that
# skips a slot, and the failure it gives back is the unhelpful "invalid
# arguments for wl_registry#2.bind" — a demarshalling error, not a complaint
# about the values.

# zwlr_virtual_pointer_v1 request opcodes, declaration order.
MOTION = 0
MOTION_ABSOLUTE = 1
BUTTON = 2
AXIS = 3
FRAME = 4
AXIS_SOURCE = 5
AXIS_STOP = 6
AXIS_DISCRETE = 7

AXIS_VERTICAL = 0
AXIS_SOURCE_WHEEL = 0
BTN_LEFT = 0x110


def fixed(value):
    """wl_fixed: signed 24.8."""
    return int(round(value * 256.0))


def message(sender, opcode, body=b""):
    size = 8 + len(body)
    return struct.pack("<II", sender, (size << 16) | opcode) + body


def string(text):
    raw = text.encode() + b"\0"
    pad = (-len(raw)) % 4
    return struct.pack("<I", len(raw)) + raw + b"\0" * pad


class Wayland:
    def __init__(self, display):
        path = display
        if not os.path.isabs(path):
            path = os.path.join(os.environ["XDG_RUNTIME_DIR"], display)
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.connect(path)
        self.buf = b""
        self.last_id = REGISTRY

    def new_id(self):
        self.last_id += 1
        return self.last_id

    def send(self, data):
        self.sock.sendall(data)

    def recv(self, seconds=1.0):
        """Read whatever has arrived, and yield (sender, opcode, body)."""
        self.sock.settimeout(seconds)
        try:
            chunk = self.sock.recv(65536)
        except socket.timeout:
            return
        if not chunk:
            return
        self.buf += chunk
        while len(self.buf) >= 8:
            sender, word = struct.unpack("<II", self.buf[:8])
            size = word >> 16
            opcode = word & 0xFFFF
            if len(self.buf) < size:
                return
            body = self.buf[8:size]
            self.buf = self.buf[size:]
            yield sender, opcode, body

    def roundtrip(self, seconds=2.0):
        """wl_display.sync, drained: any protocol error surfaces here.

        A callback object is destroyed once it fires, so each roundtrip takes
        a fresh id rather than reusing one the compositor has already retired.
        """
        callback = self.new_id()
        self.send(message(WL_DISPLAY, 0, struct.pack("<I", callback)))
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            for sender, opcode, _ in self.check(self.recv(0.2)):
                if sender == callback and opcode == 0:
                    return
        raise SystemExit("the compositor never answered wl_display.sync")

    def check(self, events):
        """Turn a wl_display.error into a readable failure."""
        for sender, opcode, body in events:
            if sender == WL_DISPLAY and opcode == 0:
                obj, code = struct.unpack("<II", body[:8])
                length = struct.unpack("<I", body[8:12])[0]
                text = body[12 : 12 + length - 1].decode(errors="replace")
                raise SystemExit("wayland error on object %d (code %d): %s" % (obj, code, text))
            yield sender, opcode, body


def globals_of(wl):
    """Every advertised global, as {interface: (name, version)}."""
    wl.send(message(WL_DISPLAY, 1, struct.pack("<I", REGISTRY)))
    found = {}
    deadline = time.monotonic() + 2.0
    while time.monotonic() < deadline:
        got = False
        for sender, opcode, body in wl.check(wl.recv(0.3)):
            got = True
            if sender == REGISTRY and opcode == 0:
                name = struct.unpack("<I", body[:4])[0]
                length = struct.unpack("<I", body[4:8])[0]
                interface = body[8 : 8 + length - 1].decode()
                padded = 8 + length + ((-length) % 4)
                version = struct.unpack("<I", body[padded : padded + 4])[0]
                found[interface] = (name, version)
        if not got and found:
            return found
    return found


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--display", default=os.environ.get("WAYLAND_DISPLAY", "wayland-1"))
    parser.add_argument("--at", default="1920,1080", help="absolute position, X,Y")
    parser.add_argument("--extent", default="3840,2160", help="the axes' extents, W,H")
    parser.add_argument("--scroll-up", type=int, default=0)
    parser.add_argument("--scroll-down", type=int, default=0)
    parser.add_argument("--click", action="store_true")
    parser.add_argument("--drag", default=None, help="press here, move to X,Y, release")
    parser.add_argument("--pause", type=float, default=0.12)
    parser.add_argument(
        "--move-by",
        default=None,
        help="relative motion, DX,DY — the placement that needs no output frame",
    )
    parser.add_argument(
        "--no-output",
        action="store_true",
        help="create the pointer without an output, the v1 constructor",
    )
    args = parser.parse_args()

    x, y = (int(part) for part in args.at.split(","))
    width, height = (int(part) for part in args.extent.split(","))

    wl = Wayland(args.display)
    advertised = globals_of(wl)
    manager = advertised.get("zwlr_virtual_pointer_manager_v1")
    if not manager:
        raise SystemExit(
            "the compositor does not advertise zwlr_virtual_pointer_manager_v1; "
            "advertised: %s" % sorted(advertised)
        )
    def bind(interface, cap):
        entry = advertised.get(interface)
        if not entry:
            return None
        name, version = entry
        oid = wl.new_id()
        wl.send(
            message(
                REGISTRY,
                0,
                struct.pack("<I", name)
                + string(interface)
                + struct.pack("<II", min(version, cap), oid),
            )
        )
        return oid

    # A pointer with no seat and no output has no frame of reference for
    # absolute motion, so bind both when they are offered and use the
    # `_with_output` constructor. This is the difference between a pointer the
    # compositor places and one it quietly ignores.
    seat = bind("wl_seat", 7)
    output = bind("wl_output", 4)
    manager_id = bind("zwlr_virtual_pointer_manager_v1", 2)
    pointer = wl.new_id()
    if manager[1] >= 2 and output and not args.no_output:
        # create_virtual_pointer_with_output(seat, output, id)
        wl.send(
            message(manager_id, 2, struct.pack("<III", seat or 0, output, pointer))
        )
    else:
        # create_virtual_pointer(seat, id)
        wl.send(message(manager_id, 0, struct.pack("<II", seat or 0, pointer)))
    wl.roundtrip()
    POINTER = pointer

    now = [0]

    def stamp():
        now[0] += 10
        return now[0]

    def frame():
        wl.send(message(POINTER, FRAME))
        wl.roundtrip()
        time.sleep(args.pause)

    def move(to_x, to_y):
        wl.send(
            message(
                POINTER,
                MOTION_ABSOLUTE,
                struct.pack("<IIIII", stamp(), to_x, to_y, width, height),
            )
        )
        frame()

    def scroll(clicks, direction):
        # Negative is up, the direction the surface content moves. One
        # discrete click is 15 units of axis value, the wheel's traditional
        # step.
        for _ in range(clicks):
            wl.send(message(POINTER, AXIS_SOURCE, struct.pack("<I", AXIS_SOURCE_WHEEL)))
            wl.send(
                message(
                    POINTER,
                    AXIS_DISCRETE,
                    struct.pack("<IIii", stamp(), AXIS_VERTICAL, fixed(15.0 * direction), direction),
                )
            )
            wl.send(
                message(
                    POINTER,
                    AXIS,
                    struct.pack("<IIi", stamp(), AXIS_VERTICAL, fixed(15.0 * direction)),
                )
            )
            frame()
            wl.send(message(POINTER, AXIS_SOURCE, struct.pack("<I", AXIS_SOURCE_WHEEL)))
            wl.send(message(POINTER, AXIS_STOP, struct.pack("<II", stamp(), AXIS_VERTICAL)))
            frame()

    if args.move_by:
        dx, dy = (float(part) for part in args.move_by.split(","))
        wl.send(
            message(POINTER, MOTION, struct.pack("<Iii", stamp(), fixed(dx), fixed(dy)))
        )
        frame()
    else:
        move(x, y)
    if args.scroll_up:
        scroll(args.scroll_up, -1)
    if args.scroll_down:
        scroll(args.scroll_down, 1)
    if args.click:
        wl.send(message(POINTER, BUTTON, struct.pack("<III", stamp(), BTN_LEFT, 1)))
        frame()
        wl.send(message(POINTER, BUTTON, struct.pack("<III", stamp(), BTN_LEFT, 0)))
        frame()
    if args.drag:
        to_x, to_y = (int(part) for part in args.drag.split(","))
        wl.send(message(POINTER, BUTTON, struct.pack("<III", stamp(), BTN_LEFT, 1)))
        frame()
        steps = 8
        for step in range(1, steps + 1):
            move(x + (to_x - x) * step // steps, y + (to_y - y) * step // steps)
        wl.send(message(POINTER, BUTTON, struct.pack("<III", stamp(), BTN_LEFT, 0)))
        frame()

    # Give the compositor a moment to deliver before the socket closes.
    list(wl.check(wl.recv(0.3)))
    print("done: %s" % sorted(advertised))
    return 0


if __name__ == "__main__":
    sys.exit(main())
