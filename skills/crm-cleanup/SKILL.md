---
name: crm-cleanup
description: Reconcile a call transcript against live CRM records, then execute the decided allowlisted field updates through a CRM transport in the same run, sealing a before/after write_result.
---

# CRM Cleanup

Keep pipeline data from rotting after calls. The skill reads the current CRM
records from a real source at run time (a `web.fetch` connector export or a
`data.read_projection` event stream — never a hand-pasted fixture), reconciles
the transcript against them, and executes the decided updates through a CRM
transport in the same run, sealing a before/after `write_result`.

## Procedure

1. `read-source` reads the current CRM records at run time. `crm_source` picks
   the channel: `{kind: web_fetch, url}` for a connector export over HTTPS,
   `{kind: read_projection, ...}` for a data-store projection, or
   `{kind: inline, records}` for deterministic harness runs.
2. Native `data.digest` binds the exact transcript and record set.
3. The reconciling agent drafts takeaways and candidate updates from the
   transcript, each with the target record, field, new value, and a supporting
   verbatim quote.
4. Deterministic enforcement checks every proposal: the record must exist in
   the source read, the field must be inside `crm_schema.allowed_fields`
   (out-of-allowlist updates are rejected with a named reason), the quote must
   appear verbatim in the transcript, and the value must be non-empty. An
   unknown record or an invented quote refuses the whole run.
5. When updates survive, `prepare-write` applies them to the source records
   and binds the payload to `write_target`; `apply-write` executes the write
   through the configured CRM transport (the bundled mock transport is
   `fs.write` to an outbox file); `record-write` seals `write_result` with the
   before/after of every applied field bound to the decision digest.
6. A run with no supported updates seals `no_action` and executes nothing —
   the write steps stay withheld and no `write_result` is emitted.

## Output

`crm_cleanup` (`runx.crm_cleanup.v2`) carries `decision` (`applied`,
`no_action`, `refused`), `takeaways`, `field_updates` keyed to `crm_schema`
fields with before/after values and evidence quotes, `rejected_updates`, the
`write_plan`, the `source` handle that was actually read, both input digests,
and `validation`. On an applied run, `write_result` (`runx.write_result.v1`)
carries the executed `before`/`after` pairs from the transport.

Inputs are `transcript`, `crm_source`, `crm_schema`, and `write_target`.

## Agent task contracts

### `crm-cleanup-read`

Read `crm_source` from step inputs. Use `web.fetch` when `kind` is
`web_fetch` (fetch `url` over HTTPS and return the record array it carries),
`data.read_projection` when `kind` is `read_projection`, or replay `records`
verbatim when `kind` is `inline`. Return `source_read` with `read_kind`,
`fetched_ref` (the URL or projection ref actually read, `inline:<name>` for
inline), and `records` — the current CRM records as an array. Never invent
records.

### `crm-cleanup-reconcile`

Read `transcript` and `crm_schema` from step inputs and `crm_records` from
step context. Return `update_draft` with `takeaways` (short strings
summarizing what the call established) and `updates`: an array of
`record_id`, `field`, `to`, and `evidence_quote` entries. Only propose
updates the transcript actually supports, quote the transcript verbatim, and
only target fields inside the allowlist. Return an empty `updates` array when
the call changes nothing. Never invent records, quotes, or values.
