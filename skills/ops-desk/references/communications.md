# Communications Reference

Use this reference for email, SMS, support replies, social posts, provider
comments, campaign sends, and notification actions.

## Rule

Live communication requires a principal, audience, content reference, consent or
policy basis, provider readiness, and approval. Drafting is not sending.

## Common Lanes

- `send.plan`: route through `message-send`; no live delivery.
- `send.test`: provider test or preview; usually no live approval.
- `send.approve`: human approval for live delivery.
- `send.schedule`: live/public delivery; approval required.
- `send.transactional`: may be live; approval depends on operator policy.
- `provider.reply`: source-thread reply; approval depends on public/customer
  visibility.

## Required Evidence

- principal represented;
- audience or recipient;
- content digest or stable draft ref;
- consent basis or policy justification;
- unsubscribe/suppression/preflight status when applicable;
- provider account readiness;
- approval ref for live sends.

## Stop Conditions

- No principal.
- No audience.
- Mutable content without digest/stable ref.
- All-contacts or broad audience without explicit reconfirmation.
- Missing consent, unsubscribe, suppression, preflight, or verified sender.
- Request to send from a provider preview or draft without approval.

Route provider-specific execution to the selected provider's connector; keep
the authority model in `message-send`. Branded providers belong in their own skills
and connectors, not in this ops desk spine.
