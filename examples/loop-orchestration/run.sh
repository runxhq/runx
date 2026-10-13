#!/usr/bin/env sh
# Loop orchestration demo: a local loop host chains ordinary governed runx runs.
# No provider key, no network, no resident kernel loop.
set -eu

HERE="$(cd "$(dirname "$0")" && pwd)"
node "$HERE/loop/loop-host.mjs"
