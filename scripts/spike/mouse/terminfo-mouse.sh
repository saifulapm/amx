#!/usr/bin/env bash
# X01: what the terminfo database says each installed terminal does with the
# mouse.
#
# `XM` is the extended capability that carries the *enable/disable* string a
# terminal wants for mouse tracking (`%p1%{1}%=%t...h%e...l%;` — the parameter
# is on/off), and `xm` is the report *grammar* the terminal will send back.
# ncurses ships these for every terminal that has them, so they are a
# machine-readable statement of the modes each emulator implements, readable on
# a headless box.
#
# This is evidence about what a terminal's *database entry* claims, which is
# not the same thing as watching a byte arrive. It is recorded as such in
# docs/notes/m4-mouse-path.md.
#
# Usage: terminfo-mouse.sh [term ...]   (default: the list below)
set -uo pipefail

terms=("$@")
if [ "${#terms[@]}" -eq 0 ]; then
    terms=(
        foot foot-extra
        alacritty alacritty-direct
        xterm xterm-256color
        tmux tmux-256color
        screen screen-256color
        kitty wezterm
        vte-256color linux
    )
fi

for term in "${terms[@]}"; do
    if ! infocmp -1x "$term" >/dev/null 2>&1; then
        printf '%-18s ABSENT from the terminfo database\n' "$term"
        continue
    fi
    printf '%-18s present\n' "$term"
    infocmp -1x "$term" 2>/dev/null |
        grep -E '^[[:space:]]*(XM|xm|XR|xr|kmous)=' |
        sed 's/^[[:space:]]*/    /'
done
