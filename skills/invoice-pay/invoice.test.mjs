import assert from "node:assert/strict";
import { test } from "node:test";
import { admitInvoice, documentRequest, invoiceTerms } from "./invoice.mjs";
const request = {
  action: "pay",
  resource_url: "https://merchant.example/invoice/42",
  amount_minor: 150,
  currency: "EUR",
};
const invoice = {
  invoice_id: "invoice-42",
  amount_minor: 150,
  currency: "EUR",
  payee: { payment_url: request.resource_url },
  connector: "link",
  request,
};
test("invoice binding preserves a non-EVM, non-USD provider request without inventing an address or FX", () => {
  assert.deepEqual(admitInvoice(invoice), { payment_request: request });
  for (const change of [
    { amount_minor: 151 },
    { currency: "USD" },
    { payee: { payment_url: "https://other.example/invoice" } },
  ])
    assert.throws(() => admitInvoice({ ...invoice, ...change }));
});

const terms = {
  invoice_id: "INV-1",
  amount_decimal: "42.00",
  currency: "USD",
  payment_url: "https://payee.example/pay",
};
const ready = {
  decision: "ready",
  value: terms,
  citations: [],
  source_digest: "sha256:a",
  missing_fields: [],
};
test("invoice terms come from a ready extraction, at the unit the selected method counts in", () => {
  for (const connector of ["wallet/evm", "link"])
    assert.equal(invoiceTerms({ extraction: ready, connector }).scale, 2);
  assert.throws(
    () =>
      invoiceTerms({
        extraction: { decision: "held", value: null, missing_fields: ["/payment_url"] },
        connector: "link",
      }),
    /Missing: \/payment_url/,
  );
  assert.throws(
    () =>
      invoiceTerms({
        extraction: { ...ready, value: { ...terms, currency: "EUR" } },
        connector: "link",
      }),
    /no bound unit/,
  );
});
test("the document completes the method request and the caller may not restate its terms", () => {
  assert.deepEqual(
    documentRequest({ connector: "link", request: { action: "pay" }, terms, amount_minor: 4200 }),
    {
      request: {
        action: "pay",
        resource_url: terms.payment_url,
        amount_minor: 4200,
        currency: "USD",
      },
    },
  );
  assert.equal(
    documentRequest({
      connector: "wallet/evm",
      request: { resource: { method: "POST", body: {} } },
      terms,
      amount_minor: 4200,
    }).request.resource.url,
    terms.payment_url,
  );
  for (const request of [{ amount_minor: 1 }, { resource: { url: "https://other.example" } }])
    assert.throws(
      () => documentRequest({ connector: "wallet/evm", request, terms, amount_minor: 4200 }),
      /leave it out/,
    );
});
