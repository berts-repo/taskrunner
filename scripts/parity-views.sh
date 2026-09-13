#!/bin/sh
# Rust-port parity check for the read side: fold one event log with the
# TypeScript index, render a fixed set of views (session list, outlines,
# compact and timeline reads, searches, task lookups) with both the
# TypeScript and the Rust renderers, and diff the text.
# Usage: scripts/parity-views.sh <events.jsonl>
# Prints nothing and exits 0 when every view renders byte for byte the same.
set -eu

log=$1
here=$(cd "$(dirname "$0")/.." && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

npx --prefix "$here" tsx "$here/scripts/debug-refold.ts" "$log" "$work/index.db" >/dev/null
npx --prefix "$here" tsx "$here/scripts/render-views.ts" "$work/index.db" > "$work/ts.txt"
(cd "$here/rust" && cargo run -q --example render -- "$work/index.db") > "$work/rs.txt"
diff "$work/ts.txt" "$work/rs.txt"
