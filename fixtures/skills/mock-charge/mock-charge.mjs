export function simulateCharge(inputs) {
  requiredRecord(inputs.mcp_tool_call, "mcp_tool_call");
  const policy = requiredRecord(inputs.provider_policy, "provider_policy");
  return {
    charge_result: {
      status: "simulated",
      settlement_family: "mock",
      amount_minor: positiveInteger(policy.price_minor, "provider_policy.price_minor"),
      currency: currencyCode(policy.currency, "provider_policy.currency"),
      counterparty: requiredString(policy.counterparty, "provider_policy.counterparty"),
      idempotency_key: requiredString(inputs.idempotency_seed, "idempotency_seed"),
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
