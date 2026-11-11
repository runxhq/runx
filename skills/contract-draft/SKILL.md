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
   missing, repeated or unknown clauses all refuse. Titles and clause headings
   stay with the versioned template; they cannot be silently replaced. Missing
   placeholder values stop before drafting and cannot be excused as deviations.
   Clause text is compared exactly, including meaningful whitespace.
5. A drafted verdict carries the document and a send proposal gated on a human
   approver through `message-send`. `delivery_performed` is always false.

## Output

`contract_draft` (`runx/contract-draft.draft.v1`) carries `decision`, `reason`, the
`document` or null, confirmed `deviations` with baseline and text, the gated
`send_proposal` or null, `validation`, and the three input digests.

Inputs are `template`, `parties`, and `terms`. Inspection exposes their nested
shape and the exact agent result. `template` includes `id`, `version`, `title`,
`required_terms` and ordered `clauses` with unique `id`, `heading` and `baseline`.
`parties` maps explicit party names to records; `terms` maps the agreed deal keys
to supplied values. A placeholder such as `{{customer.legal_name}}` or
`{{terms.fee_usd}}` must resolve to a scalar or a list of scalars. Nulls, missing
paths, objects and unsupported expressions are unresolved evidence, not words
the drafting agent may invent.

The returned document contains every complete clause in template order, ready
for the host to display or render for review. It is not a signed agreement or
proof of legal suitability. Keep its template/version, party and term bindings
and explicit deviations with the text. An upstream intake may supply those
values once; this method does not fetch another template or renegotiate them.
Reuse the completed draft directly when only a later delivery step remains.

On refusal, correct the specific source or deviation finding and draft again
from the same intended template. Do not describe a null document as completed
work. Delivery still needs a real destination, exact content and the approval
required by the selected communication lane; a send proposal creates no
approval and performs no delivery.

## Agent task: contract-draft-assemble

Read `template`, `parties`, and `terms` from step inputs. Return `draft_doc`
with `title`, `clauses` (each `id`, `heading`, `text`), and `deviations` as an
array of `clause_id` plus `reason` entries. Render every placeholder from
the supplied values; never invent parties, amounts, or dates. First render the
baseline with those values, then compare the draft text to that rendered baseline.
Replacing placeholders with supplied values is not a deviation. Return
`deviations: []` when every clause matches its rendered baseline. Declare only
actual changes to the rendered text, justified by supplied terms, with a reason.
Do not add clauses the template does not define and
do not change the title or headings, repeat clause ids, or produce delivery instructions; delivery belongs to the gated proposal.
