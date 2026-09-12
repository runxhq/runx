---
name: crm-cleanup
description: Fetch current CRM records, reconcile a call transcript into allowlisted updates, execute them through a bounded CRM transport, and return source, decision, and write evidence.
---

# CRM Cleanup

Clean CRM follow-up data after a call without letting the model invent records,
quotes, or write authority. The skill fetches the current CRM record source,
derives only allowlisted updates supported by transcript evidence, executes the
result through the configured CRM transport, and seals the before/after write
evidence.

The default transport is `mock-crm`. It is intentionally deterministic: it
applies the exact field updates to the fetched records in memory and returns
the resulting before/after packet. Real CRM adapters can replace that transport
behind the same boundary without changing the reconciliation policy.

## Procedure

1. Fetch `crm_source_url` over public HTTPS inside the bounded runner.
2. Parse the fetched JSON source. It must contain `records`, and every record
   must have an `id`.
3. Digest the transcript and parsed records so downstream evidence is bound to
   the exact source content.
4. Reconcile the transcript against the fetched records and
   `crm_schema.allowed_fields`. The implementation only updates recognized
   fields when a transcript quote supports the change.
5. Route hedged or uncertain transcript evidence to `needs_review`; do not
   execute a CRM write when confidence is low.
6. Execute proposed updates through `write_transport`. For `mock-crm`, the
   write is consumed immediately and returns `before_records`, `after_records`,
   `applied_count`, and `idempotency_key`.
7. Finalize `crm_cleanup_result` with source provenance, takeaways, field
   updates, confidence, review gate, write evidence, digests, and validation
   findings.

## Safety Rules

- Do not write fields outside `crm_schema.allowed_fields`.
- Do not update a record that is not present in the fetched CRM source.
- Do not write when the transcript uses uncertain language such as "might",
  "maybe", "not sure", or "unclear"; return `needs_review` instead.
- Do not emit an executed result unless the write transport reports
  `status: executed`.
- Do not claim a no-op as a write; no-action runs return a skipped write.
- Do not include secrets, CRM credentials, or private tokens in inputs or
  receipts.

## Output

`crm_cleanup_result` (`runx.crm_cleanup.result.v1`) contains:

- `decision`: `executed`, `no_action`, `needs_review`, or `refused`.
- `source_evidence`: source URL, final URL, content digest, and record count.
- `takeaways`: concise transcript-backed observations.
- `field_updates`: each update with `record_id`, `field`, `from`, `to`, and
  `confidence` plus `evidence_quote`.
- `confidence`: overall confidence level, score, and reasons.
- `review_gate`: whether human CRM review is required before any write.
- `hosted_harness_status`: honest local/hosted validation status.
- `write_result`: CRM transport evidence, including before/after records and
  idempotency key.
- `transcript_digest` and `records_digest`.
- `validation`: pass/fail plus findings.

## Inputs

- `crm_source_url`: public HTTP(S) source of current CRM records.
- `transcript`: call transcript to reconcile.
- `crm_schema.allowed_fields`: exact fields the skill may update.
- `write_transport`: currently `mock-crm`.
- `idempotency_key`: stable retry key for the write attempt.
