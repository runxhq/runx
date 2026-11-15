---
name: github
description: Read or synchronize bounded GitHub issues, threads, and pull requests through the GitHub connector, with scoped authority and readback.
---

# GitHub

Read actual GitHub issues, pull requests and comments with the selected account,
or apply exact already-decided changes and confirm them independently. For a
normal read, give `repo` and optionally `kind`, `number`, `page` and `limit`.
The read authority is selected by the existing native operation; callers do not
need to construct synchronization direction or scope just to inspect an issue.

The default `github` method reads one bounded page or exact numbered record.
`kind` is `issues` (the shared GitHub issue feed, including PRs), `pull_requests`
or `comments`. Comments are issue/PR conversation comments; review diff comments
are a different capability. `comments` requires the issue or PR number. Exact
numbered reads return the actual current record; state/label filters are for
list queries. Raw provider records and the original native operation evidence
are preserved. A bounded page never establishes complete conversation/history
coverage, including when filtering PRs out of an issue-feed page.

The explicit `sync` method performs the requested advanced pull or push. `plan`
is the explicit no-effect method; `pull` and `push` remain explicit execution
lanes for compatible callers. Pulls are bounded reads. A push carries one to
twenty typed mutations per call that one GitHub operation applies, and returns each
item's outcome. It closes only on an independent GitHub readback of every
applied item. An item whose outcome is unknown holds the push for
reconciliation and is never silently retried. `activity` reads what changed
in each of a list of repositories, to the end, and writes nothing. `follow` is
what a schedule runs: it keeps each reader's read position for each
repository in this package's own store, moves it only past what it read and
stored, and returns a verdict on whether every repository was read. No GitHub
token enters this package.

## When to use it

Use the default for actual bounded records. Resolve an omitted repository in
the host with ordinary git remote/context tools, verify the exact owner/name,
and pass it explicitly. Do not guess a repository, treat a folder name as one,
or introduce a separate repository/cursor state store.

Use `sync` when an operator needs a reproducible pull or push contract for
issues, pull requests, or threads. Use `plan` only when the requested outcome
is a plan. Use `pull` for an explicitly selected read lane. Use `activity` to
learn what changed in some repositories since a time or a position: it reads
issues and pull requests together and returns the fields a reader needs
rather than GitHub's whole records. Use `follow` for a schedule that must
read everything, such as a daily summary: it reads on from where this reader
last stopped, and its verdict says whether every repository was read through
the window's end. Use `push` for already-decided issue,
pull-request, or comment changes of one kind. A call accepts up to twenty changes, matching the connector's batch contract; split larger work into separate calls and retain each result. Batches are bounded and
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
3. A `push` plan requires write scope and one to twenty typed mutations per call that
   one GitHub operation applies: `issues.create`, `issues.update`,
   `pullrequests.update`, `comments.create`, or `comments.update`. It rejects unknown fields,
   oversized content, and mixed kinds, then returns `ready_for_execution`;
   planning itself does not grant provider authority.
4. The explicit `sync` method selects the same bounded read or write path from the
   validated direction. `pull` calls the planned read (`issues.list`,
   `issues.read`, `pullrequests.list`, `pullrequests.read`, or
   `comments.list`) with `repo.read`. `push` calls the planned write once under
   `repo.write` for issue and pull-request changes or `issue.comment` for comment changes: the runtime sends each item, records each outcome, and reads
   every applied item back before the push closes.
   Both run under the GitHub instance selected here: the signed-in `gh` session
   or a stored token.
5. `activity` reads each repository with `issues.list` under `repo.read`, by
   keyset: every page asks for everything updated since the mark, oldest
   first, open or closed, and drops what was already read at the mark. Page
   numbers would skip an item whenever another is updated mid-read; a keyset
   read only finds the updated one later, where it's read. A page shorter
   than 100 is the end. When a whole page shares one timestamp, the read goes
   on by page there and the mark keeps the ids read at it. GitHub's listing
   holds pull requests too, so one read finds both. Each item keeps its
   number, kind, title, state (`merged` for a merged pull request), whether
   it was opened since the read began, its author and whether that is a bot,
   its times, labels, assignees, comment count and link, and the first 600
   characters of its body with the count left out. A repository that fails
   is reported with its error and the rest go on, so a failure never reads as
   nothing changed. A read stops short of its calls and of 2,000 items, and
   what it didn't reach is incomplete at its position or unstarted.
