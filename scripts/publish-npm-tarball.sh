#!/usr/bin/env bash
# Publish one packed npm tarball, once. A version already on the registry must
# be these exact bytes; a published one is read back until the registry
# serves the same integrity.
set -euo pipefail

tarball="$1"
manifest=$(tar -xOzf "$tarball" package/package.json)
name=$(node -e 'process.stdout.write(JSON.parse(process.argv[1]).name)' "$manifest")
version=$(node -e 'process.stdout.write(JSON.parse(process.argv[1]).version)' "$manifest")
integrity=$(node -e '
  const { createHash } = require("node:crypto");
  const { readFileSync } = require("node:fs");
  process.stdout.write(`sha512-${createHash("sha512").update(readFileSync(process.argv[1])).digest("base64")}`);
' "$tarball")

if published=$(npm view "${name}@${version}" dist.integrity --json --prefer-online 2>/dev/null); then
  actual=$(node -e 'process.stdout.write(JSON.parse(process.argv[1]))' "$published")
  if [ "$actual" != "$integrity" ]; then
    echo "::error::${name}@${version} exists with different tarball integrity"
    exit 1
  fi
  echo "${name}@${version} already matches the packed candidate"
else
  errors=$(mktemp)
  status=0
  npm publish "$tarball" --access public --tag latest 2>"$errors" || status=$?
  cat "$errors" >&2
  # npm processes a new version for minutes before serving it, so an earlier
  # attempt's publish can still be staged; the readback proves its bytes.
  if [ "$status" -ne 0 ] && ! grep -q 'previously staged version' "$errors"; then
    exit "$status"
  fi
fi

for attempt in $(seq 1 60); do
  observed=$(npm view "${name}@${version}" dist.integrity --json --prefer-online 2>/dev/null || true)
  if [ "$observed" = "\"$integrity\"" ]; then
    exit 0
  fi
  sleep 10
done
echo "::error::${name}@${version} registry integrity readback did not match the candidate"
exit 1
