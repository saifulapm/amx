#!/usr/bin/env bash
# X01, observation 4: turn a wheel in a real terminal emulator and watch what
# arrives.
#
# This is the question the whole spike is named for. It needs three things at
# once — a real emulator, a real wheel event, and a program reading the tty —
# and a box with nobody sitting at it can still have all three: a wlroots-style
# compositor will create a pointer device on request, and the emulator cannot
# tell it from a hand.
#
# Two runs, and the difference between them is the finding:
#
#   baseline  the probe asks the terminal for nothing, exactly as `amx attach`
#             does today, and the wheel is turned anyway.
#   sgr       the probe asks for ?1006 and ?1000 and the wheel is turned again.
#
# Requires: a running wlroots-style compositor advertising
# zwlr_virtual_pointer_manager_v1 (checked, and reported plainly if absent).
#
# Usage: wheel-in-emulator.sh <path-to-mouse_probe> [emulator]
set -uo pipefail

probe="${1:?usage: wheel-in-emulator.sh <path-to-mouse_probe> [emulator]}"
emulator="${2:-foot}"
probe="$(cd "$(dirname "$probe")" && pwd)/$(basename "$probe")"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

if ! command -v "$emulator" >/dev/null 2>&1; then
    printf 'wheel-in-emulator: %s is not installed\n' "$emulator" >&2
    exit 2
fi
if [ -z "${WAYLAND_DISPLAY:-}" ]; then
    printf 'wheel-in-emulator: no WAYLAND_DISPLAY; run this from a Wayland session\n' >&2
    exit 2
fi

# A locked session routes every pointer event to the lock surface and none to
# an ordinary window, so this run would produce an empty transcript that looks
# exactly like "the emulator sends nothing". It is not that, and the run says
# so rather than letting the reader draw the wrong conclusion.
session="$(loginctl show-session self -p Id --value 2>/dev/null)"
if [ -z "$session" ]; then
    session="$(loginctl list-sessions --no-legend 2>/dev/null |
        awk '$4 == "seat0" {print $1; exit}')"
fi
if [ -n "$session" ] &&
    [ "$(loginctl show-session "$session" -p LockedHint --value 2>/dev/null)" = "yes" ]; then
    printf 'wheel-in-emulator: session %s is locked (LockedHint=yes).\n' "$session" >&2
    printf '  A lock surface takes every pointer event, so nothing would reach\n' >&2
    printf '  the emulator and the empty result would be indistinguishable from\n' >&2
    printf '  "no report was sent". Unlock the session and run this again.\n' >&2
    exit 3
fi

# The pointer is driven in absolute coordinates over the output's extent, so
# the run needs to know how big the output is. niri reports it; anything else
# can be passed through the environment.
extent="${AMX_SPIKE_EXTENT:-}"
if [ -z "$extent" ] && command -v niri >/dev/null 2>&1; then
    extent="$(niri msg outputs 2>/dev/null |
        awk '/Logical size:/ {print $3; exit}' | tr 'x' ',')"
fi
extent="${extent:-1920,1080}"
centre="$(printf '%s' "$extent" | awk -F, '{printf "%d,%d", $1/2, $2/2}')"
printf 'output extent %s, pointer at %s\n\n' "$extent" "$centre"

out="$(mktemp -d)"
trap 'rm -rf "$out"' EXIT

launch() {
    local label="$1"
    local modes="$2"
    local log="$out/$label.log"

    # Each emulator spells "fullscreen, and this title" its own way; the
    # window has to be big and findable, and neither flag is portable.
    case "$emulator" in
        foot)
            "$emulator" --fullscreen --title=amx-x01-wheel \
                "$probe" --modes "$modes" --seconds 14 --alt --log "$log" &
            ;;
        alacritty)
            "$emulator" --title amx-x01-wheel \
                -o 'window.startup_mode="Fullscreen"' \
                -e "$probe" --modes "$modes" --seconds 14 --alt --log "$log" &
            ;;
        *)
            "$emulator" -e "$probe" --modes "$modes" --seconds 14 --alt --log "$log" &
            ;;
    esac
    local emu=$!

    # Wait on the probe announcing itself, never on a fixed nap.
    local waited=0
    while [ ! -s "$log" ] || ! grep -q listening "$log"; do
        if [ "$waited" -ge 200 ]; then
            printf '   the probe never announced itself inside %s\n' "$emulator"
            kill "$emu" 2>/dev/null
            wait "$emu" 2>/dev/null
            return 1
        fi
        command sleep 0.05
        waited=$((waited + 1))
    done

    printf '== %s: modes requested "%s" ==\n' "$label" "$modes"
    python3 "$here/virtual-pointer.py" \
        --at "$centre" --extent "$extent" \
        --scroll-up 2 --scroll-down 2 --click >/dev/null || {
        printf '   the virtual pointer failed\n'
        kill "$emu" 2>/dev/null
        wait "$emu" 2>/dev/null
        return 1
    }
    wait "$emu" 2>/dev/null
    sed 's/^/   /' "$log"
    printf '\n'
}

launch baseline ""
launch sgr "1006,1000"
