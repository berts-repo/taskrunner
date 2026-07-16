#!/bin/sh
# Builds the taskrunner worker and egress proxy images (npm run build:images).
set -eu

cd "$(dirname "$0")/.."

docker build -t taskrunner/egress-proxy docker/egress-proxy
docker build -t taskrunner/codex-worker docker/codex-worker
docker build -t taskrunner/claude-worker docker/claude-worker

echo "built: taskrunner/egress-proxy taskrunner/codex-worker taskrunner/claude-worker"
