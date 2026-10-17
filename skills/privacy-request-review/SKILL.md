---
name: privacy-request-review
description: Judge a data subject erasure or export request against explicit policy evidence, record the verdict durably through the store, and emit only a bounded handoff for a separate governed downstream run.
---

# Privacy Request Review

Judge whether a data subject request is in policy without erasing, exporting,
or sending anything. The skill reads a request packet, requestor proof, and
policy bounds, decides eligibility deterministically, appends the verdict to
the subject request event stream through the store's `events.append`, and returns a bounded
handoff that a separate governed operator run may consume.

This is not a legal authority and does not replace counsel. It is an execution
boundary for policy evidence: the receipt proves which inputs were inspected,
which lawful basis was named, which scope was allowed or refused, and which
durable verdict was appended.

## Procedure

1. Native `data.digest` binds the exact request packet and policy.
2. Deterministic judgment: the request type must be erasure or export, the
   identity provider must be in `trusted_identity_providers`, the proof must
   carry a valid `verified_at` and a sha256 assertion digest bound to the same
   subject, a lawful basis must be named for the request type, and every
   requested data class must sit inside both `scope_bounds.data_classes` and
   the type-specific allowed list. Any failure refuses with every reason named;
   nothing is inferred.
3. The verdict, eligible or refused, is appended to the pinned store
   stream with optimistic concurrency and an idempotency key, so repeated runs
   cannot double-record.
4. The final packet binds the append evidence and both digests. An eligible
   verdict carries a bounded handoff (erasure operator, or
   read-projection through `redact-pii` into `message-send` for export); a refusal
   carries escalation to `human_privacy_review` and no handoff.

`downstream_effect_performed` is always false; the downstream operator owns the
actual erasure or export under its own authority and receipt.

## Output

`subject_request_verdict` (`runx.data_subject_request.v1`) carries `request`,
`decision`, `escalation`, `legal`, `requestor`, `scope_bounds`, `handoff` or
null, `persistence` with the committed stream version, and both input digests.

Inputs are `request_packet`, `requestor_proof`, `policy`, and the store
binding (`data_source_ref`, `resource`, `aggregate_id`, `expected_version`,
`idempotency_key`).
