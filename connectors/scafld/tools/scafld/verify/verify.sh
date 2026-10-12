#!/bin/sh
# The scafld connector's receipt.verify, as a trusted host process. It runs
# the scafld CLI on PATH to verify one signed receipt against an exact commit
# and reports whether it passed, with the receipt it read and that receipt's
# SHA-256. A receipt scafld rejects is an answer, not a failure; any other
# verifier error fails the read. Inputs arrive as validated arguments; output
# is JSON.
set -eu

receipt_path=$1
target=$2
repo_root=$3

fail() {
  printf '%s\n' "$1" >&2
  exit 2
}

cd "${RUNX_CWD:-.}"
root=$(cd "$repo_root" 2>/dev/null && pwd -P) || fail "repo_root is not a directory"
directory=$(cd "$root" && cd "$(dirname "$receipt_path")" 2>/dev/null && pwd -P) ||
  fail "receipt_path is not within a directory"
receipt="$directory/$(basename "$receipt_path")"
case "$receipt" in "$root"/*) ;; *) fail "receipt_path must be within repo_root" ;; esac
[ -f "$receipt" ] || fail "receipt_path is not a file"

if command -v sha256sum >/dev/null 2>&1; then
  digest=$(sha256sum "$receipt" | cut -d ' ' -f 1)
else
  digest=$(shasum -a 256 "$receipt" | cut -d ' ' -f 1)
fi

set +e
scafld verify "$receipt" --target "$target" --root "$root" --json >/dev/null
status=$?
set -e
case $status in
  0) passed=true ;;
  3) passed=false ;;
  *) printf 'scafld verify failed with status %s\n' "$status" >&2; exit 1 ;;
esac

printf '{"passed":%s,"receipt_digest":"sha256:%s","receipt":' "$passed" "$digest"
cat "$receipt"
printf '}\n'