6. `follow` reads each repository's position for this reader, this GitHub
   account (by its id, so a rotated token keeps it) and this query from the
   `positions` stream, runs `activity` from there, stores what was read as an
   artifact, and only then appends `position.moved` at the version it read,
   so a position never moves past results the reader didn't receive. A move
   that loses to a concurrent one reads again from the winner's position. A
   run that runs out of calls says so in `budget_exhausted`, and the host runs
   `follow` again. Its verdict, `runx.verdict.v1` against
   `runx/github.coverage`, is `met` only when every repository is checked
   through the window's end.

## Inputs and result

- `repo` is exactly `owner/name`.
- The default needs only `repo`: e.g. `{repo: "runxhq/runx", kind: "pull_requests", number: 554}` reads that exact PR. To continue comments use `kind: comments`, its `number`, and the next `page`, retaining the same query and original provider receipt.
- `direction` is `pull` or `push` for the explicit `sync` and planning phases.
- `resources` contains the issue, PR, thread, or `batch` selector and a
  limit no greater than 100. A push has one to twenty `mutations` entries with `path`, `op`, and an
  exact typed `payload`. Issue and PR changes use `op: update`; a new issue
  uses `path: issues` and `op: create`; new thread comments use `op: comment`. Bodies are capped at 65,536 characters. Pulls
  return GitHub's records whole.
- `scope` is the requested read or write scope.
- `activity` takes `repositories`, each `{repository, since}` or
  `{repository, position}`, and returns `coverage`, each repository's
  `started`, `complete`, `error`, `items` and next `position`, and
  `evidence`, the reads it rests on.
- `follow` takes `repositories`, `reader` (who's reading, such as a schedule)
  and `until`, the window's end, and returns `coverage`, each repository's
  `position`, `checked_through`, `complete`, `error` and the `fragments`
  stored this run, with `budget_exhausted` and the `verdict`.

The `runx/github.sync_plan.v1` plan records exact direction, provider operation,
scope, filters, blockers, authority posture, and the explicit absence of
remote effects. Execution additionally emits the native
provider-operation packet; it does not rewrite the planning packet to imply a
remote effect.

## Stop conditions

- Refuse malformed repositories, unknown resource kinds, unbounded selectors,
  limits above the contract, more than twenty mutations in one call, unknown mutation fields, or
  content beyond the declared bounds.
- Refuse push when write scope is missing; do not degrade silently.
- Do not treat a stored read position as remote provider readback.
- Refuse a push that mixes new issues, issue, pull-request, and comment
  changes; send each kind as its own push.
- Refuse when no GitHub instance is selected, or the selected one lacks the
  scope. Never treat an available instance as evidence of the intended
  repository.
- Do not claim comments, labels, issues, or PRs were read or changed until the
  native provider operation proves the expected access, operation, and readback.

## Example

A caller wants the next 50 open issues after page `2`: `pull` performs
`issues.list` through the selected instance and returns the provider result.
To close issue 241, a push carries `path: issues/241`, `op: update`, and
`payload: {state: closed, state_reason: completed}`. runx applies only that
issue update through `issues.update` under `repo.write`, and reads issue 241
back. A morning summary's schedule runs `follow` with its repositories,
`reader: morning` and `until` set to the morning's time, reports the items
that are new or changed by someone else, and trusts the morning only when the
verdict is `met`.

For composition, consume the returned native provider-operation packet directly.
Pass already read issue fields to `github-issue-triage` `respond`, preserving
repository, exact number, title, state, full relevant body and original source
link/receipt. This path does not fetch or post again. `pull` and `push` retain
their complete typed selectors for power users; the common reader does not
replace them.
