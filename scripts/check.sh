#!/usr/bin/env bash
# Full local gate: formatting, lints, tests and architecture rules (docs/IMPLEMENTATION.md §4).
set -euo pipefail
cd "$(dirname "$0")/.."
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo test -p omr-archtest
echo "check.sh: all gates passed"
