# Payments Reference

Review payment readiness without treating plans or dashboard labels as settlement.
Payment skills run through an operator-selected local, self-hosted or hosted
connector. Runx Hosted is optional; merchant credentials do not confer buyer
spending authority.

Choose the actual capability by role:

- `charge` collects a merchant payment.
- `pay` pays a known merchant resource through a held buyer method.
- `invoice-pay` binds already admitted invoice terms before `pay`.
- `refund` reverses an original collection.
- `dispute-respond` prepares or files the selected processor's dispute response.
- `procurement-policy-check` evaluates purchase policy; it does not reserve
  budget or grant payment authority.

Inspect the chosen method and connector before naming operations or arguments.
Do not invent dotted payment aliases, unsupported rails, a payout capability or
a private endpoint to fill a gap. Account custody and provider protocol stay
with the selected connector; the local operating host retains its state.

A useful proposal preserves exact amount/currency, principal/payee, account,
resource or original payment, original request identity, quote/mandate evidence,
required approval and the independent readback that establishes the result.
Never convert units, infer FX or select another account from dashboard prose.

A quote, reservation, consent or transaction acknowledgement is not confirmed
settlement. Keep uncertain outcomes explicit and continue the original effect
through its owner. x402 completion includes the paid resource result; Link
payment evidence does not by itself prove merchant delivery. Never mark paid,
refunded or fulfilled from a recommendation.

Use the existing execution owner's one native approval when required. This
review prepares the proposal and approval context, not another approval gate.
