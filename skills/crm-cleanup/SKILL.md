---
name: crm-cleanup
description: Read current CRM records from a real source, reconcile a call transcript against them, and execute the allowlisted field updates through a CRM transport in the same run, sealing a before/after write result bound to the decision.
---

# CRM Cleanup

Keep pipeline data from rotting after calls. The skill proves the whole
read → decide → write loop in one sealed run: it reads the current CRM
records from a real source at run time, reconciles the call transcript
against them, enforces the update authority deterministically, and executes
the decided updates through a CRM transport that seals a before/after write
result bound to the decision — not an inert proposal object nothing consumes.

## Procedure

1. `fetch-source` reads the typed source handle's https URL through native
   `web.fetch` under the caller-supplied host allowlist. A `read_projection`,
   `connector_export`, or `web_fetch` handle carries the URL and its
   `allowlist`; nothing else is admitted as a record source.
2. `normalize-records` validates the source read into the working record set
   and seals its origin (`source_read.kind`, `ref`, `count`).
3. Native `data.digest` binds the exact transcript and the exact record set
   that was read.
4. The reconciling agent proposes takeaways and field updates from the
   transcript, each with the target record, field, new value, and a
   supporting quote.
5. `decide` enforces every proposal deterministically: the record must exist
   in the set that was read, the field must be inside
   `crm_schema.allowed_fields` (out-of-allowlist updates are rejected with a
   named reason, not silently dropped), the quote must appear verbatim in the
   transcript, and the value must be non-empty. An unknown record or an
   invented quote refuses the whole run; nothing partial escapes.
6. `execute-writes` runs only on an `apply` decision. The CRM transport
   (`mock-crm.v1`) re-checks each update's `from` value against the records
   that were read — drift aborts the write — then applies the updates and
   seals `write_result` with `before`, `after`, the `applied` list, a
   `write_ref`, and the decision's digests as its binding.
7. `finalize` seals the typed result: `takeaways`, `field_updates` keyed to
   `crm_schema` fields, and `write_result{before,after}` from the executed
   transport. A run with no supported updates seals `no_action` and executes
   nothing — the transport step never runs.

## Output

`crm_cleanup_result` (`runx.crm_cleanup_result.v1`) carries `decision`
(`applied`, `no_action`, `refused`), `takeaways`, `field_updates` with
before/after values and evidence quotes, `rejected_updates`, `write_result`
(`runx.crm_write_result.v1` with `executed`, `transport`, `write_ref`,
`applied`, `before`, `after`; `null` when nothing executed), `source_read`
(`kind`, `ref`, `count`), `validation`, and both input digests.

Inputs are `crm_source` (the typed source handle), `transcript`, and
`crm_schema`.

## Agent task contracts

`crm-cleanup-reconcile` receives the transcript, the records read from the
source, and the update authority; it returns `update_draft` with `takeaways`
and candidate `updates` (`record_id`, `field`, `to`, `evidence_quote`). The
draft is advisory only — deterministic code decides what executes.
