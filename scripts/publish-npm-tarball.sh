#!/usr/bin/env bash
# Publish packed npm tarballs, once each, then read every one back until the
# registry serves the same integrity. A version already on the registry must
# be these exact bytes. npm processes a new version for minutes before serving
# it, so the tarballs given together publish together and share one readback
# window. With --check, only refuse a version the registry holds with other
# bytes, and publish nothing.
set -euo pipefail

check=false
if [ "${1:-}" = "--check" ]; then
  check=true
  shift
fi
if [ "$#" -eq 0 ]; then
  echo "usage: publish-npm-tarball.sh [--check] <tarball>..." >&2
  exit 2
fi

specs=()
integrities=()
errors=$(mktemp)
for tarball in "$@"; do
  manifest=$(tar -xOzf "$tarball" package/package.json)
  name=$(node -e 'process.stdout.write(JSON.parse(process.argv[1]).name)' "$manifest")
  version=$(node -e 'process.stdout.write(JSON.parse(process.argv[1]).version)' "$manifest")
  integrity=$(node -e '
    const { createHash } = require("node:crypto");
    const { readFileSync } = require("node:fs");
    process.stdout.write(`sha512-${createHash("sha512").update(readFileSync(process.argv[1])).digest("base64")}`);
  ' "$tarball")
  spec="${name}@${version}"

  if published=$(npm view "$spec" dist.integrity --json --prefer-online 2>"$errors"); then
    actual=$(node -e 'process.stdout.write(JSON.parse(process.argv[1]))' "$published")
    if [ "$actual" != "$integrity" ]; then
      echo "::error::${spec} exists with different tarball integrity"
      exit 1
    fi
    echo "${spec} already matches the packed candidate"
  elif ! grep -q E404 <<<"$published" && ! grep -q E404 "$errors"; then
    # Only a 404 proves the version is absent; any other failure decides nothing.
    cat "$errors" >&2
    echo "::error::the registry did not say whether ${spec} exists"
    exit 1
  elif [ "$check" = true ]; then
    echo "${spec} is not on the registry yet"
  else
    status=0
    npm publish "$tarball" --access public --tag latest 2>"$errors" || status=$?
    cat "$errors" >&2
    # An earlier attempt's publish can still be staged; the readback proves
    # its bytes.
    if [ "$status" -ne 0 ] && ! grep -q 'previously staged version' "$errors"; then
      exit "$status"
    fi
  fi
  specs+=("$spec")
  integrities+=("$integrity")
done

if [ "$check" = true ]; then
  exit 0
fi

# npm took 14.6 minutes to serve a 0.10.2 platform package.
deadline=$((SECONDS + 1800))
pending=("${!specs[@]}")
while :; do
  waiting=()
  for index in "${pending[@]}"; do
    observed=$(npm view "${specs[$index]}" dist.integrity --json --prefer-online 2>/dev/null || true)
    if [ "$observed" = "\"${integrities[$index]}\"" ]; then
      echo "${specs[$index]} is served with the packed bytes"
    else
      waiting+=("$index")
    fi
  done
  if [ "${#waiting[@]}" -eq 0 ]; then
    exit 0
  fi
  pending=("${waiting[@]}")
  if [ "$SECONDS" -ge "$deadline" ]; then
    for index in "${pending[@]}"; do
      echo "::error::${specs[$index]} registry integrity readback did not match the candidate"
    done
    exit 1
  fi
  sleep 10
done
