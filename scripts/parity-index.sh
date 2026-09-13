#!/bin/sh
# Rust-port parity check for the storage layer: fold one event log with the
# TypeScript index and with the Rust index, dump every table of both with the
# sqlite3 CLI (a witness neither implementation controls), and diff.
# Usage: scripts/parity-index.sh <events.jsonl>
# Prints nothing and exits 0 when the two indexes match row for row.
set -eu

log=$1
here=$(cd "$(dirname "$0")/.." && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

npx --prefix "$here" tsx "$here/scripts/debug-refold.ts" "$log" "$work/ts.db" >/dev/null
(cd "$here/rust" && cargo run -q --example refold -- "$log" "$work/rs.db" >/dev/null)

# One JSON line per row, ordered by every column so the dump is deterministic.
# FTS5 shadow tables (messages_fts_*) are internal to the SQLite build and are
# skipped; messages_fts itself is dumped through its declared columns.
dump() {
  db=$1
  echo "user_version=$(sqlite3 "$db" 'PRAGMA user_version')"
  sqlite3 "$db" "SELECT name FROM sqlite_master
                 WHERE type IN ('table') AND name NOT LIKE 'messages_fts_%'
                   AND name NOT LIKE 'sqlite_%' ORDER BY name" |
  while read -r table; do
    columns=$(sqlite3 "$db" "SELECT group_concat(name, ',') FROM pragma_table_info('$table')")
    echo "== $table"
    sqlite3 -json "$db" "SELECT $columns FROM \"$table\" ORDER BY $columns"
  done
}

dump "$work/ts.db" > "$work/ts.dump"
dump "$work/rs.db" > "$work/rs.dump"
diff "$work/ts.dump" "$work/rs.dump"
