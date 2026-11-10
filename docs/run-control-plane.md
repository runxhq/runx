# Run Control Plane

runx issue automation is governed by receipts, not by a tracker-style
queue object. Source adapters admit signals, decisions open or decline run
nodes, contained acts perform revision, reply, review, observation, or
verification acts, and each run seals to a receipt.

## Ownership

OSS owns:

- canonical signal, decision, act, run, artifact, redaction, verification,
  and receipt contracts
- local skill inputs and outputs
- source-thread and outbox packets
- receipts and scafld-backed issue-to-PR execution

Cloud owns:

- hosted run storage and receipt indexing
- approval state for runs explicitly submitted to the hosted service, not the
  default provider-neutral operator inbox
- credential-bound source transport for explicitly hosted runs, not source
  filtering or operator workflow policy
- org routing and operational APIs

Consuming repos own:

- Slack, Sentry, GitHub, or file source filters
- target repo policy
- owner suggestion rules
- source-thread notification policy

## Lifecycle

The durable lifecycle is explicit:

1. A signal records the admitted source event.
2. A decision opens, defers, declines, or routes a run.
3. The run admits authority and records idempotency.
4. Acts inside the run record their intent, form and reason.
5. Child runs carry attenuated authority.
6. The run seals normally or abnormally to a receipt.

Every terminal path produces a receipt. Failed, killed, timed out, blocked, and
declined paths are not missing evidence; they are abnormal seals with reason
codes, criterion state, and hash commitments.

Test cases execute in independent scratch workspaces and receipt stores.
When a case needs prior receipt evidence, `setup.receipts` may name only
normalized `.json` files contained by the owning skill package. The runtime
verifies those receipts before seeding that case and never treats fixture setup
as provider readback. Newly produced receipts are verified again before they
are copied to the requested durable output store.

## Evidence

Evidence is carried by references, artifacts, verification checks, redaction
records, and receipt commitments. There is no separate evidence bundle
contract. Adapters may hydrate richer provider context before a decision opens
a mutation run, but the governed boundary is the receipt.

## Merge Authority

The issue-to-PR product lane may create PRs and post source-thread updates, but
human merge authority stays outside the default runx mutation authority. A
hosted operator that wants auto-merge needs an explicit policy surface with
separate repo allowlists, branch protections, checks, audit events, and
rollback contracts.
