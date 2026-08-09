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
#
# It makes **no follow-up query** for the agent it names. `attention_enqueued`
# carries the identity block docs/10-attention-surfaces.md §D15 put on it —
# workspace, agent name, reason — so the notification reads "api/backend blocked
# (permission_dialog)" from the delivery alone. The one query below is the gap
# path, where by definition the deliveries that carried those names are gone.
set -eu

# How you are told. The fallback prints, so this script is never silent about
# something it saw.
notify() {
    if command -v notify-send >/dev/null 2>&1; then
        notify-send 'amx' "$1"
    elif command -v osascript >/dev/null 2>&1; then
        osascript -e "display notification \"$1\" with title \"amx\""
    else
        printf 'amx: %s\n' "$1"
    fi
}

# One string value out of a delivery. The lines are compact JSON, one object per
# line, so a field is a substring — no JSON parser required for a stream this
# narrow.
field() {
    printf '%s\n' "$2" | sed -n "s/.*\"$1\":\"\([^\"]*\)\".*/\1/p"
}

# The nested workspace object, and the delivery with that object removed.
#
# `"name"` appears twice on an enqueue — the workspace's label and the agent's —
# and a substring match cannot tell them apart. So the inner object is lifted out
# and each name is read against a string holding exactly one of them.
workspace_of() {
    printf '%s\n' "$1" | sed -n 's/.*"workspace":{\([^}]*\)}.*/\1/p'
}
outer_of() {
    printf '%s\n' "$1" | sed 's/"workspace":{[^}]*},*//'
}

# What to say about an enqueue, from the enqueue alone.
#
# Every field of the identity block is optional on the wire — the hub sends what
# it has seen and never invents a label (docs/11-m4-plan.md D-M4-6) — so this
# degrades a field at a time: `api/backend`, then `backend`, then the pane's
# uuid, which is all a pre-M4 server ever sent.
describe() {
    outer=$(outer_of "$1")
    project=$(field name "$(workspace_of "$1")")
    agent=$(field name "$outer")
    reason=$(field reason "$outer")

    if [ -n "$agent" ] && [ -n "$project" ]; then
        who="$project/$agent"
    elif [ -n "$agent" ]; then
        who=$agent
    else
        who="pane $(field pane "$outer")"
    fi
    if [ -n "$reason" ]; then
        printf '%s blocked (%s)\n' "$who" "$reason"
    else
        printf '%s needs input\n' "$who"
    fi
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
            notify "$(describe "$line")"
            ;;
        *'"delivery":"gap"'*)
            # Deliveries were dropped: this consumer fell behind the server's
            # replay buffer. The events are gone, the state they described is
            # not, so re-query and notify for whoever is waiting now. A gap
            # costs you a repeated notification, never a missed one — which is
            # the trade the bus's gap contract exists to let you make.
            #
            # This is the one path that queries, and it is the one path where
            # the identity block cannot help: the deliveries that carried the
            # names are exactly what was lost. `session.state`'s queue is panes,
            # so a gap notification names panes.
            waiting_now | while IFS= read -r pane; do
                notify "pane $pane needs input"
            done
            ;;
    esac
done
