---
name: crm-cleanup
description: Reconcile a call transcript against current CRM records and propose allowlisted field updates, each traced to a verbatim transcript quote, with the write left to a gated CRM operator.
---

# CRM Cleanup

Keep pipeline data from rotting after calls without letting an agent write to
the CRM on vibes. The transcript and the current records are the only
evidence; the reconciling agent proposes updates, and deterministic code
enforces the update authority and the evidence trail. The skill never writes;
the sealed proposal is what a gated CRM operator or human approver consumes.

## Procedure

1. Native `data.digest` binds the exact transcript and record set.
2. The reconciling agent proposes field updates from the transcript, each with
   the target record, field, new value, and a supporting quote.
3. Deterministic enforcement checks every proposal: the record must exist in
   the supplied set, the field must be inside `crm_schema.allowed_fields`
   (out-of-allowlist updates are rejected with a named reason, not silently
   dropped), the quote must appear verbatim in the transcript, and the value
   must be non-empty. An unknown record or an invented quote refuses the whole
   run; nothing partial escapes.
4. The proposal packet carries each update's `from` value from the supplied
   records so the downstream write can detect drift before applying.

`write_performed` is always false and the packet names its gate
(`crm-operator-or-human-approver`). A run with no supported updates seals
`no_action` rather than inventing work.

Two more deterministic guarantees keep proposals trustworthy:

- **Idempotent proposals.** Identical updates (same record, field, value, and
  quote) are deduplicated. A reconciling agent that repeats itself cannot
  inflate the proposal.
- **Conflict refusal.** Two different target values for the same record and
  field refuse the whole run with a named finding
  (`update.conflicting_values`). The skill never picks a winner silently;
  the conflict goes back to the operator or human approver.

Non-string values (numbers, booleans) are supported as update targets and
carried through with their types intact.

## Apply runner

The `apply` runner executes a sealed proposal through a mock CRM transport.
It takes `crm_update_proposal` and `crm_records`, applies each update in
memory, and returns `crm_write_result` (`runx.crm_write_result.v1`) with
`executed`, `write_result{before,after}`, and a transport log. A `no_action`
or `refused` proposal executes nothing. The mock transport stands in for a
real CRM connector; swap it for your connector in production.

## Output

`crm_update_proposal` (`runx.crm_update_proposal.v1`) carries `decision`
(`proposed`, `no_action`, `refused`), `updates` with before and after values
and evidence quotes, `rejected_updates`, `validation`, and both input digests.

Inputs are `transcript`, `crm_records`, and `crm_schema`.

## Agent task contracts

### `crm-cleanup-reconcile`

Read `transcript`, `crm_records`, and `crm_schema` from step inputs. Return
`update_draft` with `updates`: an array of `record_id`, `field`, `to`, and
`evidence_quote` entries. Only propose updates the transcript actually
supports, quote the transcript verbatim, and only target fields inside the
allowlist. Return an empty `updates` array when the call changes nothing.
Never invent records, quotes, or values.
