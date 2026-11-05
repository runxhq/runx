/** Compare invoice terms with the selected public payment request; pay owns authority. */
export function admitInvoice(input) {
  const request = input.request;
  const link = input.connector === "link";
  const target = link ? request?.resource_url : request?.resource?.url;
  if (target !== input.payee.payment_url) throw new Error("Payment target differs from the invoice.");
  if (request?.amount_minor !== input.amount_minor) throw new Error("Payment amount differs from the invoice.");
  if ((link ? request?.currency : "USD") !== input.currency) throw new Error("Payment currency differs from the invoice; no FX is implied.");
  return {payment_request: request};
}

/**
 * The unit each held method counts an invoice currency in. x402 settles USD in
 * cents and Link's Stripe charge takes cents; any other pair has no bound unit
 * here, so a caller or document can never choose one.
 */
const MINOR_UNIT_DIGITS = {'wallet/evm': {USD: 2}, link: {USD: 2}};

/** Accept only a ready extraction, and bind its currency to the method's unit. */
export function invoiceTerms({extraction, connector}) {
  const {decision, value, citations, source_digest, missing_fields} = extraction;
  if (decision !== 'ready' || !value) {
    const missing = missing_fields?.length ? ` Missing: ${missing_fields.join(', ')}.` : '';
    throw new Error(`The document did not state one exact amount due, currency, reference and HTTPS payment URL.${missing}`);
  }
  const scale = MINOR_UNIT_DIGITS[connector]?.[value.currency];
  if (scale === undefined) {
    throw new Error(`${connector} has no bound unit for ${value.currency} here. Supply exact minor units to the invoice-pay runner.`);
  }
  return {...value, scale, source_digest, citations};
}

const DOCUMENT_TERMS = {
  link: ['resource_url', 'amount_minor', 'currency'],
  'wallet/evm': ['amount_minor'],
};

/** Complete the selected method's request from the document's terms, which the caller may not restate. */
export function documentRequest({connector, request, terms, amount_minor}) {
  const stated = DOCUMENT_TERMS[connector].filter((field) => field in request);
  if (stated.length || request.resource?.url) {
    throw new Error(`The document supplies ${stated.length ? stated.join(', ') : 'resource.url'}; leave it out of the request.`);
  }
  if (connector === 'link') {
    return {request: {...request, resource_url: terms.payment_url, amount_minor, currency: terms.currency}};
  }
  return {request: {...request, resource: {...request.resource, url: terms.payment_url}, amount_minor}};
}
