#!/usr/bin/env bash
# The single source of truth for CI: the workflow calls this and nothing else,
# so `scripts/ci.sh` locally is exactly what the workflow runs.
#
# AMX_CI_DRY_RUN=1 prints the step list without executing it — that is how the
# acceptance test inspects the pipeline without recursing into cargo test.
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

export AMX_CI=1

step() {
    printf '=== %s\n' "$*"
    if [ "${AMX_CI_DRY_RUN:-0}" = "1" ]; then
        return 0
    fi
    "$@"
}

# Tier 2 of the SSH bridge (D-M3-9): the loopback-sshd suite runs inside the
# ordinary test step, admitted by this variable. Linux only, because darwin
# runners cannot host an sshd a test can start unprivileged (R-M3-6), and only
# when one is installed — `tests/remote_ssh.rs` prints its reason and skips
# either way, so a machine without it loses the transport tier and nothing else.
if [ "$(uname -s)" = "Linux" ] && [ -x /usr/sbin/sshd ]; then
    export AMX_TEST_SSHD=1
    printf '=== loopback sshd: enabled (tier 2)\n'
else
    printf '=== loopback sshd: skipped on %s (R-M3-6)\n' "$(uname -s)"
fi

step cargo fmt --all --check
step cargo clippy --workspace --all-targets --all-features -- -D warnings
step cargo test --workspace --all-features
step scripts/check-module-size.sh

printf 'ci: ok\n'
