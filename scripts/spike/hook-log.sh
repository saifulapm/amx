#!/usr/bin/env bash
# One logging command for every lifecycle event of the spike's scratch project.
# Appends one JSON line per invocation to the log named on the command line.
#
#   hook-log.sh <tag> <logfile>
#
# The log path is an *argument*, never an environment variable: whether the
# agent's hook processes inherit the launching terminal's environment is the
# thing under test (M6), so the logger must work when the answer is "no".
#
# $EPOCHREALTIME is a bash builtin, so the timestamp costs no fork and is taken
# before anything else runs — it is the earliest a shell hook can stamp itself.
ts=$EPOCHREALTIME
tag=$1
log=$2
[ -n "$log" ] || exit 0

payload=$(cat)

# Every AMX_* variable the identity scheme plants, plus the few ambient ones
# worth knowing about. Absent variables simply do not appear in the object.
env_json=$(env | grep -aE '^(AMX_[A-Z_]*|TERM|SHLVL|CLAUDECODE|CLAUDE_CODE_ENTRYPOINT|USER|PWD)=' |
	jq -Rs 'split("\n") | map(select(length > 0) | split("=") | {key: .[0], value: (.[1:] | join("="))}) | from_entries')

# Ancestry, read straight out of /proc with builtins (no forks, no ps): the
# chain from this hook process up to the agent that spawned it.
tree=()
p=$$
for _ in 1 2 3 4 5; do
	[ -r "/proc/$p/comm" ] || break
	read -r comm <"/proc/$p/comm"
	tree+=("$p:$comm")
	ppid=""
	while read -r k v; do
		if [ "$k" = "PPid:" ]; then
			ppid=$v
			break
		fi
	done <"/proc/$p/status"
	[ -n "$ppid" ] && [ "$ppid" != 0 ] || break
	p=$ppid
done
tree_json=$(printf '%s\n' "${tree[@]}" | jq -Rs 'split("\n") | map(select(length > 0))')

if [ -n "$payload" ] && [ "${payload:0:1}" = "{" ]; then
	line=$(jq -cn --argjson ts "$ts" --arg tag "$tag" --argjson pid "$$" \
		--argjson env "$env_json" --argjson tree "$tree_json" --argjson payload "$payload" \
		'{ts: $ts, tag: $tag, pid: $pid, env: $env, tree: $tree, payload: $payload}' 2>/dev/null)
fi
if [ -z "$line" ]; then
	# Non-JSON or unparsable stdin is itself a finding: keep the bytes verbatim.
	line=$(jq -cn --argjson ts "$ts" --arg tag "$tag" --argjson pid "$$" \
		--argjson env "$env_json" --argjson tree "$tree_json" --arg raw "$payload" \
		'{ts: $ts, tag: $tag, pid: $pid, env: $env, tree: $tree, payload: null, raw: $raw}')
fi

# One write(2) of one line: hooks run in parallel, and O_APPEND keeps
# concurrent writers from interleaving as long as each line is a single write.
printf '%s\n' "$line" >>"$log"
exit 0
