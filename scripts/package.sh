#!/usr/bin/env bash
# Packages gridterm.app for a GitHub release. Checks that the tag matches
# the workspace version in Cargo.toml, builds the bundle and zips it with
# ditto (which keeps the code signature). Prints the path of the zip.
#
#   scripts/package.sh v0.1.0           check + build + zip
#   scripts/package.sh --check v0.1.0   only check the tag
set -euo pipefail
cd "$(dirname "$0")/.."

CHECK_ONLY=0
if [[ "${1-}" == "--check" ]]; then
  CHECK_ONLY=1
  shift
fi
TAG="${1:?usage: scripts/package.sh [--check] vX.Y.Z}"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
if [[ -z "$VERSION" ]]; then
  echo "error: no version found in Cargo.toml" >&2
  exit 1
fi
if [[ "$TAG" != "v$VERSION" ]]; then
  echo "error: tag '$TAG' does not match the version in Cargo.toml ($VERSION); expected 'v$VERSION'" >&2
  exit 1
fi
[[ "$CHECK_ONLY" == 1 ]] && exit 0

scripts/bundle.sh >&2
ZIP="target/release/gridterm-$VERSION-$(uname -m).zip"
rm -f "$ZIP"
ditto -c -k --keepParent target/release/bundle/gridterm.app "$ZIP"
echo "$ZIP"
