---
name: crm-update-plan
description: Reconcile a call transcript against current CRM records and propose allowlisted field updates, each traced to a verbatim transcript quote, with the write left to a gated CRM operator.
---

# CRM Update Plan

Keep pipeline data from rotting after calls without letting an agent write to
the CRM on vibes. The transcript and the current records are the only
evidence; the reconciling agent proposes updates, and deterministic code
enforces the proposal allowlist and exact quotation trail. Quote presence does not independently establish the interpretation or field semantics. The skill never writes;
the sealed proposal is what a gated CRM operator or human approver consumes.

## Procedure

1. Native `data.digest` binds the exact transcript and record set.
2. The reconciling agent proposes field updates from the transcript, each with
   the target record, field, new value, and a supporting quote.
3. Deterministic enforcement checks every proposal: the record must exist in
   the supplied set, the field must be inside `crm_schema.allowed_fields`
   (out-of-allowlist updates are rejected with a named reason, not silently
   dropped), the quote must appear verbatim in the transcript, and each record-field pair can have only one final proposed value. Duplicate record identities, conflicting field updates, an unknown record or an invented quote refuse the whole
   run; nothing partial escapes.
   Deliberate clearing may use null or an empty string when the transcript explicitly supports that CRM operation; false and zero remain valid field values.
4. The proposal packet carries each update's `from` value from the supplied
   records so the downstream write can detect drift before applying.

`write_performed` is always false and the packet names its gate
(`crm-operator-or-human-approver`). A run with no supported updates seals
`no_action` rather than inventing work.

## Output

`crm_update_proposal` (`runx/crm-update-plan.proposal.v1`) carries `decision`
(`proposed`, `no_action`, `refused`), `updates` with before and after values
and evidence quotes, `rejected_updates`, `validation`, and both input digests.

Inputs are `transcript`, `crm_records`, and `crm_schema`. The caller selects the relevant records from its existing CRM reader or supplies a current snapshot. This skill does not discover accounts or add a provider client. Keep the original source/fetch time and transcript alongside the returned digests.

The downstream CRM writer is selected by the operator. Before a write it must reread the target, compare the original snapshot and each before value, validate the proposed value against the provider field schema, and apply its native authority and approval policy. A sealed proposal is not permission or evidence that any field changed. Do not lose a supported change merely because no writer is configured: retain this reviewable proposal until one is selected.

## Agent task: crm-update-plan-reconcile

Read `transcript`, `crm_records`, and `crm_schema` from step inputs. Return
`update_draft` with `updates`: an array of `record_id`, `field`, `to`, and
`evidence_quote` entries. Only propose updates the transcript actually
supports, quote the transcript verbatim, and only target fields inside the
allowlist. Choose one final value for each record-field pair after considering the full call. Use null or an empty string only for an explicit supported clear. Return an empty `updates` array when the call changes nothing.
Never invent records, quotes, or values.
