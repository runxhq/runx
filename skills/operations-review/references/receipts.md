# Receipts Reference

Use this reference for verification, ledger questions, public proof, receipt
publication, and after-action review.

## Rule

Receipts are the source of truth for what runx did. Projections and dashboards
are derived read models. If they disagree, the receipt/effect chain wins.

## Common Lanes

- `ledger.query`: cross-run audit question.
- `receipt.verify`: verify one receipt or receipt tree.
- `receipt.publish`: publish a receipt for stranger verification.
- `history.analyze`: graded run-history report.
- `access.audit`: least-privilege review from receipt usage.
- `post_action.verify`: confirm the expected receipt/effect/readback exists.

## Operator Packet Requirements

For proof-heavy actions include:

- expected receipt schema or class;
- receipt id or digest when known;
- effect ref when the product projects state;
- trust root or verifier path for public proof;
- failure mode when verification is absent.

## Stop Conditions

- Claiming an action completed with no receipt, effect, provider readback, or
  explicit manual exception.
- Using a receipt summary when the requested decision needs raw receipt
  verification.
- Publishing secret-bearing material.
- Treating absence of receipts as proof that nothing happened.
