---
name: contact-consent-review
description: Decide and record conservative contact-consent transitions from bounded engagement, bounce, and consent evidence before any send.
---

# Contact Consent Review

Use this skill after collecting current contact evidence and before preparing a
campaign. It decides whether a contact remains eligible, needs re-permission,
must be suppressed, or needs human review. When policy justifies a state
transition, it records that decision in the operator's consent ledger with
optimistic concurrency and verifies the exact event through a second read.

This skill never sends a message and never mutates a mailing provider. The operator-selected mail delivery capability must read the current ledger
decision and provider unsubscribe/bounce state again at delivery time. This
skill does not wire that check into a sender; a delivery lane that cannot
enforce it must remain unused. The
ledger records the operator's policy decision; it does not replace provider
unsubscribe or bounce data.

## Operating model

The caller supplies one bounded engagement observation, the current consent
observation, and the policy threshold that applies to the contact. The evidence
must be marked `read` and versioned to the same ledger head that the method
reads through the store's `events.read`, in this skill's own `consent` stream,
which only it writes. The read returns only the latest event, its ledger
version and chain digest. Once a transition exists, its
recorded state must agree with the caller observation; matching versions alone
are insufficient. A conflicting or unrecognized ledger head requires review.
Missing, stale, ambiguous, or conflicting
evidence produces `human_review` without a write.

The deterministic decision order is conservative:

1. An unsubscribe marker stops automation.
2. An existing suppression is preserved; age alone can never relax it.
3. Verified hard-bounce evidence suppresses when policy permits that automatic
   action, otherwise it stops for review.
4. A subscribed contact whose last engagement exceeds the decay threshold is
   moved to `re_permission`.
5. A clean, recent subscribed contact returns `verify` without writing.

Only `re_permission` and `suppress` can create transitions. A transition
decides whether a person can be sent to, so its append waits on a person's
approval of the exact event (`consent_transition`), whatever the run's
`effects`. The append uses the observed ledger version and the caller's stable
idempotency key. Readback must
match the append status, the contact's key, new version, event reference, event
digest, and chain digest before the result reports a recorded transition.
No-write decisions also perform a second read and require the key to remain
unchanged.

## Authority and finality

The store reader requires `store.read` and, only for an admitted transition,
the append requires `store.append`. These scopes authorize a bounded consent-ledger operation;
they do not authorize delivery or changes to a CRM or mailing provider. The
store instance owns storage and credentials.

A sealed result proves the ledger reads and any exact recorded transition. It
does not prove provider state, re-subscription, campaign approval, or delivery.
Every result therefore keeps `downstream_send.status` at `not_run` and leaves
`downstream_send.skill` null: this review neither selects nor executes a sender.

## Decisions and recovery

- `verify`: current evidence supports no transition. Nothing is appended.
- `re_permission`: engagement has decayed past policy and a subscribed contact
  must regain permission before sending. The transition is recorded once.
- `suppress`: hard-bounce evidence requires suppression, or an existing
  suppression is preserved. A write occurs only when state actually changes.
- `human_review`: evidence or policy cannot safely support automation. Nothing
  is appended.

If the ledger version changes before the decision, run again with a fresh read
and fresh evidence. Reuse an idempotency key only for the identical transition.
The method deliberately makes no claim that a post-commit retry can recover
from the old expected version; inspect the receipt and current ledger before
deciding whether another operation is needed.

## Inputs and result

Provide the contact's key in the consent ledger, the observed version, stable transition key, bounded engagement counts and recency,
bounce policy, and current consent evidence. Counts must be non-negative and
the policy threshold must be between 1 and 3,650 days.

The result preserves the decision and reason, the observed consent state, the
readback-bound transition identity when one was written, a human escalation
only when review is required, and the untouched downstream send boundary.

## Agent rules

- Never invent engagement, bounce, consent, or version evidence.
- Never relax suppression from contact age alone.
- Never re-permission an unsubscribed contact automatically.
- Never describe an internal ledger append as a provider update.
- Never dispatch, send, or claim delivery from this result.
