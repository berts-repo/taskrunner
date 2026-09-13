#!/bin/sh
# Rust-port parity check for the CLI: with a daemon booted on one event log,
# run a fixed list of commands with the TypeScript CLI and then with the Rust
# CLI, capturing stdout, stderr and the exit code, and diff. Covers the
# usage text, the query commands with their flags, error paths, and status
# (minus the pid).
# Usage: scripts/parity-cli.sh <events.jsonl>
# Prints nothing and exits 0 when every command behaves the same.
set -eu

log=$1
here=$(cd "$(dirname "$0")/.." && pwd)
work=$(mktemp -d /tmp/tr-cli.XXXXXX)   # short: unix socket paths are capped
trap 'rm -rf "$work"' EXIT
(cd "$here/rust" && cargo build -q)
cp "$log" "$work/events.jsonl"
chmod 600 "$work/events.jsonl"
printf '[ingest.sources.claude-code]\ndirs = []\n[ingest.sources.codex]\ndirs = []\n' > "$work/config.toml"

task=$(grep -o '"type":"task.created","task_id":"[^"]*"' "$log" | head -1 | sed 's/.*task_id":"//; s/"$//')
turn=$(grep -o '"type":"turn.started","turn_id":"[^"]*"' "$log" | head -1 | sed 's/.*turn_id":"//; s/"$//')
project=$(grep -o '"type":"project.created"[^}]*"root":"[^"]*"' "$log" | head -1 | sed 's/.*root":"//; s/"$//')
session=$(grep -o '"native_session_id":"[^"]*"' "$log" | head -1 | sed 's/.*id":"//; s/"$//')

# Each line is one argv, split on spaces (no argument here contains one).
commands() {
  cat <<CMDS
help
--help
sessions
sessions --limit 3
session $session
session $session --view outline
session $session --view compact --last 5
session $session --prompt 2
session $session --tool-lines 2
session $session --view bogus
session nope
session
search rust --limit 5
search --tool Edit
search --failed true --limit 3
search --target proxy --failed false
search proxy --last-sessions 3 --sort recent
search --limit abc rust
search
task $task
task $task --include turns,trace,audit,artifacts,diff,transcript
task $task --include transcript --view compact --last 2
task $task --include turns --turn $turn
task $task --turn turn_nope
task nope
task
tasks --project $project
tasks --project /nope
doctor
frobnicate
CMDS
}

run_all() {
  cli=$1
  out=$2
  : > "$out"
  commands | while IFS= read -r line; do
    echo "===== $line" >> "$out"
    # shellcheck disable=SC2086
    set +e
    $cli $line --state-root "$work" > "$work/stdout" 2> "$work/stderr"
    code=$?
    set -e
    sed '/^taskrunner daemon .* running (pid /d' "$work/stdout" >> "$out"
    sed 's/^/stderr: /' "$work/stderr" >> "$out"
    echo "[exit $code]" >> "$out"
  done
}

node "$here/dist/cli.js" up --state-root "$work" > /dev/null 2>&1 &
sleep 2
run_all "node $here/dist/cli.js" "$work/ts.txt"
node "$here/dist/cli.js" down --state-root "$work" > /dev/null

"$here/rust/target/debug/taskrunner" up --state-root "$work" > /dev/null 2>&1 &
sleep 2
run_all "$here/rust/target/debug/taskrunner" "$work/rs.txt"
"$here/rust/target/debug/taskrunner" down --state-root "$work" > /dev/null

diff "$work/ts.txt" "$work/rs.txt"
