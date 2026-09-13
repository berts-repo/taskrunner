#!/bin/sh
# Rust-port parity check for the MCP tool contract: list the tools through
# the TypeScript shim and through the Rust shim with the same client, and
# diff names, descriptions and input schemas. The SDK-generated `execution`
# entry is not part of the contract and is dropped before comparing.
# Usage: scripts/parity-tools.sh
set -eu

here=$(cd "$(dirname "$0")/.." && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
(cd "$here/rust" && cargo build -q)
printf '[ingest.sources.claude-code]\ndirs = []\n[ingest.sources.codex]\ndirs = []\n' > "$work/config.toml"

list() {
  npx --prefix "$here" tsx "$here/scripts/tools-list.ts" "$@" mcp --state-root "$work" |
    jq 'map(del(.execution))'
}
list node "$here/dist/cli.js" > "$work/ts.json"
node "$here/dist/cli.js" down --state-root "$work" > /dev/null
rm -rf "$work/runtime"
list "$here/rust/target/debug/taskrunner" > "$work/rs.json"
"$here/rust/target/debug/taskrunner" down --state-root "$work" > /dev/null
diff "$work/ts.json" "$work/rs.json"
