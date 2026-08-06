#!/usr/bin/env bash
# Module size budget: soft 500 lines (warn), hard 1000 lines (fail).
#
# Generated code is exempt: bindgen output, anything produced into OUT_DIR, and
# the vendored trees. The budget is an architectural forcing function — a file
# that wants to grow past it gets split by responsibility, not raised.
#
# Usage: check-module-size.sh [root]   (default: the repository root)
set -euo pipefail

root="${1:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
soft="${AMX_MODULE_SOFT:-500}"
hard="${AMX_MODULE_HARD:-1000}"

if [ ! -d "$root" ]; then
    printf 'check-module-size: no such directory: %s\n' "$root" >&2
    exit 2
fi

# Paths that are generated or vendored, matched against the root-relative path.
is_exempt() {
    case "$1" in
        crates/amx-vt/src/sys.rs) return 0 ;;   # include!(OUT_DIR/bindings.rs)
        */bindings.rs) return 0 ;;              # bindgen output, wherever it lands
        *) return 1 ;;
    esac
}

failed=0
warned=0

while IFS= read -r file; do
    rel="${file#"$root"/}"
    is_exempt "$rel" && continue

    lines=$(wc -l <"$file")
    lines=$((lines))

    if [ "$lines" -gt "$hard" ]; then
        printf 'error: %s is %d lines (hard budget %d)\n' "$rel" "$lines" "$hard" >&2
        failed=$((failed + 1))
    elif [ "$lines" -gt "$soft" ]; then
        printf 'warning: %s is %d lines (soft budget %d)\n' "$rel" "$lines" "$soft" >&2
        warned=$((warned + 1))
    fi
done < <(
    find "$root" \
        \( -path '*/target' -o -path '*/vendor' -o -path '*/.git' -o -name 'OUT_DIR' \) -prune -o \
        -name '*.rs' -type f -print | sort
)

if [ "$failed" -gt 0 ]; then
    printf 'check-module-size: %d file(s) over the hard budget of %d lines\n' "$failed" "$hard" >&2
    exit 1
fi

printf 'check-module-size: ok (%d over the soft budget of %d lines)\n' "$warned" "$soft"
