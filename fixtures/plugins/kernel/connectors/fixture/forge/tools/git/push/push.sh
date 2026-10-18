#!/bin/sh
# fixture/forge's branch.push: it reports the branch at the requested commit
# and touches nothing, so a test exercises the operation's governance only.
set -eu
phase=$1
branch=$3
commit=$4
case "$phase" in
  perform | readback) printf '{"branch":"%s","commit":"%s"}\n' "$branch" "$commit" ;;
  *) printf 'phase is invalid\n' >&2; exit 2 ;;
esac
