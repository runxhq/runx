---
name: cold-outreach
description: Prepare one relevant, evidence-bound cold email for an exact prospect using scoped brand and taste context, or first contact for each lead of a batch; hold unsafe or unsupported contact and leave delivery to an approved mail capability.
---

# Cold outreach

Prepare useful first contact, for one prospect or for each lead of a batch, never
a campaign sequence or an email transport. Reuse
supplied prospect research, approved offer facts, `brand-voice` and `taste-profile`
context. The calling application selects that workspace context; this skill never
searches global memory or a private credential database.

`prepare` admits the evidence and writing context through `ghostwrite#admit` and
checks the supplied contact-policy status and suppression state. Only then
does it take one writing turn of its own, which `ghostwrite#verify` binds to the
admitted evidence before review. A matched
lead, public address or good commercial fit does not establish contact permission.
Unknown eligibility, suppression, missing policy evidence or an unsupported draft
produces a hold. No approval, delivery or provider acknowledgement is implied.

Name memory levels in `memory`, and `prepare` recalls what memory holds about
the recipient before it drafts, through `runx/memory#search` by the recipient's
address, so a contact memory already knows is drafted as known. The run reads
only the memory streams its authority names, and each entry keeps the labels
memory gave it. With no levels named, nothing is recalled.

`batch` prepares first contact for each lead of one campaign. Each lead's
supplied sources go through `lead-enrichment#enrich`, the lead is qualified
against the criteria by `lead-qualify#qualify`, with its contact policy as its
contact eligibility, and `prepare` drafts from that qualification. Four leads
run side by side, each lane taking the next lead when its last is done, so a
batch holds at most four leads at once however many it has, and every lead's
outcome is collected in the order given: a hold or a failure is that lead's
alone, and a failure names the stage it stopped at. Reading the lead is its
first stage, and a lead past the 4 MiB a script reads is rejected there,
named by its place in the outcomes since its id was never read. Each outcome carries
the `idempotency_key` its send is made under, made from the campaign and the
recipient and never the draft, so a revised batch never sends a lead twice. The
batch runs under `effects: none` and sends nothing: a mail capability sends each
approved draft under its key. Memory levels it's given go to each lead's
`prepare`.

A lead's qualification is reused for up to a week (604,800 seconds). A
revised batch, say with a new offer, then keeps each lead's fit and drafts
again. Qualifying reads nothing of its own: it judges only the enrichment
packet, criteria and contact eligibility it's given, and reuse matches those
exactly. A changed fact, criterion or policy therefore qualifies the lead
again at once. The age bounds only how long the judgment itself stands.
A week covers the revisions of one campaign, and a qualification older than
that is made again.

## Composes

<!-- Generated from the native closure; run pnpm core-skills:composes:generate. -->

- `ghostwrite#admit`
- `ghostwrite#verify`
- `lead-enrichment#enrich`
- `lead-qualify#qualify`
- `memory#search`

## Outreach judgment

Find a specific, public or disclosure-approved reason this offer relates to the
recipient's work. Prefer an observed change, relevant problem or request to a
generic compliment. If the same rationale fits every company, narrow it or hold
for better evidence. An uncertain need is a question or hypothesis, not a claim
about their internal problems, budget, tools or authority.

Offer a concrete useful result or small next step. Use current supported proof;
voice and taste cannot approve factual claims. Start around 60–120 words in short
paragraphs, excluding the supplied identity and opt-out footer. Adapt that length
to the offer. Lead with relevance, explain value plainly and make one simple ask.

Use a truthful subject and plain text. No fake `Re:` or `Fwd:`, invented familiarity,
metrics, urgency, exaggerated praise, tracking tricks or private personal details.
Keep internal citations and reasoning in the review artifact. The exact footer
must appear unchanged in the body. If the evidence does not support a worthwhile
angle, return `needs_more_evidence` instead of producing generic sales copy.

## Composition and result

Use `lead-enrichment` for missing facts and `lead-qualify` for fit;
neither is mandatory when adequate evidence is supplied. If `qualification` is
supplied, bind `prospect.id` to its exact subject; a hold or mismatched subject
stops drafting even when other contact inputs are eligible. `research` supplies a validated evidence packet when research was needed.
For known prospect/offer facts, supply `facts` instead; `ghostwrite#admit` binds
that source directly without a research run. Never supply both source forms.
These facts remain caller-supplied assertions, not independently verified
research or contact-policy evidence. `audience` names the reader segment the prospect
belongs to. Brand and taste packets apply only when their declared scope names
that audience and the email channel, and they keep their structured profiles,
evidence bindings and stop conditions through ghostwrite's admission.

The result includes an `outreach_review`, a `runx/cold-outreach.review.v1`
packet, with the exact sender, recipient and draft status. A ready result
includes the exact subject/body and the native digest
of its validated ghostwrite source packet; a hold exposes no usable email payload.
Route the ready payload through a compatible mail capability. Delivery admission must bind sender,
recipient and complete content to native approval and provider execution.

The calling application retains the draft and outcomes. Follow-ups require a fresh
conversation and suppression read, add something useful, and stay within the
user's configured limit. Reply ends the cold sequence; opt-out, complaint or
invalid address stops it. Out-of-office is not buying interest. This skill owns
no timer, queue or send ledger. Each outward message still follows the
configured approval policy.

## Agent task: cold-outreach-draft

Write one first-contact email from `sender` to `prospect` that serves `purpose`,
following the outreach judgment above. `offer` is the value to propose; support
it only as far as the admitted `evidence_context` allows. Apply each admitted
writing context's profile, applicability and stop conditions.

Return `content_draft` with `decision` set to `ready`, or to
`needs_more_evidence` when no worthwhile angle is supported. `content_brief`
repeats the evidence context's `objective`, `audience` and `channel` exactly.
`draft.title` is the subject and `draft.body` the plain-text email, with
`required_footer` unchanged. `draft.claims` cites admitted source digests
for every material claim. Add a review checklist and distribution notes, and
list in `context_binding.applied` only packets from
`evidence_context.writing_contexts`. When that array is empty, return
`context_binding: {applied: []}`. Factual `source_digests` belong in claims;
they are never writing-context packet digests. Host writing profiles such as
`VOICE.md` guide style but are not admitted packets to cite here.
For each admitted writing context, copy its exact `packet_digest`, select only
the `rules` you used, and supply `scope_review: {decision, reason}` with decision
`applies`, `out_of_scope` or `uncertain`. Never invent a packet or rule to fill
the array. Never send, schedule or claim delivery.
