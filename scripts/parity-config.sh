#!/bin/sh
# Rust-port parity check for config: load every sample under
# tests/fixtures/config with both implementations, print the result as JSON
# (or ERROR), and diff. Keys and defaults are frozen, so the JSON must match.
# Usage: scripts/parity-config.sh
set -eu

here=$(cd "$(dirname "$0")/.." && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
(cd "$here/rust" && cargo build -q --example print_config)

for file in "$here"/tests/fixtures/config/*.toml; do
  echo "===== $(basename "$file")"
  npx --prefix "$here" tsx "$here/scripts/print-config.ts" "$file"
done > "$work/ts.txt"
for file in "$here"/tests/fixtures/config/*.toml; do
  echo "===== $(basename "$file")"
  "$here/rust/target/debug/examples/print_config" "$file"
done > "$work/rs.txt"
diff "$work/ts.txt" "$work/rs.txt"
