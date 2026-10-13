#!/usr/bin/env bash
# Pack the TypeScript packages a CLI release ships beside the CLI: the
# contracts and the client its hosts build on. pnpm resolves their workspace
# dependencies to the packed versions.
set -euo pipefail

destination="$1"
node scripts/build-workspace.mjs --pack
mkdir -p "$destination"
for package in contracts client; do
  (cd "packages/$package" && pnpm pack --pack-destination "$destination")
done
