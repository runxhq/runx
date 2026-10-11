---
name: lead-qualify
description: Assess known person or company facts against supplied ICP criteria, keeping commercial fit separate from contact eligibility.
---

# Lead Qualify

Decide whether one lead meets the actual ICP. Use `lead-enrichment` for factual
context; use this skill for commercial judgment; use `cold-outreach` for copy.
Qualification neither obtains consent nor creates a provider send plan.

## Procedure

1. Supply the enrichment packet, explicit ICP criteria, and a separately sourced
   contact-eligibility decision. At least one criterion must be mandatory.
   The eligibility names the subject it decides, with the same `ref` and `kind`
   as the enrichment subject. Eligibility decided for any other subject
   requests evidence before any criterion is assessed.
2. Keep missing and conflicting facts visible. Partial enrichment can support
   a criterion only through its admitted, non-conflicting facts.
3. Assess every criterion as `met`, `not_met` or `unknown`. Cite exact factual
   field names for each supported assessment and explain its relevance.
4. A failed mandatory criterion disqualifies the lead. An unknown mandatory
   criterion requests evidence. All mandatory criteria met establish fit;
   optional criteria explain strengths and weaknesses without overriding it.
5. Return the fit independently from contact eligibility. A good fit with
   unknown or ineligible contact status remains a hold.

Never replace the supplied ICP with generic scoring. A company title is not
buying authority, activity is not consent, and a matched work email is not
permission to contact. Do not infer protected or sensitive personal traits.

## Result and composition

`lead_qualification` records the exact enrichment digest, subject, criteria,
assessments, contact-eligibility evidence and a bounded next action:
`prepare_outreach`, `request_evidence`, `resolve_contact_policy` or `stop`.
Contact eligibility appears only when it was decided for that subject; another
subject's eligibility is left out, and its blocker remains.
No message, CRM update, enrollment or durable suppression change occurs.

A qualified result is context for `cold-outreach#prepare`, not send authority.
That skill requires its own exact recipient-bound eligibility/suppression
input. The selected sending capability performs native approval and delivery.
The calling application owns task assignment, refresh, follow-up timing and
continuing work.

## Agent task contract

### `lead-qualify-assess`

Apply each supplied criterion to the admitted facts. Return one assessment per
criterion ID, with status, rationale and exact `fact_fields`. Use `unknown`
when the required evidence is absent or contradictory; it may cite no field.
Do not invent fact fields, change mandatory criteria, score on sensitive traits,
reinterpret contact eligibility or claim an external action.
