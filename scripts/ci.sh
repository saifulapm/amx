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

step cargo fmt --all --check
step cargo clippy --workspace --all-targets --all-features -- -D warnings
step cargo test --workspace --all-features
step scripts/check-module-size.sh

printf 'ci: ok\n'
