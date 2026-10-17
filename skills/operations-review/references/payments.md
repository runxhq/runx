# Payments Reference

Use this reference for funding, payouts, refunds, target changes, chargebacks,
settlement health, and payment reconciliation.

## Rule

Money state changes only after rail proof becomes a receipt/effect. UI state,
provider optimism, local API success, or agent narration is not settlement.

The ops desk spine is rail-neutral. It selects a public payment contract and
stops at the right approval gate. Real payment execution always requires Runx
Hosted. Rail-specific funding, wallet, webhook, dispute, settlement, recovery,
and ledger details stay in the private hosted implementation.

## Common Lanes

- `payment.charge`: hosted seller-side settlement; approval required.
- `payment.spend`: hosted buyer-side settlement; approval required.
- `payment.invoice.settle`: hosted invoice settlement; approval required.
- `payment.x402`: hosted x402 settlement; approval required.
- `payment.refund`: money movement; approval required.
- matching `.read` operations: provider readback; no mutation approval.
- `payment.dispute_response`: customer/provider communication; approval
  depends on whether it submits externally.

## Rail Adapter Contract

A payment rail adapter must make these fields explicit before settlement:

- operation: charge, spend, invoice settlement, x402, refund, dispute, or readback;
- amount and currency;
- payer, payee, counterparty, or refund target;
- network, rail, account, asset, or processor path;
- cap, expiry, and idempotency key;
- approval or payer-signature requirement;
- settlement proof shape;
- readback source after settlement.

Do not infer cross-rail compatibility. A balance, address, token, processor
account, webhook, or credential on one rail does not imply readiness on another
rail. If a product supports multiple rails, each rail has its own adapter status,
target configuration, proof, and reconciliation readback.

## Operator Packet Requirements

For each payment proposal include:

- payer/payee refs, redacted when necessary;
- amount and currency;
- rail adapter and network/account path;
- quote, reservation, approval, or settlement refs;
- expiry or idempotency key;
- approval requirement;
- expected receipt/effect;
- reconciliation readback.

## Stop Conditions

- Missing amount, payee, rail adapter, quote, approval, signature, or
  idempotency key.
- Requested manual funded/paid/refunded marking without receipt-backed proof.
- Network, asset, account, or rail mismatch between quote and payer funds.
- Target update requested without explicit operator approval.
- Refund or payout amount not tied to the original settlement or policy.
- Payout requested for a claim, invoice, or obligation that has not reached the
  product's payable state.
- Rail-specific funding or recovery requested without loading the rail adapter
  or product runbook that owns that procedure.
