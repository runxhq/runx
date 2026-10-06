# Postmortem Maker — Bounty #83 Delivery Report

## What was shipped
`astra-intelligence/postmortem-maker` — a published runx graph skill that turns a resolved incident record into a source-cited postmortem and executes a sealed outbox delivery only when the evidence settles the cause.

## Package facts
- Package: astra-intelligence/postmortem-maker@sha-279a27f325fd
- Registry: https://api.runx.ai/skills/astra-intelligence/postmortem-maker
- PR: https://github.com/runxhq/runx/pull/530
- runx CLI: runx-cli 0.9.1 (>= 0.6.14 requirement) — verified via `runx --version`

## How the skill works
1. `read-source` agent-task reads the incident record from a **real source at run time**: `web.fetch` for an incident thread URL, `data.read_projection` for a ticket/incident event-stream projection (allowed_tools + scopes declared).
2. `digest-source` binds the exact fragment set with `data.digest`.
3. `draft` agent-task separates facts from hypotheses into a postmortem draft.
4. `finalize` deterministically enforces every citation: timeline entries and non-unknown root causes must quote an existing fragment verbatim; invented citations refuse the run.
5. When the postmortem is publishable (no unknowns, supported root cause) and `postmortem_policy.allow_publish` is true, `prepare-delivery`/`deliver`/`record-delivery` execute a **sealed outbox write** (fs.write) bound to the fragments digest and emit `publish_result` recording the executed send plan. Otherwise the send plan is withheld and nothing is published.

## Harness (4 sealed cases, green locally + after registry install)
1. postmortem-maker-publishable-seals — consistent evidence → publishable + **executed** sealed delivery (record-delivery emits publish_result)
2. postmortem-maker-unknowns-block-publish — insufficient evidence → needs_more_evidence, send plan withheld, nothing published
3. postmortem-maker-invented-citation-refuses — fabricated citation → refused, nothing published
4. postmortem-maker-needs-fragments — empty record → refused

## Dogfood (real source read at run time)
- Input: inc-libgit2-issue-6521 / web_fetch https://api.github.com/repos/nltk/nltk/issues/3733 (real closed GitHub issue thread; live fetch + comments)
- Result: sealed run `run_make_c9135b4e01cb7501`, receipt sha256:9cd34dd39c08709722f031e82120248128ddbb68b93d06b937506d7d03fde9c4, decision publishable
- Executed outbox write: outbox/inc-libgit2-issue-6521.postmortem.json
- `runx verify` verdict: **valid** (with --allow-local-development-signatures for the local fixture signing identity)

## How a new user installs, runs, and verifies
```
runx add astra-intelligence/postmortem-maker@sha-279a27f325fd --registry https://api.runx.ai
runx skill astra-intelligence/postmortem-maker@sha-279a27f325fd --registry https://api.runx.ai --inputs - <<'JSON'
{"incident_ref":"inc-1","incident_source":{"kind":"inline","fragments":[{"id":"f1","source":"log","text":"The batch failed at export."}]},"postmortem_policy":{"allow_publish":true,"outbox_dir":"outbox"}}
JSON
runx verify --receipt <receipt.json> --json
```
