#!/usr/bin/env bash
# Vendoring and toolchain management for libghostty-vt.
#
#   vendor-libghostty-vt.sh toolchain
#       Download and verify the pinned Zig into vendor/toolchain/. The build
#       script looks there first, so this is the one command a contributor
#       needs when the system Zig is the wrong version.
#
#   vendor-libghostty-vt.sh sync --source-repo <ghostty checkout>
#       Rebuild the source dist from a clean ghostty checkout at the pinned
#       commit, replace vendor/libghostty-vt/, re-apply the local patches and
#       rewrite vendor/libghostty-vt.vendor.json.
#
# `sync` is only run when the pin moves. See vendor/README.md for what the
# build still fetches and why.
set -euo pipefail

# The pinned upstream ghostty commit. Moving it means re-running `sync`,
# refreshing the patch manifest and re-checking the R7 grapheme test.
SOURCE_COMMIT=c5a21edfcbc2d5b46540ad91b7980aca31f5f1f3

# The vendored build.zig.zon declares minimum_zig_version = 0.15.2 and the
# compiler enforces it at comptime; 0.16.0 is rejected by that check.
ZIG_VERSION=0.15.2

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
vendor_dir="$repo_root/vendor"
dest_dir="$vendor_dir/libghostty-vt"
manifest="$vendor_dir/libghostty-vt.vendor.json"
patch_dir="$vendor_dir/patches/libghostty-vt"
toolchain_dir="$vendor_dir/toolchain"

die() {
    printf 'vendor-libghostty-vt: %s\n' "$*" >&2
    exit 1
}

# Scratch directories are tracked globally so the EXIT trap can see them
# whatever function created them.
scratch=""
cleanup() { [ -z "$scratch" ] || rm -rf "$scratch"; }
trap cleanup EXIT

zig_host_triple() {
    local arch os
    case "$(uname -m)" in
    x86_64 | amd64) arch=x86_64 ;;
    arm64 | aarch64) arch=aarch64 ;;
    *) die "no pinned Zig build for $(uname -m)" ;;
    esac
    case "$(uname -s)" in
    Linux) os=linux ;;
    Darwin) os=macos ;;
    *) die "no pinned Zig build for $(uname -s)" ;;
    esac
    printf '%s-%s' "$arch" "$os"
}

# Checksums from https://ziglang.org/download/index.json for ZIG_VERSION.
zig_sha256() {
    case "$1" in
    x86_64-linux) printf 02aa270f183da276e5b5920b1dac44a63f1a49e55050ebde3aecc9eb82f93239 ;;
    aarch64-linux) printf 958ed7d1e00d0ea76590d27666efbf7a932281b3d7ba0c6b01b0ff26498f667f ;;
    x86_64-macos) printf 375b6909fc1495d16fc2c7db9538f707456bfc3373b14ee83fdd3e22b3d43f7f ;;
    aarch64-macos) printf 3cc2bab367e185cdfb27501c4b30b1b0653c28d9f73df8dc91488e66ece5fa6b ;;
    *) die "no pinned Zig checksum for $1" ;;
    esac
}

sha256_of() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | cut -d' ' -f1
    else
        shasum -a 256 "$1" | cut -d' ' -f1
    fi
}

# Path to the pinned compiler, downloading it if it is not there yet.
ensure_toolchain() {
    local triple dir tarball url want got
    triple=$(zig_host_triple)
    dir="$toolchain_dir/zig-$triple-$ZIG_VERSION"
    if [ -x "$dir/zig" ]; then
        printf '%s\n' "$dir/zig"
        return 0
    fi

    mkdir -p "$toolchain_dir"
    tarball="$toolchain_dir/zig-$triple-$ZIG_VERSION.tar.xz"
    url="https://ziglang.org/download/$ZIG_VERSION/zig-$triple-$ZIG_VERSION.tar.xz"
    printf 'downloading %s\n' "$url" >&2
    curl -fsSL -o "$tarball" "$url" || die "download failed: $url"

    want=$(zig_sha256 "$triple")
    got=$(sha256_of "$tarball")
    [ "$want" = "$got" ] || die "checksum mismatch for $tarball: want $want, got $got"

    tar -xf "$tarball" -C "$toolchain_dir"
    rm -f "$tarball"
    [ -x "$dir/zig" ] || die "expected $dir/zig after extracting the tarball"
    printf '%s\n' "$dir/zig"
}

