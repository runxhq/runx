---
name: helpdesk
description: Classify a bounded support request, choose the safe next path, and draft a customer-ready reply only when a human-gated send is appropriate.
---

# Helpdesk

## What this skill does

Classify one bounded support request, choose the safest next path, and draft a
customer reply only when the supplied context supports one. The runner emits a
support triage packet with classification, severity, confidence, evidence,
draft email, and a send gate.

This skill never sends email, posts to Slack, opens issues, mutates accounts, or
touches billing. It prepares the decision packet that a separate governed send
skill can review, approve, and deliver with its own authority grant and receipt.

## When to use this skill

Use this skill when an agent has a single support request and needs a safe first
decision: answer, ask for more information, route to engineering, route to
billing, route to account review, or route to abuse review.

It is useful where support triage fans out from inbox
intake, then hands off only safe drafts to `send-as` or another provider-backed
send lane. It also works for dry-run review of support queues because it has no
external side effects.

## When not to use this skill

Do not use this skill as a helpdesk transport, account access workflow,
billing authority, customer identity verifier, or automatic sender. Do not use
it to answer from private account state unless that state has already been
summarized into an approved `support_request` packet.

If the request asks for account recovery, billing changes, abuse handling, or
anything that needs private records, the skill must not draft a definitive
answer. It should return a review route and let a stronger authority gate handle
the consequence.

## Procedure

1. Require `support_request` to contain at least `subject` or `body`.
2. Normalize the request text and classify it as `how_to`, `billing`,
   `account_access`, `bug`, `abuse`, or `unknown`.
3. Estimate severity and confidence from matched signals.
4. Choose the recommended path.
5. Draft a customer reply only for safe `how_to` requests.
6. Record missing context and matched signals.
7. Emit `send_gate.status = requires_human_approval` for every result.
8. Let downstream send or operator lanes decide whether a draft may be sent.

## Edge cases and stop conditions

Return `needs_input` when `support_request` is missing or lacks both subject and
body. Return `needs_more_evidence` when the request is too vague to route. Mark
private-state paths as gated review, not as ready-to-send drafts. If a caller
asks the skill to send, mutate an account, bypass an approval gate, or provide
credential recovery instructions, return `refused`.

The authority scope is classification and draft preparation only. The proof
surface is the sealed receipt containing the request summary, matched signals,
recommended path, draft proposal if any, and send gate. Any live customer send
requires a separate `send-as` receipt.

## Output schema

The runner emits `runx.support.triage_reply.v1`:

```json
{
  "classification": "how_to | billing | account_access | bug | abuse | unknown",
  "severity": "low | medium | high | critical",
  "confidence": 0.78,
  "recommended_path": "reply_draft | request_info | engineering_intake | billing_review | account_review | abuse_review | manual_review",
  "evidence": {
    "source": "fixture:safe-how-to",
    "source_summary": "How do I verify my sending domain?",
    "matched_signals": ["verify", "dns", "domain"],
    "missing_context": [],
    "taxonomy_coverage": ["how_to", "billing", "account_access", "bug", "abuse", "unknown"],
    "private_data_required": false,
    "send_side_effects": "none"
  },
  "draft_email": {
    "proposed": true,
    "subject": "Re: How do I verify my sending domain?",
    "body": "..."
  },
  "send_gate": {
    "status": "requires_human_approval",
    "action": "send_customer_email",
    "rationale": "..."
  }
}
```

## Worked example

```bash
runx skill "$PWD" \
  --runner triage \
  --input-json support_request='{
    "customer_name": "Mira",
    "customer_email": "mira@example.test",
    "subject": "How do I verify my sending domain?",
    "body": "I added the DNS records. What should I check next?",
    "source": "fixture:safe-how-to"
  }' \
  --input-json policy='{
    "product_name": "ExampleDesk",
    "support_signature": "ExampleDesk Support"
  }' \
  --json
```

Expected result: `classification = how_to`, `recommended_path = reply_draft`,
`draft_email.proposed = true`, and `send_gate.status =
requires_human_approval`. The run does not send the email.

## Inputs

- `support_request`: object with `subject`, `body`, optional `customer_name`,
  optional `customer_email`, optional `source`, and optional `refs`.
- `policy`: optional object with `product_name`, `support_signature`,
  `safe_reply_topics`, and `escalation_contacts`.

## Outputs

- `classification`: request class.
- `severity`: operational severity.
- `confidence`: number from 0 to 1.
- `recommended_path`: next safe handling path.
- `evidence`: matched signals, missing context, source summary, and taxonomy
  coverage.
- `draft_email`: proposed customer reply or blocker reason.
- `send_gate`: always requires human approval before delivery.
