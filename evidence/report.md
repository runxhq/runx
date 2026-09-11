# postmortem-maker delivery draft

Status: blocked for external release.

This workspace contains a reviewable `postmortem-maker` package at version
`0.1.0`. The package is source-first: it reads a bounded public GitHub issue at
runtime (or an upstream read projection), keeps exact event citations, places
uncertainty in `unknowns[]`, and executes a digest-bound local outbox append
only when `postmortem_policy.allow_publish` is explicitly true. The second
step reads the outbox back, so the successful path records an executed
send-as-shaped plan and readback rather than an inert proposal.

## Local evidence

The direct step-handler smoke test passed:

```text
node tests/postmortem-maker-smoke.mjs
{"status":"passed","cases":[{"name":"consistent-incident-published","status":"sealed","publishable":true,"delivery":"executed"},{"name":"conflicting-evidence-withheld","status":"sealed","publishable":false,"unknowns":2,"delivery":null},{"name":"idempotent-replay","status":"passed","replayed":true},{"name":"fixture-traversal-refused","status":"refused"},{"name":"source-allowlist-refused","status":"refused"},{"name":"long-event-body-accepted","status":"passed"}]}
```

The consistent fixture produces a confirmed root cause, four source-cited
timeline entries, an executed local publish result, and a digest-matching
readback. The conflicting fixture produces an unconfirmed root cause, two
unknowns, no publish result, and a readback proving no delivery. These are
local fixture checks. The smoke test also exercises a 2,500-character event
body, preventing the GitHub issue-body regression found during dogfood.
The native runx harness passes both graph cases and emits local fixture
receipts; those receipts are not the required post-publish real-source
dogfood receipt.

`X.yaml` also parses successfully with PyYAML, and `node --check` passes for
all six step modules and the smoke test.

## Native runx validation

The pinned CLI is installed and the exact command returned:

```text
runx-cli 0.6.15
```

The native graph harness passed with two cases and zero assertion errors:

```text
runx harness ./skills/postmortem-maker --json
status=passed
case_count=2
assertion_error_count=0
receipt_refs=sha256:c32a641142a378bcf2fc64b39615813a4ad6c271bc5ce3cece072bbc4dbb2514, sha256:1e8d6add7aa953f096acc5811e5d157a0e50b241cafc2ff688e33ce3dc0c6608
```

The receipts above cover checked-in fixtures only. This draft still does not
claim a clean `runx add`, a hosted harness, registry publication, or a
post-publish `runx skill` dogfood. No login, registry write, PR, remote push,
external provider send, or funds operation was attempted.

The local fixture receipt tree was separately checked with `runx verify` using
the matching temporary public key and returned `valid=true`. This local check
does not satisfy the post-publish real-source receipt requirement.

## Prepublish real-source dogfood

Before publication, the local package was run against the public GitHub issue
`https://github.com/kubernetes/kubernetes/issues/141080` using the runtime
GitHub API reader, not a checked-in fixture. The graph sealed with a confirmed
root cause, two source events, zero unknowns, an executed local-outbox
send-as-equivalent action, `provider_act_performed=true`, and a digest-matching
readback. The root receipt was
`sha256:70b368ab108417b19092cb77e2f50b98d29f1f9623f87c426af2fbc48e73bb44`;
the production-mode `runx verify` tree returned `valid=true`.

This is intentionally labeled prepublish evidence. It proves the corrected
local implementation can consume a real source and perform the sealed local
workflow, but it is not the required post-publish owner/version dogfood.

The package manifest digest used only for this draft is
`sha256:3a3239dbeaf28cbe1f7ba1c6ef6e02f31754b566a1a53c611021842f27bf5ced`.
It describes the earlier frozen AJP review package and is not a hosted
registry digest or a substitute for a public source revision. Because the
GitHub issue-body fix and its regression test were added afterward, the
pending AJP approval package must be regenerated before anyone approves it.

## Human completion checklist

An authorized operator with the real CLI and a public source repository must
complete the following outside this restricted workspace, then replace the
null fields in `evidence/evidence.json` and `evidence/verification.json` with
captured values:

```text
runx harness ./skills/postmortem-maker
runx login --provider github --for publish
runx registry publish ./skills/postmortem-maker/SKILL.md --registry https://api.runx.ai
runx add <owner>/postmortem-maker@<version> --registry https://api.runx.ai
runx registry read <owner>/postmortem-maker@<version> --json
runx skill <owner>/postmortem-maker@<version> --registry https://api.runx.ai --json ...
runx verify --receipt <receipt.json> --json
```

The dogfood input must be a real public incident/thread URL or a real
incident/ticket read projection read at runtime, not either checked-in fixture.
The evidence packet must then use the same package version and source revision
for the registry ref, PR head, raw `X.yaml`, raw `SKILL.md`, receipt, and
report before an authorized operator submits the platform preflight.
