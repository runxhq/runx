/** Bind caller-supplied invoice terms; this does not authenticate the invoice. */
export function admitInvoice(input) {
  const reference = input.invoice_id;
  if (typeof reference !== "string" || !reference.trim() || reference !== reference.trim() || reference.length > 256) {
    throw new Error("Invalid invoice reference.");
  }
  if (!Number.isSafeInteger(input.amount_minor) || input.amount_minor < 1) {
    throw new Error("Invoice amount must be positive whole minor units.");
  }
  if (typeof input.currency !== "string" || !/^[A-Z]{3}$/.test(input.currency)) {
    throw new Error("Invoice currency must be an explicit ISO code.");
  }
  const request = record(input.request, "request");
  const payee = record(input.payee, "payee");
  const link = input.connector === "link";
  if (!link && input.connector !== "wallet/evm") throw new Error("Unsupported buyer connector.");
  const target = link ? request.resource_url : record(request.resource, "request.resource").url;
  if (typeof payee.payment_url !== "string" || !/^https:\/\/[^\s/?#@]+[^\s]*$/.test(payee.payment_url) || target !== payee.payment_url) {
    throw new Error("The payment target must be the invoice payee exact HTTPS endpoint.");
  }
  if (request.amount_minor !== input.amount_minor) throw new Error("Payment amount differs from the invoice.");
  if ((link ? request.currency : "USD") !== input.currency) throw new Error("Payment currency differs from the invoice; no FX is implied.");
  return { payment_request: request };
}

function record(value, label) {
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error(`${label} must be an object.`);
  return value;
}
