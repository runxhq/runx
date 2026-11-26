#!/usr/bin/env bash
# Pack the TypeScript packages a CLI release ships beside the CLI: the
# contracts and the client its hosts build on. pnpm resolves their workspace
# dependencies to the packed versions. Named packages, as a package's own
# tag names one, are packed in their place.
set -euo pipefail

destination="$1"
shift
packages=("$@")
[ "${#packages[@]}" -gt 0 ] || packages=(contracts client)
node scripts/build-workspace.mjs --pack
mkdir -p "$destination"
for package in "${packages[@]}"; do
  (cd "packages/$package" && pnpm pack --pack-destination "$destination")
done
