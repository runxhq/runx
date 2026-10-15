import assert from "node:assert/strict";
import test from "node:test";

import { admitInvoice } from "./invoice.mjs";

const PAYEE = { payment_url: "https://payee.example/v1/invoices/pay", pay_to: "0x00000000000000000000000000000000000000a1" };
const AUTHORITY = { term_id: "t", currency: "USD", max_amount_minor: 5000, counterparty: PAYEE.payment_url };
const invoice = (overrides = {}) => ({
  invoice_ref: "INV-1", amount_minor: 4200, currency: "USD", payee: PAYEE, parent_payment_authority: AUTHORITY, ...overrides,
});

test("an invoice within its authority becomes the exact payment pay makes", () => {
  assert.deepEqual(admitInvoice(invoice()), {
    payment_signal: {
      resource: { url: PAYEE.payment_url, method: "POST", body: { invoice_ref: "INV-1" } },
      amount_minor: 4200,
      pay_to: PAYEE.pay_to,
    },
  });
});

test("an invoice the authority does not cover is refused before anything is paid", () => {
  assert.throws(() => admitInvoice(invoice({ amount_minor: 9000 })), /exceeds the payment authority/);
  assert.throws(() => admitInvoice(invoice({ amount_minor: 0 })), /positive whole number/);
  assert.throws(() => admitInvoice(invoice({ currency: "EUR" })), /pay settles USD/);
  assert.throws(() => admitInvoice(invoice({ parent_payment_authority: { ...AUTHORITY, counterparty: "https://other.example/pay" } })), /does not name this invoice's payee/);
  assert.throws(() => admitInvoice(invoice({ payee: { ...PAYEE, payment_url: "http://payee.example/pay" } })), /exact HTTPS payment endpoint/);
  assert.throws(() => admitInvoice(invoice({ payee: { ...PAYEE, pay_to: "acct_123" } })), /EVM address/);
});
