#!/bin/sh
# Rust-port parity check for ingest: sweep the same transcript directories
# with the TypeScript and the Rust sweeper into two fresh event logs, and
# diff the logs with the per-run fields (event id, event timestamp) removed.
# Usage: scripts/parity-sweep.sh <claude-projects-dir> <codex-sessions-dir>
# Snapshot the directories first if a harness may be writing to them.
# Prints nothing and exits 0 when both sweeps emit the same events in order.
set -eu

claude=$1
codex=$2
here=$(cd "$(dirname "$0")/.." && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/ts" "$work/rs"

npx --prefix "$here" tsx "$here/scripts/sweep-dirs.ts" "$work/ts" "$claude" "$codex" >/dev/null
(cd "$here/rust" && cargo run -q --example sweep -- "$work/rs" "$claude" "$codex" >/dev/null)

strip() { jq -c 'del(.id, .ts)' "$1"; }
strip "$work/ts/events.jsonl" > "$work/ts.txt"
strip "$work/rs/events.jsonl" > "$work/rs.txt"
diff "$work/ts.txt" "$work/rs.txt"
