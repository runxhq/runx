# Dashboard Reference

Use this reference when the operator objective is about manager dashboard shape,
state projection, action catalogs, or agent-controlled operations.

## Shape

The dashboard is a projection plus an action catalog:

```text
projections:
  health
  money
  communications
  providers
  receipts
  access
  deploy

actions:
  read-only checks
  proposal builders
  approval-gated mutations
  post-action verification
```

Do not implement dashboard buttons as bespoke backend methods. Each button must
map to a governed lane the agent can also route to.

## Agent Contract

The agent may:

- summarize projected state;
- rank risks;
- prepare an action proposal;
- ask for missing inputs;
- prepare approval copy;
- route to a governed lane;
- verify the receipt after action.

The agent must not:

- mutate provider state from dashboard prose;
- mark action success without a receipt or provider readback;
- bypass human approval because the UI already shows a button;
- invent state that is absent from the projection.

## Dashboard Cards

Each card should expose these fields:

```yaml
card:
  id: string
  area: health | money | communications | providers | receipts | access | deploy
  status: ok | needs_attention | blocked | unknown
  headline: string
  evidence_refs: [string]
  primary_action:
    action_id: string
    lane: string
    consequence: read_only | draft | live_mutation | money_movement | public_send | deploy
    approval_required: boolean
```

## Action Catalog

Actions should be small named lanes:

- `ledger.query`
- `receipt.verify`
- `payment.spend`
- `payment.refund`
- `payment.payout`
- `send.plan`
- `send.approve`
- `provider.health_check`
- `provider.sync`
- `deploy.smoke`
- `access.audit`

Products may alias these for UX, but the ops desk packet should still
include the canonical lane.

## Stop Conditions

- Missing projection: return `needs_input` or `unknown`, not `ok`.
- Multiple unrelated criticals: rank them; do not flatten into a long list.
- Unknown action consequence: require human review before preparing execution.
