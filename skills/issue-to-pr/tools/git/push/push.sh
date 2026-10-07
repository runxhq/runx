#!/bin/sh
# The github connector's branch.push, as a trusted host process. It pushes an
# exact local commit to a GitHub branch over HTTPS with the instance's token
# and never forces: a branch already at the commit is left alone, and one at
# another commit is refused. Run with phase readback, it reports the branch as
# GitHub has it. Inputs arrive as validated arguments; output is JSON.
set -eu

phase=$1
repository=$2
branch=$3
commit=$4

invalid() {
  printf '%s is invalid\n' "$1" >&2
  exit 2
}
case "$repository" in
  */*/* | /* | */ | *[!A-Za-z0-9_./-]*) invalid repository ;;
  */*) ;;
  *) invalid repository ;;
esac
case "$branch" in "" | *[!A-Za-z0-9._/-]* | -*) invalid branch ;; esac
case "$commit" in *[!0-9a-f]*) invalid commit ;; esac
[ ${#commit} -eq 40 ] || [ ${#commit} -eq 64 ] || invalid commit

remote="https://github.com/$repository.git"
if [ -n "${GITHUB_TOKEN:-}" ]; then
  GIT_CONFIG_COUNT=1
  GIT_CONFIG_KEY_0=http.https://github.com/.extraheader
  GIT_CONFIG_VALUE_0="AUTHORIZATION: basic $(printf 'x-access-token:%s' "$GITHUB_TOKEN" | base64 | tr -d '\n')"
  export GIT_CONFIG_COUNT GIT_CONFIG_KEY_0 GIT_CONFIG_VALUE_0
fi
GIT_TERMINAL_PROMPT=0
GIT_OPTIONAL_LOCKS=0
export GIT_TERMINAL_PROMPT GIT_OPTIONAL_LOCKS
cd "${RUNX_CWD:-.}"

remote_head() {
  git -c core.fsmonitor=false ls-remote --refs "$remote" "refs/heads/$branch" | {
    read -r sha _ || true
    printf '%s' "${sha:-}"
  }
}

case "$phase" in
  readback)
    printf '{"branch":"%s","commit":"%s"}\n' "$branch" "$(remote_head)"
    exit 0
    ;;
  perform) ;;
  *) invalid phase ;;
esac

local_commit=$(git -c core.fsmonitor=false rev-parse --verify "$commit^{commit}" 2>/dev/null || true)
if [ "$local_commit" != "$commit" ]; then
  printf '{"refused":"commit %s is not in this workspace"}\n' "$commit"
  exit 0
fi
current=$(remote_head)
if [ "$current" = "$commit" ]; then
  printf '{"branch":"%s","commit":"%s","pushed":false}\n' "$branch" "$commit"
  exit 0
fi
if [ -n "$current" ]; then
  printf '{"refused":"branch %s already points at %s; a push is never forced"}\n' "$branch" "$current"
  exit 0
fi
git -c core.fsmonitor=false push --porcelain "$remote" "$commit:refs/heads/$branch" >/dev/null
printf '{"branch":"%s","commit":"%s","pushed":true}\n' "$branch" "$commit"
