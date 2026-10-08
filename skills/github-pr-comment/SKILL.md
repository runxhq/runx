---
name: github-pr-comment
description: Post one exact GitHub pull-request comment through the GitHub connector, with retry safety and independent readback.
---

# GitHub PR Comment

Post one bounded review comment to one known GitHub pull request. The skill is
deliberately narrower than a general GitHub operator: it binds the exact
repository, pull request, and comment body to a scoped provider operation, and
closes only when provider evidence identifies and reads back the created note.

Use it after a review workflow has produced final wording and the operator wants
that exact note posted. Use `issue-triage` for analysis and drafting, and a
broader GitHub skill for other resource types. This skill never merges a pull
request; comment authority cannot be promoted into merge authority.

## How it works

1. Supply `owner/name`, the exact pull-request number, exact comment body, and a
   stable idempotency key for that body.
2. The default `post` runner calls `github/comments.create` once with the exact
   body, under the GitHub instance selected here: the signed-in `gh` session
   or a stored token.
3. The runtime records GitHub's acknowledgement, reads the comment back by its
   id, and completes only when the read-back id and body match the approved
   comment.

The separate `merge-refused` runner asks for `github/pullrequest.merge` under
the same comment grant. It is an executable authority test: the merge request
must seal as policy denied, never as a backdoor merge path.

## Stop conditions

- Stop when repository, pull-request number, body, or idempotency key is
  missing or changes after admission.
- Refuse a provider grant that does not resolve uniquely to `pr.comment`.
- Refuse when no GitHub instance is selected, or the selected one cannot
  comment; the skill never handles a raw token.
- Do not treat mutation acceptance without an independent comment read whose
  id and body match as final readback.
- Never merge, close, label, or otherwise mutate the pull request beyond the
  exact comment.

## Example

An operator invokes `post` with “Please add the missing recovery fixture” for
PR 42 under a `pr.comment` grant. The skill posts exactly that body under the
stable retry key and seals the returned comment id. Changing the body changes
the operation identity. Attempting to reuse the comment grant for merge is
denied before GitHub mutation.
