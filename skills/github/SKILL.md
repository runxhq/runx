---
name: github
description: Read or synchronize bounded GitHub issues, threads, and pull requests through the GitHub connector, with scoped authority and readback.
---

# GitHub

Define one bounded synchronization between local Runx state and a GitHub
repository. The skill makes direction, resource set, filters, content identity,
scope, cursor, and authority posture explicit before the selected GitHub
instance is allowed to read or mutate GitHub.

The default `github` runner performs the requested pull or push. `plan`
is the explicit no-effect runner; `pull` and `push` remain explicit execution
lanes for compatible callers. Pulls are bounded reads. A push carries one to
eight typed mutations that one GitHub operation applies, and returns each
item's outcome. It closes only on an independent GitHub readback of every
applied item. An item whose outcome is unknown holds the push for
reconciliation and is never silently retried. `activity` reads what changed
in a repository since a time, compact enough to summarise. No GitHub token
enters this package.

## When to use it

Use `github` when an operator needs a reproducible pull or push contract for
issues, pull requests, or threads, especially when cursor state must survive
across runs. Use `plan` only when the requested outcome is a plan. Use `pull`
for an explicitly selected read lane. Use `activity` to learn what changed in
a repository since a time, such as since the last check of a daily summary:
it reads issues and pull requests together and returns the fields a reader
needs rather than GitHub's whole records. Use `push` for already-decided issue,
pull-request, or comment changes of one kind. Batches are bounded and
best-effort, not atomic: a refused item does not stop the rest, the result
identifies each item, and the operator reconciles before retrying. Use `github-issue-triage` to decide what an issue means
and `github-issue-to-pr` to govern an implementation lane.

Do not use a plan as evidence that remote state was read or changed. Only the
native `runx.provider.operation.v1` result from `pull` or `push` is provider
evidence. Do not silently turn a denied push into a pull.

## How it works

1. Validate the exact `owner/name` repository, direction, resource kind, bounded
   filters, maximum result count, and requested scope.
2. A `pull` plan requires read scope and records the exact resources a provider
   may fetch.
3. A `push` plan requires write scope and one to eight typed mutations that
   one GitHub operation applies: `issues.create`, `issues.update`,
   `pullrequests.update`, `comments.create`, or `comments.update`. It rejects unknown fields,
   oversized content, and mixed kinds, then returns `ready_for_execution`;
   planning itself does not grant provider authority.
4. Optional `plan_and_append_cursor` calls the store's `events.append` and
   `projection.read` to append the bounded plan and read the projection back. That proves local
   cursor persistence, not GitHub synchronization; this package does not own a
   second cursor database or storage adapter.
5. The default runner selects the same bounded read or write path from the
   validated direction. `pull` calls the planned read (`issues.list`,
   `issues.read`, `pullrequests.list`, `pullrequests.read`, or
   `comments.list`) with `repo.read`. `push` calls the planned write once under
   `repo.write`: the runtime sends each item, records each outcome, and reads
   every applied item back before the push closes.
   Both run under the GitHub instance selected here: the signed-in `gh` session
   or a stored token.
6. `activity` calls `issues.list` with `repo.read` for one page of 100 of
   everything updated at or after `since`, open or closed. GitHub's listing
   holds pull requests too, so one read finds both. The result keeps each
   item's number, kind, title, state (`merged` for a merged pull request),
   whether it was opened at or after `since`, its author and whether that is a
   bot, its times, labels, assignees, comment count and link, and the first
   600 characters of its body with the count left out. Items come newest
   change first; a full page names its `next_page`.

## Inputs and result

- `repo` is exactly `owner/name`.
- `direction` is `pull` or `push`.
- `resources` contains the issue, PR, thread, or `batch` selector and a
  limit no greater than 100. A push has one to eight `mutations` entries with `ref`, `op`, and an
  exact typed `payload`. Issue and PR changes use `op: update`; a new issue
  uses `ref: issues` and `op: create`; new thread comments use `op: comment`. Bodies are capped at 65,536 characters. Pulls
  return GitHub's records whole.
- `scope` is the requested read or write scope.
- `activity` takes `repo`, `since` (RFC 3339) and an optional `page`, and
  returns `activity`: the repository, `since`, the page, its items, and
  `next_page` when more may follow. Its provider evidence is the read's
  `runx.provider.operation.v1`, kept in the run's record.

The `runx.github_sync.v1` plan records exact direction, provider operation,
scope, filters, blockers, authority posture, cursor state when used, and the
explicit absence of remote effects. Execution additionally emits the native
provider-operation packet; it does not rewrite the planning packet to imply a
remote effect.

## Stop conditions

- Refuse malformed repositories, unknown resource kinds, unbounded selectors,
  limits above the contract, more than eight mutations, unknown mutation fields, or
  content beyond the declared bounds.
- Refuse push when write scope is missing; do not degrade silently.
- Do not treat local cursor persistence as remote provider readback.
- Refuse a push that mixes new issues, issue, pull-request, and comment
  changes; send each kind as its own push.
- Refuse when no GitHub instance is selected, or the selected one lacks the
  scope. Never treat an available instance as evidence of the intended
  repository.
- Do not claim comments, labels, issues, or PRs were read or changed until the
  native provider operation proves the expected access, operation, and readback.

## Example

A caller wants to pull the next 50 open issues after cursor `2`. Planning can
persist that cursor locally; `pull` then performs `issues.list` through the
selected instance and returns the provider result. To close issue 241, a push
carries `ref: issues/241`, `op: update`, and
`payload: {state: closed, state_reason: completed}`. Runx applies only that
issue update through `issues.update` under `repo.write`, and reads issue 241
back. A morning summary calls `activity` with `since` set to its last check
and reports the items that are new, or changed by someone else.
