export function simulatePayment(inputs) {
  const signal = requiredRecord(inputs.payment_signal, "payment_signal");
  requiredRecord(inputs.parent_payment_authority, "parent_payment_authority");
  const idempotencyKey = requiredString(inputs.idempotency_seed, "idempotency_seed");
  const amount = positiveInteger(signal.amount_minor, "payment_signal.amount_minor");
  const currency = currencyCode(signal.currency, "payment_signal.currency");
  return {
    payment_result: {
      status: "simulated",
      rail: "mock",
      amount_minor: amount,
      currency,
      counterparty: requiredString(signal.counterparty, "payment_signal.counterparty"),
      idempotency_key: idempotencyKey,
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
