#!/usr/bin/env bash
# Publish one packed npm tarball, once. A version already on the registry must
# be these exact bytes; a published one is read back until the registry
# serves the same integrity. With --check, only refuse a version the registry
# holds with other bytes, and publish nothing.
set -euo pipefail

check=false
if [ "${1:-}" = "--check" ]; then
  check=true
  shift
fi
tarball="$1"
manifest=$(tar -xOzf "$tarball" package/package.json)
name=$(node -e 'process.stdout.write(JSON.parse(process.argv[1]).name)' "$manifest")
version=$(node -e 'process.stdout.write(JSON.parse(process.argv[1]).version)' "$manifest")
integrity=$(node -e '
  const { createHash } = require("node:crypto");
  const { readFileSync } = require("node:fs");
  process.stdout.write(`sha512-${createHash("sha512").update(readFileSync(process.argv[1])).digest("base64")}`);
' "$tarball")

errors=$(mktemp)
if published=$(npm view "${name}@${version}" dist.integrity --json --prefer-online 2>"$errors"); then
  actual=$(node -e 'process.stdout.write(JSON.parse(process.argv[1]))' "$published")
  if [ "$actual" != "$integrity" ]; then
    echo "::error::${name}@${version} exists with different tarball integrity"
    exit 1
  fi
  echo "${name}@${version} already matches the packed candidate"
elif ! grep -q E404 <<<"$published" && ! grep -q E404 "$errors"; then
  # Only a 404 proves the version is absent; any other failure decides nothing.
  cat "$errors" >&2
  echo "::error::the registry did not say whether ${name}@${version} exists"
  exit 1
elif [ "$check" = true ]; then
  echo "${name}@${version} is not on the registry yet"
else
  status=0
  npm publish "$tarball" --access public --tag latest 2>"$errors" || status=$?
  cat "$errors" >&2
  # npm processes a new version for minutes before serving it, so an earlier
  # attempt's publish can still be staged; the readback proves its bytes.
  if [ "$status" -ne 0 ] && ! grep -q 'previously staged version' "$errors"; then
    exit "$status"
  fi
fi

if [ "$check" = true ]; then
  exit 0
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
