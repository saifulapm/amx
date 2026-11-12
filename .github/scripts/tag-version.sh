#!/bin/sh
# Refuses a release tag that is not Cargo.toml's package version, so a
# release never names a version its binary does not report. The release
# workflow runs it before any build, and a person can run it by hand:
#
#   .github/scripts/tag-version.sh v0.1.3
set -eu

tag=${1:?usage: tag-version.sh <tag>}
toml="$(dirname "$0")/../../Cargo.toml"

# The version under [package], never a dependency's.
version=$(awk '
  /^[[:space:]]*\[/ { package = ($0 ~ /^[[:space:]]*\[package\][[:space:]]*$/); next }
  package && /^[[:space:]]*version[[:space:]]*=/ {
    sub(/^[^"]*"/, ""); sub(/".*$/, ""); print; exit
  }
' "$toml")

if [ "$tag" != "v$version" ]; then
  echo "tag $tag is not Cargo.toml's $version" >&2
  exit 1
fi
