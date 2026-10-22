/** Compare supplied purchase terms; native authority and approval remain authoritative. */
export function admitPayment(input) {
  const request = record(input.request, "request");
  const authority = record(input.parent_payment_authority, "parent_payment_authority");
  const link = input.connector === "link";
  if (!link && input.connector !== "wallet/evm") throw new Error("Unsupported buyer connector.");
  const resource = link ? request.resource_url : record(request.resource, "request.resource").url;
  const currency = link ? request.currency : "USD";
  const amount = request.amount_minor;
  if (typeof resource !== "string" || !/^https:\/\/[^\s/?#@]+[^\s]*$/.test(resource) || resource.length > 2048) {
    throw new Error("The resource must be an exact HTTPS URL.");
  }
  if (!Number.isSafeInteger(amount) || amount < 1) throw new Error("The payment amount must be positive whole minor units.");
  if (typeof currency !== "string" || !/^[A-Z]{3}$/.test(currency) || authority.currency !== currency) {
    throw new Error("Payment and parent currency differ. No FX conversion is implied.");
  }
  if (authority.counterparty !== resource) throw new Error("The parent does not authorize this exact counterparty.");
  if (!Number.isSafeInteger(authority.max_amount_minor) || authority.max_amount_minor < amount) {
    throw new Error("Payment exceeds the supplied parent maximum.");
  }
  if (typeof authority.term_id !== "string" || !authority.term_id.trim()) throw new Error("The payment mandate needs its original term reference.");
  if (link && request.action !== "pay") throw new Error("Wallet authorization is not merchant payment.");
  return { request, resource_url: resource };
}

function record(value, label) {
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error(`${label} must be an object.`);
  return value;
}
