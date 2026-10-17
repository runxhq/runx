# Providers Reference

Use this reference for provider health, credentials, webhooks, deploys, syncs,
and incident triage.

## Rule

Provider state is observed through governed checks and redacted refs. Secrets
are never copied into ops desk packets.

## Common Lanes

- `provider.health_check`: read-only.
- `provider.sync`: may be read-only or mutation depending on provider.
- `provider.webhook_check`: read-only unless changing configuration.
- `provider.credential_status`: read-only redacted status.
- `provider.credential_rotate`: credential mutation; approval required.
- `deploy.smoke`: read-only verification after deploy.
- `deploy.rollout`: deploy mutation; approval required.
- `deploy.rollback`: deploy mutation; approval required.

## Required Evidence

- provider name and account/workspace/product ref;
- status timestamp;
- credential ref, never raw credential;
- webhook endpoint and latest delivery status when relevant;
- last successful receipt/effect/sync ref;
- known degraded dependency.

## Stop Conditions

- Raw token, API key, wallet private key, seed, webhook secret, or customer data
  appears in provider context.
- Provider mutation requested without an owner or approval.
- Deploy requested while health checks are unknown and no break-glass approval is
  present.
- Sync action would mutate public/customer-visible state but is framed as
  read-only.
