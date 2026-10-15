const ADDRESS = /^0x[0-9a-fA-F]{40}$/u;
const HTTPS = /^https:\/\/[^\s/?#]+[^\s]*$/u;

/**
 * One invoice, checked against the authority that may pay it, as the exact
 * payment `pay` makes: its payee's x402 endpoint, its amount, its payee
 * address. Today `pay` pays in USD only, so an invoice in another currency is
 * refused rather than converted.
 */
export function admitInvoice(inputs) {
  const invoiceRef = text(inputs.invoice_ref, "invoice_ref", 256);
  const amountMinor = inputs.amount_minor;
  if (!Number.isSafeInteger(amountMinor) || amountMinor < 1) {
    throw new Error("Invoice amount must be a positive whole number of minor units.");
  }
  if (inputs.currency !== "USD") {
    throw new Error(`Invoice currency ${JSON.stringify(inputs.currency)} has no payment method; pay settles USD.`);
  }
  const payee = record(inputs.payee, "payee");
  const paymentUrl = text(payee.payment_url, "payee.payment_url", 2048);
  if (!HTTPS.test(paymentUrl)) throw new Error("payee.payment_url must be the payee's exact HTTPS payment endpoint.");
  const payTo = text(payee.pay_to, "payee.pay_to", 42);
  if (!ADDRESS.test(payTo)) throw new Error("payee.pay_to must be an EVM address.");

  const authority = record(inputs.parent_payment_authority, "parent_payment_authority");
  if (authority.currency !== "USD") throw new Error("The payment authority must be in USD.");
  if (!Number.isSafeInteger(authority.max_amount_minor) || authority.max_amount_minor < amountMinor) {
    throw new Error("The invoice amount exceeds the payment authority.");
  }
  if (authority.counterparty !== paymentUrl) {
    throw new Error("The payment authority does not name this invoice's payee.");
  }

  return {
    payment_signal: Object.freeze({
      resource: Object.freeze({ url: paymentUrl, method: "POST", body: Object.freeze({ invoice_ref: invoiceRef }) }),
      amount_minor: amountMinor,
      pay_to: payTo,
    }),
  };
}

function text(value, field, maxLength) {
  if (typeof value !== "string" || !value.trim() || value !== value.trim() || value.length > maxLength) {
    throw new Error(`${field} is invalid.`);
  }
  return value;
}

function record(value, label) {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new Error(`${label} must be an object.`);
  }
  return value;
}
