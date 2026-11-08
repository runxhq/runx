---
name: privacy-request-review
description: Judge a data subject erasure or export request against explicit policy evidence, record the verdict durably through the store, and emit only a bounded handoff for a separate governed downstream run.
---

# Privacy Request Review

Judge whether a data subject request is in policy without erasing, exporting,
or sending anything. The skill consumes a request packet, metadata of an upstream-verified requestor assertion, and
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
   requested data class and scope must sit inside both `scope_bounds.data_classes` and
   the type-specific allowed list. Explicit requested scopes must also be within
   the requested data classes. These names are exact policy identifiers, not a
   wildcard or implicit subfield language. A mismatch refuses with named reasons;
   nothing is inferred.
3. The verdict, eligible or refused, is appended to the pinned store
   stream with optimistic concurrency and an idempotency key, so repeated runs
   cannot double-record. A verdict decides something about a person, so the
   append waits on a person's approval of the exact event (`privacy_verdict`),
   whatever the run's `effects`.
4. An independent `events.read` reads the exact original event position.
   Finalization requires a committed/replayed append at the expected version
   and matching source, event id, version and digest. A conflict cannot become
   a committed verdict. The final packet retains both native input digests.
5. An eligible result carries the exact subject/classes/scopes but no selected
   execution path. The operator must choose the owning product's erasure or
   secure subject-export capability. A refusal carries human review and no
   handoff. Do not invent a generic read/redact/send pipeline.

`downstream_effect_performed` is always false; the downstream operator owns the
actual erasure or export under its own authority and receipt.

## Output

`subject_request_verdict` (`runx/privacy-request-review.verdict.v1`) carries `request`,
`decision`, `escalation`, `legal`, `requestor`, `scope_bounds`, `handoff` or
null, `persistence` with the committed stream version, and both input digests.

Inputs are `request_packet`, `requestor_proof`, `policy`, and the store
binding (`data_source_uri`, `resource`, `aggregate_id`, `expected_version`,
`idempotency_key`).

## Trust and continuation

The host must obtain and verify identity through its actual identity provider
before supplying assertion metadata. A timestamp, trusted provider name and
digest-shaped string do not independently authenticate anyone. This review
checks the supplied bindings against policy and records that conditional
judgment; it performs no provider verification or legal determination.

Retain the original assertion/source provenance, policy, request and stable key
with the result. Delivery must bind the actual verified requester and secure
destination. Export the subject's entitled data: blindly applying PII redaction
to that subject's own records can defeat the request. Apply third-party
redaction or withholding only under the selected product's explicit policy.

A stale append returns a failure with no committed verdict. Inspect the current
stream and preserve the original identity; do not silently retry a new write.
Readback checks the original event position, so later unrelated events do not
invalidate confirmation of an already committed verdict. A committed policy
verdict is never authorization or proof of erasure, export or delivery.
