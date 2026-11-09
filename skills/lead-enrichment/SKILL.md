---
name: lead-enrichment
description: Enrich an identified person or company through Apollo, or reuse supplied factual evidence, preserving match confidence, provenance, unavailable fields and conflicts.
---

# Lead Enrichment

Build factual context for one identified person or company. The default
lookup reads the selected Apollo account; enrich normalizes observations
already collected elsewhere with no provider request or extra credits.
Use lead-qualify for ICP fit and cold-outreach for an unsent email. Neither
an enriched record nor an email address establishes permission to contact.

## Direct lookup

Supply subject.id and subject.kind, a selector and requested_fields.
The subject ID is the caller's stable reference, not a fabricated Apollo ID.
After the provider response, the native clock records the observation instant.
Approval delays therefore cannot turn a caller's earlier timestamp into the
lookup time. This records the observation, not when Apollo last checked a field.

For a person, prefer Apollo id, email or linkedin_url. A full name requires
an employer domain or organization_name; first_name and last_name require
the same employer evidence. For a company use domain, website or a company
linkedin_url. A name alone is ambiguous. Native connector inspection declares
all allowed selector fields. Select only the professional fields needed for
the task: person name, work email, title and LinkedIn profile, or company name,
domain, industry, employee count, country and website.

Bind an appropriately owned Apollo key through the native connector. The
provider may consume credits even though its business data is only read.
Native admission accepts scoped use authority or asks for one exact
provider-usage approval. A dollar ceiling cannot cover an unknown USD price;
do not drop that ceiling or fabricate a zero quote to proceed. Authentication
health does not establish enrichment entitlement or a credit balance.

The graph performs one synchronous person or company lookup. It does not
request personal email, phone, waterfall enrichment or a callback receiver.
A parent can process a bounded set of leads through ordinary graph calls;
this skill does not schedule continuing work or impose a global batch limit.
Do not repeat an uncertain charged lookup. Preserve the paused run and native
custody, reconcile its evidence and use remaining authority for any new attempt.

## Matching and field availability

A successful HTTP response can contain no matched record. Person facts require
Apollo's high-confidence match; medium, low or absent confidence remains
ambiguous. Contradictory returned identifiers are an identity conflict, not a
second person to merge into the subject. Company facts require an exact domain,
website or company-profile match. Obtain a stronger selector when needed.

Only requested fields enter the factual packet. A locked placeholder such as
email_not_unlocked is never a contact address. Missing and malformed fields
remain unavailable. A work email is provider-supplied factual data, not an
independent deliverability, suppression or consent decision. Do not infer buying
intent, budget, sensitive traits or permission to contact from the record.

## Supplied evidence and composition

Use enrich explicitly when factual observations already exist. Supply sources
with source_id, source_digest, subject_id, observed_at, match_confidence and
field/value facts, plus requested_fields and as_of. The existing freshness
limit is configurable with max_age_days. Only fresh, confidently matched sources
for that exact subject contribute facts; wrong-entity and stale observations
remain blockers. Conflicting values keep their separate source references.
No extra model turn, lookup, CRM write or message occurs in this phase.

Both paths return the same lead_enrichment_packet for lead-qualify#qualify.
Its acquisition and provenance labels describe the supplied observations at
the normalization boundary. They are not an independent provider-verification
claim. Lookup additionally returns lookup_evidence with the actual native
readback URI, result digest, observed time, match outcome and cost uncertainty.
Keep that evidence and the run receipt with the factual packet; do not relabel
caller-supplied observations as provider proof. Existing composed graphs that
have sources must select enrich and must not fetch them again.

The packet distinguishes enriched, partial and needs_more_evidence. Present
supported facts and missing fields together; do not report a partial result as
complete. Qualification receives the same subject and source bindings with the
actual ICP. Contact-consent-review and the selected delivery capability own
contact eligibility and sending authority; this skill authorizes neither.

The caller owns refresh schedules, subsequent work and persistence. The
connector owns credential presentation and exact provider execution; Runx owns
admission, uncertain-attempt custody, source digests and receipts. Publication
is a separate registry operation, and offline fixtures are test proof only.
