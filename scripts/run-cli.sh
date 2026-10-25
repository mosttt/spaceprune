#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."
# Cargo run builds only the selected host, not its separate native programs.
cargo build --locked -p spaceprune-cli -p spaceprune-readonly-helper --bins
exec cargo run --locked -p spaceprune-cli -- "$@"
