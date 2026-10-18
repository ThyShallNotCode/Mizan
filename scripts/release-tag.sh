#!/usr/bin/env bash
# Checks the tag of an unsigned CI pre-release (.github/workflows/release-build.yml) and prints
# its version. The tag must be v<workspace version>-<pre-release>, for example v0.1.0-preview.1;
# plain v<version> tags are left to the signed releases of the release enclave (spec §24.2).
set -euo pipefail
cd "$(dirname "$0")/.."
tag=${1:?usage: scripts/release-tag.sh <tag>}
version=$(sed -n '/^\[workspace\.package\]/,/^\[/s/^version = "\([^"]*\)"$/\1/p' Cargo.toml)
if [ -z "$version" ]; then
  echo "release-tag.sh: no version in [workspace.package] of Cargo.toml" >&2
  exit 1
fi
if [[ ! $tag =~ ^v([0-9]+\.[0-9]+\.[0-9]+)-([0-9A-Za-z]+(\.[0-9A-Za-z]+)*)$ ]]; then
  echo "release-tag.sh: '$tag' is not v<version>-<pre-release>, for example v$version-preview.1" >&2
  exit 1
fi
if [ "${BASH_REMATCH[1]}" != "$version" ]; then
  echo "release-tag.sh: '$tag' does not match the workspace version $version" >&2
  exit 1
fi
echo "${tag#v}"
