#!/bin/sh
# Install amx from a GitHub release.
#
#   curl -fsSL https://saifulapm.github.io/amx/install.sh | sh
#
# AMX_VERSION     release tag to install, e.g. v0.1.0 (default: latest)
# AMX_INSTALL_DIR where the binary goes (default: ~/.local/bin)

set -eu

repo="saifulapm/amx"
version="${AMX_VERSION:-latest}"
dir="${AMX_INSTALL_DIR:-$HOME/.local/bin}"

say() { printf '%s\n' "$*"; }
die() { printf 'amx install: %s\n' "$*" >&2; exit 1; }

case "$(uname -s)" in
  Linux) os="unknown-linux-musl" ;;
  Darwin) os="apple-darwin" ;;
  *) die "no prebuilt binary for $(uname -s); build from source with: cargo install --git https://github.com/$repo" ;;
esac

case "$(uname -m)" in
  x86_64 | amd64) arch="x86_64" ;;
  aarch64 | arm64) arch="aarch64" ;;
  *) die "no prebuilt binary for $(uname -m); build from source with: cargo install --git https://github.com/$repo" ;;
esac

# A shell running under Rosetta reports x86_64 on Apple silicon.
if [ "$os" = "apple-darwin" ] && [ "$arch" = "x86_64" ] &&
  [ "$(sysctl -n sysctl.proc_translated 2>/dev/null)" = "1" ]; then
  arch="aarch64"
fi

asset="amx-$arch-$os.tar.gz"
if [ "$version" = "latest" ]; then
  url="https://github.com/$repo/releases/latest/download/$asset"
else
  url="https://github.com/$repo/releases/download/$version/$asset"
fi

if command -v curl >/dev/null 2>&1; then
  fetch() { curl -fsSL "$1" -o "$2"; }
elif command -v wget >/dev/null 2>&1; then
  fetch() { wget -qO "$2" "$1"; }
else
  die "need curl or wget"
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

say "Downloading $asset ($version)"
fetch "$url" "$tmp/$asset" || die "download failed: $url"
fetch "$url.sha256" "$tmp/$asset.sha256" || die "download failed: $url.sha256"

expected="$(cut -d ' ' -f 1 "$tmp/$asset.sha256")"
if command -v sha256sum >/dev/null 2>&1; then
  actual="$(sha256sum "$tmp/$asset" | cut -d ' ' -f 1)"
else
  actual="$(shasum -a 256 "$tmp/$asset" | cut -d ' ' -f 1)"
fi
[ "$expected" = "$actual" ] || die "checksum mismatch for $asset"

tar -xzf "$tmp/$asset" -C "$tmp"
mkdir -p "$dir"
install -m 755 "$tmp/amx-$arch-$os/amx" "$dir/amx"
say "Installed $("$dir/amx" --version) to $dir/amx"

case ":$PATH:" in
  *":$dir:"*) ;;
  *)
    say ""
    say "$dir is not on your PATH. Add it in your shell profile:"
    say "  export PATH=\"$dir:\$PATH\""
    ;;
esac

if ! command -v tmux >/dev/null 2>&1; then
  say ""
  say "amx needs tmux 3.2 or newer, and tmux is not installed."
fi

say ""
say "Next:"
say "  amx setup claude   # or pi, codex, opencode"
say "  amx doctor"
