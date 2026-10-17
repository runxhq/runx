# Issue to PR

`github-issue-to-pr` implements one bounded GitHub issue with the repository's normal
development tools. It is useful when invoked directly by an agent and when a
larger Runx chain already has issue or host-work evidence.

The public promise is literal: the default runner reads the issue, hands normal
repository work to the host agent, requires tested change evidence and one
successful `scafld finalize`, then either returns the completed local change or
opens the exact requested pull request under scoped provider authority and
reads it back.

## Direct flow

1. Resolve `repository` from an explicit `owner/name` or `.` for the current
   checkout's `origin`. Available grants never determine the target.
2. Use the already-authenticated local `gh` and Git installations by default.
   A project may bind hosted Runx Connect when provider isolation or hosted
   authority is required.
3. Read the issue once and emit `runx.issue_to_pr.issue_evidence.v1`.
4. Let the host agent investigate, edit, and test with its ordinary repository
   tools. Runx does not turn editing into a sequence of answer files.
5. Call `scafld finalize` once after the change and tests are ready. Record its
   receipt reference and contract digest in
   `runx.issue_to_pr.host_result.v1`.
6. Withhold publication unless the host result explicitly requests it. A held
   result is still a completed tested local change, not a plan.
7. When publication is requested, admit the exact `pullrequest.publish` effect
   under the repository grant, create the PR once, and independently read the
   resulting PR back.

GitHub comments, feed synchronization, notifications, documentation updates,
and source-issue closure are separate optional skills. They are not hidden
stages of issue-to-PR.

## Chain reuse

Use the smallest runner matching evidence already available:

- `github-issue-to-pr` reads the issue and performs host work.
- `from-evidence` accepts `runx.issue_to_pr.issue_evidence.v1` and skips the
  provider read.
- `resume` accepts issue evidence plus
  `runx.issue_to_pr.host_result.v1`; it skips issue discovery, editing, tests,
  and finalize.
- `publish` accepts an admitted completed host result and performs only PR open
  plus readback.

The host result must name the same repository and issue as the issue evidence.
Tests must all pass. Finalize must report one successful invocation and a
receipt. A downstream runner refuses mismatched or incomplete evidence instead
of redoing the work to guess what happened.

## Authority and recovery

Reads and local analysis require no approval. PR creation uses the exact
repository grant and does not invent a second human gate merely because it is a
write. Admission binds provider, repository, operation, payload digest,
required scopes, principal, and plan digest.

Runx persists provider attempt and readback state under the project `.runx`
store. Resume retains the admitted plan and generated idempotency binding. If a
local PR creation has an unknown prior outcome, Runx refuses to repeat the
non-idempotent mutation and asks the operator to inspect GitHub before
continuing.

## Native invocation

Run against the current checkout:

```bash
runx skill skills/github-issue-to-pr \
  --input repository=. \
  --input issue_number=442
```

Run from previously captured evidence by passing a JSON input document:

```bash
runx skill skills/github-issue-to-pr resume \
  --inputs .runx/github-issue-to-pr/resume.json
```

If Runx pauses for host work, continue the same immutable run:

```bash
runx resume <run-id> -
```

The stdin document (`runx.resume.v1`) carries the requested agent answer,
bound to the request's digest. Normal output stays
compact; use `--diagnostics` only when graph internals are actually needed.

## Dogfood proof

The checked-in journeys are the executable product contract:

- `skills/github-issue-to-pr/fixtures/standalone-host-flow.yaml`
- `skills/github-issue-to-pr/fixtures/composed-pr-flow.yaml`
- `skills/github-issue-to-pr/fixtures/resume-after-provider-failure.yaml`
- `runx-cli` integration test `issue_to_pr_journey`

They use a fake local `gh`, never a live repository. Together they prove
checkout target resolution, local issue read, typed host evidence, exactly one
finalize claim, chain reuse without repeated work, one scoped PR mutation,
independent PR readback, and the absence of implicit feed or notification work.
