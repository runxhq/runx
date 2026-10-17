---
name: github-issue-to-pr
description: Implement one scafld-governed repository issue with normal host tools, prove the tested change through its finalize wall, and optionally publish the exact pull request through scoped provider authority with readback.
---

# GitHub Issue to PR

Turn one issue into a tested change and, when authorized, a verified pull
request. Use the repository's normal development workflow. Runx owns evidence
continuity and the consequential publication boundary; it does not replace the
host agent's editor, shell, Git, tests, or repository judgment.

## Scope

Use this runner for work the repository actually governs with scafld: essential
architecture, execution, security, persistence or cross-package contract changes,
or work explicitly placed under that lifecycle by the operator. Read the
repository's current instructions before choosing it. The receipt-verification
wall is part of this skill's executable contract.

For ordinary UI, copy, documentation and routine fixes, use normal host editing
and focused checks, then the selected GitHub operation for publication. Do not
create a scafld lifecycle merely to satisfy this package, and do not invent a
finalization receipt. Task size or file count alone does not make work governed.

## Direct operator flow

1. Name the repository as an exact `owner/name`. Never infer the target from
   an available grant.
2. Read through the GitHub instance selected here: the signed-in `gh`
   session (`runx connector use github --session`) or a stored token.
3. Run one preflight before work: issue identity, repository permission, branch
   state, required tools, requested outcome, and publication authority.
4. Investigate, edit, and test with the host agent's ordinary tools. Do not
   manufacture Runx answer files between normal coding steps.
5. Complete the repository-required independent review and call `scafld finalize` exactly once after the change and tests are ready.
   Preserve its workspace-scoped receipt path, exact target commit, and
   contract digest in `host_result`. Runx verifies that receipt with the scafld
   CLI on this host (`scafld/receipt.verify`) and checks its verdict, commit,
   contract and open blockers; host-authored status strings are not proof.
6. If PR publication is not authorized, return the tested/finalized result and
   stop. Do not silently downgrade the work to a plan.
7. If publication is authorized, pass the exact `host_result` to `publish`.
   Runx pushes the exact commit to the requested branch (`github/branch.push`,
   never forced, read back from GitHub), reuses the one open pull request for
   that branch or opens one (`github/pullrequest.open`, read back), and
   completes only when the pull request's head is the tested commit.
   Notifications, feeds, issue comments, and documentation sync are optional
   downstream skills.

## Reuse in chains

- `from-evidence` accepts `runx.issue_to_pr.issue_evidence.v1` and skips GitHub
  discovery and issue read.
- `resume` accepts both prior issue evidence and a completed
  `runx.issue_to_pr.host_result.v1`; it does not repeat host work or finalize.
- `verify` verifies the signed scafld receipt against the exact commit and
  contract without publishing anything.
- `finalize-local` verifies the completed host result and returns it without a
  remote mutation.
- `publish` verifies the completed host result, publishes its exact Git ref,
  and creates or recovers the authorized PR with readback.
- Preserve the same idempotency key across pause, retry, and resume. An
  uncertain PR creation never gets a new key.

## Host work contract

The `github-issue-to-pr-host-work` act performs normal repository work and returns:

```yaml
host_result:
  schema: runx.issue_to_pr.host_result.v1
  status: completed | blocked | failed
  repository: owner/name
  issue_number: string
  repo_root: workspace-relative-or-absolute-path
  branch: string
  commit: full-git-object-id
  files: [relative/path]
  tests:
    - command: string
      status: passed | failed
      evidence: string
  finalization:
    receipt_path: workspace-relative-path
    contract_digest: sha256:...
  publication:
    decision: hold | ready
    title: string
    body: string
    head: string
    base: string
    draft: boolean
    idempotency_key: string
  errors: [string]
```

Do not claim `completed` without a real edit/test outcome and one successful
finalize result. Do not claim `published` from a branch push, API
acknowledgement, or draft packet; only independent PR readback closes that
state.

## Stop conditions

- Wrong or ambiguous repository: stop with one target-resolution blocker.
- No GitHub instance selected: return the exact `runx connector use github
  --session` or `runx connector use github --credential token` handoff; do
  not cycle through unrelated skills.
- Dirty or conflicting branch state: stop before mutation and preserve the
  evidence already gathered.
- Failed tests or failed/stale finalize: return blocked or failed, never
  succeeded.
- Missing publication authority: retain the tested finalized change locally and
  stop before PR creation.
- Provider failure after admission: preserve the grant reference, idempotency key,
  finalization evidence, and mutation recovery state. Resume at publication;
  do not repeat issue discovery, coding, tests, or finalize.
