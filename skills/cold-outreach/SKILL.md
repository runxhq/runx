---
name: cold-outreach
description: Prepare one relevant, evidence-bound cold email for an exact prospect using scoped brand and taste context; hold unsafe or unsupported contact and leave delivery to an approved mail capability.
---

# Cold outreach

Prepare one useful first contact, not a campaign or an email transport. Reuse
supplied prospect research, approved offer facts, `brand-voice` and `taste-profile`
context. The calling application selects that workspace context; this skill never
searches global memory or a private credential database.

`prepare` admits the evidence and writing context through `ghostwrite#admit` and
checks the supplied contact-policy disposition and suppression state. Only then
does it take one writing turn of its own, which `ghostwrite#verify` binds to the
admitted evidence before review. A matched
lead, public address or good commercial fit does not establish contact permission.
Unknown eligibility, suppression, missing policy evidence or an unsupported draft
produces a hold. No approval, delivery or provider acknowledgement is implied.

## Composes

<!-- Generated from the native execution closure; run pnpm core-skills:composes:generate. -->

- `ghostwrite#admit`
- `ghostwrite#verify`

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
supplied, bind `prospect.ref` to its exact subject; a hold or mismatched subject
stops drafting even when other contact inputs are eligible. `research` supplies the
validated evidence packet. `audience` names the reader segment the prospect
belongs to. Brand and taste packets apply only when their declared scope names
that audience and the email channel, and they keep their structured profiles,
evidence bindings and stop conditions through ghostwrite's admission.

The result includes an `outreach_review` with the exact sender, recipient and
draft status. A ready result includes the exact subject/body and the native digest
of its validated ghostwrite source packet; a hold exposes no usable email payload.
Route the ready payload through a compatible mail capability. Delivery admission must bind sender,
recipient and complete content to native approval and provider execution.

The calling application retains the draft and outcomes. Follow-ups require a fresh
conversation and suppression read, add something useful, and stay within the
user's configured limit. Reply ends the cold sequence; opt-out, complaint or
invalid address stops it. Out-of-office is not buying interest. This skill owns
no timer, queue or send ledger. Each outward message still follows the
configured approval policy.

## Agent task contract

### `cold-outreach-draft`

Write one first-contact email from `sender` to `prospect` that serves `purpose`,
following the outreach judgment above. `offer` is the value to propose; support
it only as far as the admitted `evidence_context` allows. Apply each admitted
writing context's profile, applicability and stop conditions.

Return `content_draft` with `decision` set to `ready`, or to
`needs_more_evidence` when no worthwhile angle is supported. `content_brief`
repeats the evidence context's `objective`, `audience` and `channel` exactly.
`draft.title` is the subject and `draft.body` the plain-text email, with
`required_footer` unchanged. `draft.claim_refs` cites admitted source digests
for every material claim. Add a review checklist and distribution notes, and
list in `context_binding.applied` each admitted context's `packet_digest` with
only the `applied_rules` you used. Never send, schedule or claim delivery.
