---
name: postmortem-maker
description: Reconstruct a source-cited incident postmortem and execute a bounded, digest-bound publication only when the source and policy both allow it.
runx:
  category: ops
  tags:
    - incident-response
    - postmortem
    - evidence
    - send-as
---

# Postmortem Maker

Turn one incident source into a traceable postmortem. The skill preserves the
difference between what the source says, what remains uncertain, and what a
publication policy permits. It never turns a hypothesis into a fact.

## Inputs

The typed input is an `incident_source` handle plus a `postmortem_policy`.
`publish_target` is optional and is required only when the policy authorizes a
publication.

`incident_source` accepts these forms:

* `{ "kind": "thread_url", "url": "https://github.com/OWNER/REPO/issues/123" }`
  reads a public GitHub issue and its comments at run time. The read is bounded
  to the issue and the first 100 comments; the source digest binds the exact
  normalized events that were read.
* `{ "kind": "github_issue", "url": "https://api.github.com/repos/OWNER/REPO/issues/123" }`
  is the equivalent explicit API form.
* `{ "kind": "read_projection", "ref": "incident://...", "projection": { ... } }`
  accepts a projection already materialized by an upstream governed incident
  or ticket read. The projection must contain a title and non-empty `events`.
* `{ "kind": "fixture", "ref": "fixtures/.../thread.json" }` and
  `{ "kind": "inline", "ref": "inline://...", "thread": { ... } }` are
  deterministic harness-only forms. They are not evidence of a real-source
  dogfood run.

Every event must have a stable `id` and non-empty `text`. `at`, `author`, and
`url` are carried through when present. Timeline entries and root-cause
claims quote the exact event text and include the event id, author, time, and
source URL as evidence.

`postmortem_policy` supports:

* `require_confirmed_root_cause` — defaults to `true`; a suspected or
  competing cause cannot authorize publication.
* `require_action_items` — when `true`, at least one source-stated action item
  is required.
* `allow_publish` — defaults to `false`; it must be explicitly `true` before
  the send-as-equivalent transport can execute.

An optional `publish_target` has `data_source_ref`, `channel`, and
`aggregate_id`. It may also include `principal`, `audience`, `classification`,
and `visibility`. These values are copied into the bounded send plan; they do
not grant authority by themselves.

## Procedure

1. Read the source at execution time and bind the normalized event set to a
   SHA-256 source digest.
2. Build one timeline entry per readable event. Classify impact, mitigation,
   hypotheses, observations, and action items without changing the evidence
   quote.
3. Confirm a root cause only when one causal candidate is stated without
   hedge language and no distinct candidate competes with it. Hedged or
   conflicting candidates are recorded in `unknowns` and block publication.
4. Emit `postmortem` with `summary`, `timeline`, `impact`, `root_cause`, and
   `status`, plus `unknowns` and source-grounded `action_items`.
5. If the evidence is complete and `allow_publish` is explicitly true,
   compose a send-as-shaped plan. The bundled local outbox is an equivalent
   sealed comms transport: it performs a compare-and-set append, binds the
   exact postmortem content digest, enforces idempotency, and returns an
   executed `publish_result`.
6. Re-read the outbox and re-check the content and postmortem digests. A
   withheld path proves the absence of a send plan, provider act, and delivery
   for the selected aggregate.

The transport is deliberately local and append-only. It is not a claim that a
hosted Slack, email, or ticket provider delivered a message. An operator who
needs a provider send should use a separate approved provider adapter after
reviewing this sealed packet.

## Output contract

The final graph result is `runx.postmortem-maker.result.v1`:

* `postmortem` — the source-cited postmortem packet. `root_cause.status` is
  `confirmed` or `unconfirmed`; `postmortem.status` is `publishable` or
  `needs_more_evidence`.
* `unknowns[]` — objects with `id`, `detail`, and any supporting `evidence`.
  Missing evidence is never silently filled in.
* `action_items[]` — objects with `id`, `title`, `owner`, `evidence`, and
  `policy_rule`. `owner` is `null` when the source did not state one.
* `send_plan` — authorized or withheld, shaped like the `send-as` planning
  boundary.
* `publish_result` — the executed, digest-bound result when publication ran;
  otherwise `null`.
* `readback` and `verification` — independent outbox evidence.

## Stop conditions

Stop with a sealed refusal or failed step when the source handle is malformed,
the public GitHub source is not HTTPS or not a GitHub issue, the projection has
no readable events, event ids are duplicated, the outbox has an invalid shape,
the compare-and-set version changed, or an idempotency key is reused for
different content.

Withhold publication when the root cause is unconfirmed, impact or mitigation
is still unknown, required action evidence is absent, or
`postmortem_policy.allow_publish` is not explicitly true. A withheld run is a
valid evidence result, not a successful send.

Do not place credentials, cookies, authorization headers, private incident
body data, or ambient environment values in inputs or outputs. The GitHub read
uses no credential and the receipt-facing data contains only bounded source
events, hashes, and redacted metadata.

## Install, run, and verify

After a maintainer publishes a version, a new operator can use the package
without private context:

```text
runx add <owner>/postmortem-maker@<version> --registry https://api.runx.ai

runx skill <owner>/postmortem-maker@<version> --registry https://api.runx.ai --json \
  --input-json incident_source='{"kind":"thread_url","url":"https://github.com/OWNER/REPO/issues/123"}' \
  --input-json postmortem_policy='{"require_confirmed_root_cause":true,"allow_publish":false}' \
  --input-json publish_target='{"data_source_ref":"local://postmortems/review","channel":"incident-review","aggregate_id":"incident-123"}' \
  -R ./receipts

runx verify --receipt ./receipts/<sealed-receipt>.json --json
```

Only set `allow_publish:true` after the operator has reviewed the source and
intends to append the postmortem to the explicit local outbox target. The
receipt's `publish_result` and `readback` are the evidence for the executed
same-run send plan; a proposal without those fields is not a publication.
