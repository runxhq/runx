---
name: github-issue-to-pr
description: Implement one GitHub issue under the repository's actual development process, verify the exact local candidate, and optionally publish or recover its pull request with provider readback.
---

# GitHub Issue to PR

Turn one issue into a checked repository change and, when authorized, a pull
request carrying that exact commit. The host agent investigates, edits and runs
the appropriate checks with its normal tools. Runx binds the selected issue,
observes the actual candidate checkout, applies any repository-required
finalization, and governs publication and readback.

Use this for implementation. Use `github-issue-triage` for an unsent maintainer
response, `github-pr-review` for a review of an existing PR, and normal host
editing for work that does not need this issue-to-publication workflow.

## Start with the actual repository

Invoke the skill from the target checkout, selecting an exact `owner/name` and
issue number. Resolve a supplied issue URL into those fields; an available grant
does not choose the repository. Select a compatible GitHub binding, such as the
existing `gh` session, before the live issue read. An operator-supplied objective
can narrow the requested outcome but does not replace the issue evidence.

Read current repository instructions and inspect the relevant call paths before
editing. Check existing changes, branch state, required tools, commit permission
and the intended publication boundary. Preserve other work. Follow the actual
repository's branch, commit, review and test conventions, including an external
project's own conventions. Do not create a branch, commit or PR where the
operator or repository requires an instruction you do not have.

Ordinary fixes use ordinary editing and focused checks. Use scafld only when
the repository requires it for this work or the operator explicitly chooses it.
The existence of a `.scafld` directory, task size, or a wish to use this skill is
not that requirement. When applicable, complete the required independent review
and finalize once; retain its receipt path and exact contract digest.

## The implementation turn

The `github-issue-to-pr-host-work` act performs the real work. It returns the
owned `runx/github-issue-to-pr.host_result.v1` packet: issue identity, summary, checkout,
branch, exact candidate commit, changed files, actual check commands and their
evidence, the established repository governance, and a publication proposal or
hold. A blocked attempt can report the missing commit, failed checks and errors;
it must not invent completion evidence to satisfy the packet.

Record `governance.kind: repository` with the concrete reason ordinary repository
checks apply, or `scafld` with the repository-required finalize receipt and
contract digest. This is a host report of the applicable procedure. It cannot
override repository instructions or waive required review. A test report is also
host-reported evidence: Runx does not retroactively attest that an external host
command ran just because its name appears in the report.

After host work, the shared verification path reads native Git status, current
branch and HEAD in the invocation checkout. The reported root must identify this
same checkout. Dirty state, a detached or different branch, or a changed HEAD
holds the candidate before any push. These checks bind the current source
candidate; they do not establish workspace isolation, prove a test environment,
or authorize unrelated edits. For scafld work, the native connector additionally
verifies a passing signed receipt for the exact commit and contract, with no
open blockers. A supplied status string is insufficient.

## Publication and finality

When publication is held, return the verified local candidate and the remaining
work. A local result is not a PR, a merge approval or a deployment.

A ready proposal supplies title, body, head, base, draft status and stable retry
identity. Publication still requires the selected native provider authority.
The GitHub connector pushes only the exact verified commit, never forces a ref,
and independently reads it back. The skill lists open PRs for that head and base,
reuses the unique matching PR, or opens one. More than one match, a different
head commit, wrong base/repository, malformed provider evidence or failed
readback cannot become `published`.

The terminal `runx/github-issue-to-pr.result.v1` packet preserves the useful
summary, files and host check report. It separately states native candidate
verification, governance assurance and PR publication/readback. Host-reported
tests stay labelled as such even after successful publication.

## Reuse and recovery

- The default reads the selected issue once, then runs the implementation turn.
- `from-evidence` accepts the existing issue evidence packet and skips the read.
- `resume` consumes that evidence and a completed work packet, rechecks the
  candidate and continues delivery. It does not repeat coding, tests or finalize.
- `verify` checks the candidate and required governance without publication.
- `finalize-local` forces a local-only result, even if the proposal is ready.
- `publish` requires a ready proposal and uses the same verification path.

For example, an ordinary documentation correction can run its meaningful
documentation checks, obtain any required commit permission, and return a local
candidate without creating a scafld task. A later authorized publication reuses
that work packet. An essential engine change follows its required lifecycle and
passes the same native candidate check plus signed finalization verification.

After an uncertain provider result, retain the work packet, its issue evidence,
the original retry identity and operation receipts. Reconcile or resume that
exact proposal; do not create a new key, repeat host work or finalize again.
If the checkout has changed, recover the exact candidate or produce a newly
checked work packet. Notifications, comments, feeds and release work remain
separate downstream capabilities.

### `github-issue-to-pr-host-work`

Do the requested repository work, following its actual instructions and authority.
Inspect before editing; select appropriate checks; report their real outcome and
evidence. Return a useful summary and concrete blockers. Never invent a commit,
test, receipt, approval or published PR. Keep the work packet for continuation if
publication is not authorized or a provider step fails.