require_clean_checkout() {
    local repo status
    repo=$1
    status=$(git -C "$repo" status --porcelain --untracked-files=all)
    [ -z "$status" ] || die "refusing to vendor from dirty checkout $repo:
$status"
}

cmd_toolchain() {
    local zig
    zig=$(ensure_toolchain)
    "$zig" version
    printf '%s\n' "$zig"
}

cmd_sync() {
    local source_repo=""
    while [ $# -gt 0 ]; do
        case "$1" in
        --source-repo)
            source_repo=${2:-}
            shift 2
            ;;
        *) die "unknown argument: $1" ;;
        esac
    done
    [ -n "$source_repo" ] || die "sync requires --source-repo <ghostty checkout>"
    source_repo=$(cd "$source_repo" && pwd)

    local zig head
    zig=$(ensure_toolchain)
    require_clean_checkout "$source_repo"
    head=$(git -C "$source_repo" rev-parse HEAD)
    [ "$head" = "$SOURCE_COMMIT" ] ||
        die "checkout is at $head, pin is $SOURCE_COMMIT"

    # ghostty stamps VERSION and the archive name with `git log --pretty=%h`,
    # whose abbreviation length follows the object count of the checkout — a
    # partial clone yields a shorter hash than a full one. Pin it through the
    # environment so `sync` produces the same tree from any clone, without
    # writing to the contributor's git config.
    (
        cd "$source_repo"
        GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=core.abbrev GIT_CONFIG_VALUE_0=9 \
            "$zig" build dist -Demit-lib-vt -Doptimize=ReleaseFast
    )
    # The dist step writes into the checkout; a dirty tree afterwards means the
    # archive does not correspond to the recorded commit.
    require_clean_checkout "$source_repo"

    local short archive
    short=${head:0:9}
    archive=$(ls -1 "$source_repo"/zig-out/dist/libghostty-vt-*+"$short".tar.gz | tail -1) ||
        die "no dist archive for $short in $source_repo/zig-out/dist"

    local staging root
    staging=$(mktemp -d)
    scratch=$staging
    tar -xzf "$archive" -C "$staging"
    root=$(ls -1 "$staging")
    [ "$(printf '%s\n' "$root" | wc -l)" = 1 ] ||
        die "expected exactly one archive root in $archive, found: $root"

    rm -rf "$dest_dir"
    mkdir -p "$(dirname "$dest_dir")"
    cp -R "$staging/$root" "$dest_dir"
    chmod -R u+w "$dest_dir"

    local applied=()
    if [ -d "$patch_dir" ]; then
        local patch
        for patch in "$patch_dir"/*.patch; do
            [ -e "$patch" ] || continue
            git -C "$repo_root" apply --verbose "$patch" ||
                die "failed to apply $(basename "$patch") — refresh it against $short"
            applied+=("vendor/patches/libghostty-vt/$(basename "$patch")")
        done
    fi

    local patch_json=""
    if [ ${#applied[@]} -gt 0 ]; then
        # Command substitution eats trailing newlines, so drop the comma the
        # last entry carries and put the newline back.
        patch_json=$(printf '    "%s",\n' "${applied[@]}")
        patch_json="${patch_json%,}"$'\n'
    fi

    cat >"$manifest" <<JSON
{
  "source_commit": "$head",
  "dist_archive": "$(basename "$archive")",
  "extracted_dir": "$root",
  "zig_version": "$ZIG_VERSION",
  "patches": [
$patch_json  ],
  "zig_dependency_fetch": "network: zig build resolves the whole declared dependency graph (~347 MB) before compiling, so the resolved set is not committed; see vendor/README.md"
}
JSON
    printf 'vendored %s from %s into %s\n' "$root" "$head" "$dest_dir"
}

case "${1:-}" in
toolchain)
    shift
    cmd_toolchain "$@"
    ;;
sync)
    shift
    cmd_sync "$@"
    ;;
*)
    sed -n '2,17p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
    exit 1
    ;;
esac
