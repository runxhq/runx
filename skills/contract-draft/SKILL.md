---
name: contract-draft
description: Draft a contract from a versioned template with explicit parties and terms, reconciling every clause against the rendered baseline deterministically and keeping delivery behind a human gate.
---

# Contract Draft

Draft a contract without letting the drafting agent silently rewrite the
template. The template, parties, and terms are the only evidence; the agent
assembles clause text, and deterministic code re-renders every baseline with
the supplied values and reconciles the draft against it. A clause may differ
from its rendered baseline only through a declared deviation with a reason.

This is not legal advice and does not replace counsel. It is an execution
boundary for template fidelity: the receipt proves which template version was
drafted, which terms were bound, and which deviations were declared.

## Procedure

1. Admission requires a complete template (id, version, title, clauses with
   baselines), explicit parties, and every `required_terms` key present in
   `terms`. Missing evidence stops before the drafting agent runs.
2. Native `data.digest` binds the exact template, parties, and terms.
3. The drafting agent assembles clause text and declares any deviations.
4. Deterministic reconciliation re-renders each baseline with the supplied
   values (`{{provider.legal_name}}`, `{{terms.fee_usd}}`, and similar paths).
   A clause must match its rendered baseline exactly, or carry a declared
   deviation with a reason. Undeclared deviations, deviations that match the
   baseline anyway, unknown clause targets, unresolved placeholders, and
   missing clauses all refuse.
5. A drafted verdict carries the document and a send proposal gated on a human
   approver through `message-send`. `delivery_performed` is always false.

## Output

`contract_draft` (`runx.contract_draft.v1`) carries `decision`, `reason`, the
`document` or null, confirmed `deviations` with baseline and text, the gated
`send_proposal` or null, `validation`, and the three input digests.

Inputs are `template`, `parties`, and `terms`.

## Agent task contracts

### `contract-draft-assemble`

Read `template`, `parties`, and `terms` from step inputs. Return `draft_doc`
with `title`, `clauses` (each `id`, `heading`, `text`), and `deviations` as an
array of `clause_id` plus `reason` entries. Render every placeholder from
the supplied values; never invent parties, amounts, or dates. Match each
baseline exactly unless a deviation is genuinely needed, and declare every
deviation with its reason. Do not add clauses the template does not define and
do not produce delivery instructions; delivery belongs to the gated proposal.
