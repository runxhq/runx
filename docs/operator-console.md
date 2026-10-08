# Operator Console

The operator console is the manager surface for a project, workspace, product,
or account. It is not a second control plane. It is a projection plus an action
catalog over the same governed lanes an agent can use.

See [Operator Skills](./operator-skills.md) for the reusable skill boundary:
operator skills reason, gate, route, and verify; the CLI, hosted API, workflow,
or provider tool remains the execution interface.

## Shape

```text
state projections -> ops-desk -> governed action lane -> receipt -> projection
```

The dashboard shows state. The agent explains and routes action. The runtime
enforces authority, approvals, receipts, and provider boundaries.

## Responsibilities

The console may show:

- health and deploy status;
- payment targets, quotes, settlements, payouts, refunds, and stuck effects;
- communication drafts, approvals, sends, and provider readiness;
- receipt publication and verification status;
- provider sync/webhook/credential health;
- access and least-privilege review state.

The console must not add bespoke mutation routes for convenience. A dashboard
button maps to a governed lane such as `send-as`, `ledger`, `refund`,
`provider.send`, `least-privilege`, or a product skill.
If the lane ultimately runs a CLI command, connector operation, or repository
workflow, the dashboard and agent both reference that existing interface. They
do not duplicate its logic in the UI or in skill prose.

## Agent Contract

Use `ops-desk` when an agent is asked to manage a project, workspace, product,
or account. It reads the same projection the UI shows and emits
`runx.ops_desk.packet.v1`:

- findings grounded in evidence;
- proposed governed lanes;
- approval prompts for consequential actions;
- blockers and missing inputs;
- receipt/effect/readback expectations.

The packet is a plan/proposal surface. Consequential work still executes through
the named lane and seals its own receipt. `ops-desk` may name the command,
workflow, hosted endpoint, or skill runner to use, but it does not implement
those operations itself.

## Gates

- Read-only status and audit: no approval.
- Drafts, dry-runs, previews, and reports: no live-action approval unless they
  expose private data or widen authority.
- Live sends, payouts, refunds, public provider mutations, target changes,
  credential changes, deploys, and destructive actions: explicit approval.
- Post-action success: receipt/effect/readback required.

## Product Policy

Product-specific operator skills should provide product policy and vocabulary.
They should not fork the dashboard model or copy private product behavior into
OSS skills. The core loop stays:

```text
snapshot -> findings -> proposals -> approval -> governed lane -> receipt
```

Project profiles may describe product topology, existing workflows, and
verification URLs. They are not alternate execution engines. If a profile needs
a behavior the CLI or hosted API cannot perform cleanly, fix that underlying
interface instead of teaching an operator skill a private workaround.
