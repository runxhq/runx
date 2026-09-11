---
name: crm-cleanup
description: Read current CRM records from a live source handle, reconcile a call transcript into allowlisted field updates, and execute them through a mock CRM transport that seals a before/after write result.
---

# CRM Cleanup

Keep pipeline data from rotting after calls without writing on vibes. The
skill reads the current records from a real source at run time (a data-store
read_projection URL, a connector export, or a web-fetch), treats the
transcript as the only evidence, decides allowlisted field updates, and
executes them through a CRM transport in the same sealed run.

## Procedure

1. Native `web.fetch` reads `crm_source.url` against `crm_source.allowlist`.
   The handle kind must be `read_projection`, `connector_export`, or
   `web_fetch`. Records are never accepted as a pasted fixture argument.
2. Native `data.digest` binds the exact transcript and the fetched record
   body.
3. Deterministic code proposes field updates only when a verbatim transcript
   quote supports them: stalled-renewal language sets `account_status` to
   `at_risk`, `Next step agreed:` sets `next_action`, and `move the account
   to` sets `owner`.
4. Fields outside `crm_schema.allowed_fields` are rejected with a named
   reason, not silently dropped. An invented quote or unknown record refuses
   the whole run.
5. The mock CRM transport (`mock-crm.v1`) applies the decided updates in the
   same run and seals `write_result{before,after}`. A no-op transcript
   executes nothing (`executed: false`) instead of inventing work.

## Output

Typed outputs are `takeaways`, `field_updates` keyed to `crm_schema` fields,
and `write_result` from the executed transport. `crm_cleanup_result`
(`runx.crm_cleanup_result.v1`) also carries `decision` (`applied`,
`no_action`, `refused`), `rejected_updates`, `source_read`, both input
digests, and validation findings.

Inputs are `crm_source`, `transcript`, and `crm_schema`.
