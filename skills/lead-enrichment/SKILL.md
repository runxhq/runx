---
name: lead-enrichment
description: Normalize factual person and company evidence for one confidently matched lead, preserving provenance, freshness, missing fields and conflicts.
---

# Lead Enrichment

Build factual context for one person or company. Enrichment answers what is
known about the subject. `lead-qualify` decides whether those facts meet an ICP;
`cold-outreach` prepares copy only after contact eligibility is supplied.

The default `enrich` runner reuses supplied structured evidence. It makes no
provider request and spends no enrichment credits. An upstream declared
connector operation can supply evidence; Apollo is the planned first lookup
provider, whose credential presentation remains a native connector dependency.
Do not claim that running this skill performs an Apollo lookup.

## Procedure

1. Bind one subject reference and the professional fields actually needed.
2. Supply source references, native or upstream digests, observation times,
   explicit subject matches, and factual field/value pairs. A caller-supplied
   digest is provenance, not independent provider verification.
3. Reuse only fresh sources confidently matched to that exact subject. A weak
   name match, stale observation or mismatched identity does not fill a field.
4. Preserve conflicting values with their separate sources. Do not silently
   choose the latest value or invent a merged person from several matches.
5. Return `enriched`, `partial` or `needs_more_evidence`, supported facts,
   missing fields, conflicts and the admitted source index.

The runner performs deterministic normalization rather than an extra model
turn. Evidence acquisition and entity matching belong to the selected provider;
commercial fit belongs to qualification. It does not infer buying intent,
budget, consent, sensitive traits or private personal details.

## Composition

Pass `lead_enrichment_packet` into `lead-qualify#qualify` together with the
actual ICP criteria. Preserve the native artifact digest. For another lookup,
request only missing professional fields with a bounded credit allowance and
the same subject identity. Never enable personal-email/phone reveal or a
waterfall merely to make an incomplete packet look complete.

The skill neither updates CRM records nor authorizes messages. The calling
application owns refresh schedules and continuing work; connectors own exact
provider requests, credential custody and billing evidence.
