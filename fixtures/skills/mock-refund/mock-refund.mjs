export function simulateRefund(inputs) {
  requiredRecord(inputs.parent_payment_authority, "parent_payment_authority");
  const request = requiredRecord(inputs.refund_request, "refund_request");
  return {
    refund_result: {
      status: "simulated",
      settlement_family: "mock",
      original_receipt_ref: requiredString(inputs.original_receipt_ref, "original_receipt_ref"),
      amount_minor: positiveInteger(request.amount_minor, "refund_request.amount_minor"),
      currency: currencyCode(request.currency, "refund_request.currency"),
      idempotency_key: requiredString(inputs.idempotency_key, "idempotency_key"),
      money_moved: false,
    },
  };
}

function requiredRecord(value, field) {
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error(`${field} must be an object`);
  return value;
}
function requiredString(value, field) {
  if (typeof value !== "string" || !value.trim()) throw new Error(`${field} must be a non-empty string`);
  return value;
}
function positiveInteger(value, field) {
  if (!Number.isSafeInteger(value) || value <= 0) throw new Error(`${field} must be a positive safe integer`);
  return value;
}
function currencyCode(value, field) {
  const currency = requiredString(value, field);
  if (!/^[A-Z]{3}$/u.test(currency)) throw new Error(`${field} must be a three-letter uppercase code`);
  return currency;
}
