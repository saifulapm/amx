#!/usr/bin/env bash
# X01, observation 3: interrogate a real terminal emulator without a pointer.
#
# The question this closes is the one a headless box otherwise cannot: does an
# emulator actually implement the modes amx would request, and what are its
# defaults? DECRQM (`CSI ? Ps $ p`) makes the emulator answer in writing, and
# answering needs no mouse — the window opens, replies, and closes on a timer.
#
# What this does NOT observe is a wheel event producing a report: that needs a
# pointer, and the by-hand procedure at the end of docs/notes/m4-mouse-path.md
# is what closes it.
#
# Each emulator is launched with the probe as its command, writing a log to a
# path this script then prints. The window lives for --seconds and closes.
#
# Usage: emulator-decrqm.sh <path-to-mouse_probe> [seconds]
set -uo pipefail

probe="${1:?usage: emulator-decrqm.sh <path-to-mouse_probe> [seconds]}"
seconds="${2:-3}"
probe="$(cd "$(dirname "$probe")" && pwd)/$(basename "$probe")"

# 1007 first, because its *default* is the interesting answer: a terminal that
# reports it already set is translating the wheel to arrow keys on the
# alternate screen with nothing requested of it.
modes="1007,1006,1000,1002"

out="$(mktemp -d)"
trap 'rm -rf "$out"' EXIT

run() {
    local name="$1"
    shift
    if ! command -v "$name" >/dev/null 2>&1; then
        printf '== %s: not installed ==\n\n' "$name"
        return
    fi
    local log="$out/$name.log"
    printf '== %s ==\n' "$name"
    printf '   %s\n' "$("$name" --version 2>&1 | head -1)"
    # The window opens, the probe writes its queries, and both go away when
    # the probe's own deadline expires. Nothing here waits on the wall clock
    # for a *result*: the command returns when the emulator exits.
    if ! "$@" >"$out/$name.stdout" 2>"$out/$name.stderr"; then
        printf '   launch failed:\n'
        sed 's/^/     /' "$out/$name.stderr" | head -10
        printf '\n'
        return
    fi
    if [ ! -s "$log" ]; then
        printf '   no transcript: the emulator started but the probe wrote nothing\n\n'
        return
    fi
    sed 's/^/   /' "$log"
    printf '\n'
}

args=(--modes "$modes" --seconds "$seconds" --query --alt)

run foot foot --title=amx-x01-mouse-spike \
    "$probe" "${args[@]}" --log "$out/foot.log"

run alacritty alacritty --title amx-x01-mouse-spike \
    -e "$probe" "${args[@]}" --log "$out/alacritty.log"

run kitty kitty --title amx-x01-mouse-spike \
    "$probe" "${args[@]}" --log "$out/kitty.log"

run xterm xterm -title amx-x01-mouse-spike \
    -e "$probe" "${args[@]}" --log "$out/xterm.log"
