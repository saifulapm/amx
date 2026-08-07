#!/bin/sh
# amx's reference notifier: one desktop notification per agent that needs you.
#
# amx has exactly one built-in notify path — the OSC 9 escape the client writes
# to its host terminal. Everything out of the terminal is an extension: a
# program you run and supervise yourself, reading the same event stream every
# other consumer reads (docs/03-vision.md §4, docs/04-architecture.md §8). This
# is that program, in full. Run it beside your session:
#
#     examples/notify.sh &
#
# It needs `amx` on $PATH and a session to watch: $AMX_SESSION names it, or add
# `--session <name>` to the two amx calls below.
#
# To adapt it: change notify() for a different notifier, or widen the first
# case arm to other events — `amx events --json` carries every transition amx
# publishes, one JSON object per line.
set -eu

# How you are told. The fallback prints, so this script is never silent about
# something it saw.
notify() {
    if command -v notify-send >/dev/null 2>&1; then
        notify-send 'amx' "$1 needs input"
    elif command -v osascript >/dev/null 2>&1; then
        osascript -e "display notification \"$1 needs input\" with title \"amx\""
    else
        printf 'amx: %s needs input\n' "$1"
    fi
}

# The pane a delivery names. The lines are compact JSON, one per line, so a
# field is a substring — no JSON parser required for a stream this narrow.
pane_of() {
    printf '%s\n' "$1" | sed -n 's/.*"pane":"\([0-9a-f-]*\)".*/\1/p'
}

# Every pane on the attention queue right now, in queue order. This is the same
# queue the status line counts and `amx agent next` walks: `session.state` is
# the query, so a consumer that lost events can always ask for the truth.
waiting_now() {
    amx session state --params '{}' |
        sed -n '/"attention"/,/]/p' |
        sed -n 's/.*"\([0-9a-f-]\{36\}\)".*/\1/p'
}

amx events --json | while IFS= read -r line; do
    case $line in
        *'"event":"attention_enqueued"'*)
            notify "pane $(pane_of "$line")"
            ;;
        *'"delivery":"gap"'*)
            # Deliveries were dropped: this consumer fell behind the server's
            # replay buffer. The events are gone, the state they described is
            # not, so re-query and notify for whoever is waiting now. A gap
            # costs you a repeated notification, never a missed one — which is
            # the trade the bus's gap contract exists to let you make.
            waiting_now | while IFS= read -r pane; do
                notify "pane $pane"
            done
            ;;
    esac
done
