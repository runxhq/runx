---
name: lead-enrichment
description: Enrich a lead from supplied account signals and produce a consent-aware, evidence-bound outreach recommendation.
---

# Lead Enrichment

Turn known lead, account, and engagement signals into a reviewable picture of
fit, confidence, risk, and the narrowest sensible follow-up. Good enrichment
does not mean filling every field. It means making useful claims only where the
evidence supports them and making “do not contact” or “learn more first” first-
class outcomes.

This is a supplied-signal synthesis skill. It does not scrape the web, query an
enrichment vendor, update a CRM, or send outreach. Upstream systems own source
collection; downstream routing and provider skills own action.

## When to use it

Use `lead-enrichment` when a product already has bounded signals and needs a
consistent, auditable assessment before routing a lead. It is useful for
combining product activity, declared firmographics, CRM facts, and consent state
without letting an agent silently invent the missing pieces.

Do not use it to infer sensitive traits, reconstruct personal profiles, or
manufacture permission from engagement. A recommendation is not consent and a
source digest is not proof that Runx independently verified the provider.

## How it works

1. Supply the known lead fields and typed signals with unique source references,
   upstream SHA-256 digests, claims, and observation times.
2. Deterministic admission checks provenance, freshness, duplicates, consent,
   suppression, region, and channel constraints before synthesis.
3. Opt-out and do-not-contact signals stop the lane immediately. The model never
   gets to reason its way around them.
4. Synthesis builds the lead profile, fit assessment, recommendation, and risk
   flags using only admitted lead fields and signals.
5. Finalization rejects invented source references and any language claiming
   outreach permission, CRM mutation, or send completion.

## Inputs and result

- `lead` contains known identity and account fields; unknown values remain
  unknown.
- `signals` contain stable `source_ref`, `source_digest`, type, claim, and
  `observed_at` fields.
- `as_of` and `max_age_days` establish a reproducible freshness decision.
- `constraints` carry consent, suppression, region, and allowed-channel state.

The result is a source-bound enrichment packet containing the supported profile,
evidence references, fit and confidence, risk flags, and a non-delivery
recommendation such as hold, review, nurture, or bounded follow-up. It can feed
`lead-router`, which must preserve the packet digest and cannot broaden consent.

## Stop conditions

- Stop on opt-out, do-not-contact, prohibited region, or conflicting suppression
  evidence.
- Return `needs_more_evidence` when the remaining signals are stale, too thin,
  or lack stable provenance.
- Never infer protected or sensitive traits, personal circumstances, or missing
  firmographic facts.
- Distinguish observed facts from inferred fit and express confidence honestly.
- Do not claim a CRM update, enrollment, approval, outreach, or delivery.

## Example

A lead has a verified company role, two recent product events, and an explicit
email opt-in. The packet may identify a plausible product fit and recommend a
bounded review for outreach, citing the three exact source references. It cannot
infer budget or buying authority. If the same lead has a suppression signal,
the result is do-not-contact regardless of apparent fit.

## Agent task contract

### `lead-enrichment-synthesize`

Use only admitted lead fields and signals. Return the supported profile,
evidence, fit assessment, recommended action, and risk flags. Every evidence
claim must cite exact admitted source references; label confidence as observed
or inferred. Never infer sensitive traits or claim consent, CRM mutation,
enrollment, or send completion.
