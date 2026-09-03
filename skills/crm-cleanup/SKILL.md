---
name: crm-cleanup
description: Read current CRM records, extract evidence-backed updates from natural call transcripts, validate them against a bounded CRM schema, and execute only unambiguous changes through a receipt-backed mock transport.
---

# CRM Cleanup

Turn post-call notes into a bounded CRM update without trusting stale pasted
records or claiming a write that never happened. This package reads the current
record during the run with native `web.fetch`, uses a bounded semantic extraction
step to propose schema-keyed candidates from ordinary transcript prose, and then
validates every candidate deterministically before a transport can consume it.
The receipt seals the source digest, exact evidence quotes, review items, and the
transport's before/after records.

The mock transport is deliberate: it demonstrates the complete read/write
control loop without possessing credentials or mutating a production CRM.
Harness cases bind deterministic HTTP responses to the public connector URL so
registry verification does not depend on third-party uptime. A live run still
fetches that URL through the native network effect.

## Inputs

- `source_handle`: a typed connector-export handle with `url`, `allowlist`, and
  the `record_id` to reconcile. The URL must return JSON containing either a
  top-level record array or `{ "records": [...] }`.
- `transcript`: ordinary call notes or transcript text. Explicit
  `CRM update: field=value` and `Takeaway: ...` lines remain supported, but are
  not required; natural statements such as “the health score dropped to 48”
  may be extracted when the supporting quote and value are unambiguous.
- `crm_schema`: the record ID field plus an object of writable field
  definitions. Supported value types are `string`, `number`, and `boolean`;
  definitions may also constrain enum values, lengths, or numeric ranges.
  Optional `aliases` and a `semantic_role` (`status`, `score`, or
  `next_action`) give the extraction step bounded semantic hints.

## Semantic extraction contract

The `crm-cleanup-extract` agent task receives only `transcript` and
`crm_schema`. It returns one `extraction_draft` object:

```json
{
  "takeaways": ["Exact or directly supported transcript takeaway"],
  "candidates": [
    {
      "field": "exact_schema_key",
      "to": "typed scalar candidate",
      "evidence_quote": "Exact non-hedged transcript substring"
    }
  ],
  "review_items": [
    {
      "field": "exact_schema_key or unmapped",
      "evidence_quote": "Exact unresolved transcript substring",
      "reason": "Why this cannot be written safely"
    }
  ]
}
```

Extract a candidate only when the transcript clearly states a current value or
committed next action, the field maps to exactly one declared schema key, the
value is scalar, and the evidence quote is exact and unhedged. Put possible
changes that are hedged, conflicting, underspecified, or not uniquely mappable
in `review_items`. Ordinary discussion and confirmed unchanged state produce no
candidate. Never invent a field, value, quote, or source record.

## Procedure

1. Digest the transcript and CRM schema with native `data.digest`.
2. Fetch `source_handle.url` with native `web.fetch`, enforcing the supplied
   host allowlist and a bounded response size.
3. Ask the bounded `crm-cleanup-extract` agent task for takeaways, candidate
   field/value pairs, exact transcript evidence quotes, and unresolved review
   items. This draft is advisory and cannot write.
4. Deterministically require exact-quote inclusion, schema-authorized field
   keys, scalar values, supported types, enum/range/length conformance, and one
   unambiguous value per field. Hedged, conflicting, malformed, or actionable
   but unmappable prose becomes `needs_review` and refuses the whole write.
5. Build `field_updates` as an object keyed by the corresponding
   `crm_schema.fields` key. Each changed field carries its prior value, new
   typed value, and the exact transcript quote as evidence.
6. A separate `write-through-transport` graph step consumes that same field
   map through the transport operation in `crm-cleanup.mjs`. If any value changed,
   it applies all changes atomically and reports `executed: true`. If no
   accepted candidate changed current state, it reports `executed: false`, with
   identical before and after records. Refused plans also pass through this step
   only as a verifiable no-op.
7. A final verifier checks the transport result against the reconciliation
   plan, then seals `takeaways`, `needs_review`, `field_updates`, `write_result`,
   source provenance, input digests, and validation findings in the declared
   `crm_cleanup_result` output.

## Output

`crm_cleanup_result` contains:

- `decision`: `updated`, `no_action`, or `refused`;
- `takeaways`: normalized transcript takeaways;
- `needs_review`: exact transcript excerpts that are hedged, conflicting, or
  not safely mappable to one writable field;
- `field_updates`: a schema-keyed object of evidence-backed changes;
- `write_result`: mock transport name, execution status, record ID, applied
  fields, and exact `before`/`after` records;
- `source_read`: connector kind, target record, allowlist, requested/final URL,
  HTTP status, source digest, fetch timestamp, and byte count;
- transcript/schema digests and deterministic validation findings.

## Stop conditions

- Do not accept records directly from the caller; the runtime source read is
  the current-state authority.
- Do not trust the semantic extraction draft by itself. It is accepted only
  after deterministic quote, field, value, and schema checks.
- Do not write hedged, conflicting, or actionable-but-unmapped prose. Route it
  to `needs_review`, clear `field_updates`, and seal a zero-write result.
- Do not partially apply a transcript. Any malformed, duplicated,
  out-of-schema, type-invalid, or unresolved item refuses the whole write.
- Do not describe the mock transport as a production CRM mutation.
- Never put credentials, customer data, or a private URL in a public fixture.

## Worked example

With a source record whose `account_status` is `healthy`, this transcript:

```text
Takeaway: Rollout is blocked pending an executive review.
Dana confirmed the account is now at risk.
We agreed to send the Q3 usage report by Friday.
```

produces two schema-keyed updates and a mock-transport result whose before
record remains `healthy` and whose after record is `at_risk`. Re-running with
“Dana confirmed the account remains healthy and no CRM changes are needed”
against the same source seals `no_action`; the transport executes no write and
before equals after. “The account might be at risk” instead seals `refused`,
places the quote in `needs_review`, and also executes no write.
