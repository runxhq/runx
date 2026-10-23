// Explicit provider-package checks, separate from OSS core CI.
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { test } from "node:test";
import { operationAmount, operationInputError, renderDeclaredRequest, renderTemplate, renderOAuth2TokenEndpoint } from "../../../packages/contracts/dist/index.js";

const root = fileURLToPath(new URL("../../../", import.meta.url));
const isolated = mkdtempSync(path.join(tmpdir(), "paypal-connector-inspect-"));
let manifest;
try {
  const inspected = JSON.parse(execFileSync(process.env.RUNX_BIN ?? path.join(root, "crates/target/debug/runx"), ["connector", "inspect", "paypal", "--json"], {
    cwd: root, encoding: "utf8",
    env: { PATH: process.env.PATH ?? "/usr/bin:/bin", RUNX_CWD: root, RUNX_PROJECT_DIR: isolated, NO_COLOR: "1" },
  }));
  assert.equal(inspected.status, "success"); manifest = inspected.connector;
} finally { rmSync(isolated, { recursive: true, force: true }); }

const key = "00000000-0000-4000-8000-000000000042";
const terms = { api_host: "api-m.paypal.com", account_id: "YXZY75W2GKDQE", amount_decimal: "12.50", currency: "USD", precision: 2, reference: "invoice-42" };
const refund = { api_host: terms.api_host, account_id: terms.account_id, payment_id: "CAPTURE42", amount_decimal: "5.00", currency: "USD", precision: 2, original_status: "PARTIALLY_REFUNDED" };
const collect = { ...terms, order_id: "ORDER42" };
function render(operation, request, args, acknowledgement) {
  const declaration = manifest.operations[operation];
  assert.equal(operationInputError(declaration, args), undefined);
  return renderDeclaredRequest(declaration.implementation.http, { arguments: args, request, idempotencyKey: key,
    ...(acknowledgement ? { responses: { write: { json: acknowledgement } } } : {}) });
}

test("order creation exposes the buyer approval path without claiming payment", () => {
  const args = { ...terms, return_url: "https://merchant.example/complete", cancel_url: "https://merchant.example/cancel" };
  const request = render("payment.paypal.order.create", "order", args);
  assert.equal(request.url, "https://api-m.paypal.com/v2/checkout/orders");
  assert.equal(request.method, "POST");
  assert.equal(request.headers["paypal-request-id"], key);
  assert.deepEqual(JSON.parse(request.body), {
    intent: "CAPTURE",
    purchase_units: [{ reference_id: "invoice-42", custom_id: key, payee: { merchant_id: terms.account_id }, amount: { currency_code: "USD", value: "12.50" } }],
    payment_source: { paypal: { experience_context: { return_url: args.return_url, cancel_url: args.cancel_url, user_action: "PAY_NOW" } } },
  });
  assert.equal(operationAmount(manifest.operations["payment.paypal.order.create"], args), undefined);
  assert.ok(manifest.operations["payment.paypal.order.create"].result_fields.includes("links"));
});

test("capture requires an approved order and exact merchant terms before dispatch", () => {
  const operation = manifest.operations["payment.paypal"];
  const checks = operation.implementation.http.preconditions[0].matches;
  for (const path of ["account_id", "order_id", "amount_decimal", "currency", "reference"]) {
    assert.ok(checks.some((check) => check.expected.source === "argument" && check.expected.path === path && check.observed === path));
  }
  assert.ok(checks.some((check) => check.expected.value === "APPROVED" && check.observed === "order_status"));
  assert.ok(checks.some((check) => check.expected.value === "CAPTURE" && check.observed === "intent"));
  assert.equal(render("payment.paypal", "approved_order", collect).url, "https://api-m.paypal.com/v2/checkout/orders/ORDER42");
  const write = render("payment.paypal", "capture", collect);
  assert.equal(write.url, "https://api-m.paypal.com/v2/checkout/orders/ORDER42/capture");
  assert.deepEqual(JSON.parse(write.body), {});
  const acknowledged = { payment_id: "CAPTURE42" };
  assert.equal(render("payment.paypal", "capture_read", collect, acknowledged).url, "https://api-m.paypal.com/v2/payments/captures/CAPTURE42");
  assert.equal(operation.implementation.http.readbacks.length, 2);
  assert.equal(operationAmount(operation, collect), undefined, "incoming collection does not debit an outgoing budget");
});

test("a refund's wire amount is the same exact amount admitted by the native budget", () => {
  const request = render("payment.paypal.refund", "refund", refund);
  assert.equal(request.url, "https://api-m.paypal.com/v2/payments/captures/CAPTURE42/refund");
  assert.deepEqual(JSON.parse(request.body), { amount: { value: "5.00", currency_code: "USD" }, custom_id: key });
  assert.deepEqual(operationAmount(manifest.operations["payment.paypal.refund"], refund), { units: 500, unit: "USD" });
  assert.equal(render("payment.paypal.refund", "original_capture", refund).url, "https://api-m.paypal.com/v2/payments/captures/CAPTURE42");
  assert.equal(render("payment.paypal.refund", "refund_read", refund, { refund_id: "REFUND42" }).url, "https://api-m.paypal.com/v2/payments/refunds/REFUND42");
});

test("the declared currency precision refuses rounding and a smaller budget representation", () => {
  const operation = manifest.operations["payment.paypal.refund"];
  for (const currency of operation.input_schema.properties.currency.enum) {
    const zero = ["HUF", "JPY", "TWD"].includes(currency);
    const args = { ...refund, currency, amount_decimal: zero ? "500" : "5.00", precision: zero ? 0 : 2 };
    assert.equal(operationInputError(operation, args), undefined, currency);
    assert.deepEqual(operationAmount(operation, args), { units: 500, unit: currency });
    assert.equal(typeof operationInputError(operation, { ...args, precision: zero ? 2 : 0 }), "string");
  }
  for (const amount_decimal of ["0.00", "-5.00", "5.001", "5.0", "05.00", "5e2"]) assert.equal(typeof operationInputError(operation, { ...refund, amount_decimal }), "string");
});

test("refund confirmation binds the original capture through full relationship objects", () => {
  const readback = manifest.operations["payment.paypal.refund"].implementation.http.readbacks[0];
  const relationship = readback.matches.find((check) => check.expected.source === "template");
  assert.equal(relationship.observed, "relationships.*");
  const value = renderTemplate(relationship.expected.value, { arguments: refund, responses: { write: { json: { refund_id: "REFUND42" } } } });
  assert.deepEqual(value, [
    { rel: "self", method: "GET", href: "https://api-m.paypal.com/v2/payments/refunds/REFUND42" },
    { rel: "up", method: "GET", href: "https://api-m.paypal.com/v2/payments/captures/CAPTURE42" },
  ]);
  assert.ok(readback.matches.some((check) => check.observed === "status" && check.expected.value === "COMPLETED"));
});

test("app authentication and processor-private answers stay inside custody", () => {
  for (const operation of Object.values(manifest.operations)) {
    const http = operation.implementation.http;
    assert.equal(http.auth.app.type, "oauth2_client_credentials");
    assert.equal(http.auth.app.client_id, "client_id");
    assert.equal(http.auth.app.client_secret, "client_secret");
    assert.equal(renderOAuth2TokenEndpoint(http.auth.app.token_url, { arguments: terms }, http.allowed_hosts), "https://api-m.paypal.com/v1/oauth2/token");
    assert.equal(operation.implementation.http.decode.at(-1).op, "project");
    if (operation.verb !== "read") {
      assert.equal(operation.approval, "required");
      assert.equal(operation.idempotency_key_format, "uuid");
      assert.equal(operation.guarantees?.provider_idempotency ?? "none", "none");
    }
  }
  assert.equal(manifest.pays, undefined, "merchant app credentials do not advertise buyer-wallet authority");
  for (const patch of [{ payment_id: "OTHER/../../capture" }, { account_id: "app-not-merchant" }, { original_status: "PENDING" }]) assert.equal(typeof operationInputError(manifest.operations["payment.paypal.refund"], { ...refund, ...patch }), "string");
});

test("an uncertain capture can be observed by its original order without another submission", () => {
  const call = render("payment.paypal.lookup", "order", { api_host: terms.api_host, order_id: "ORDER42" });
  assert.equal(call.method, "GET");
  assert.equal(call.url, "https://api-m.paypal.com/v2/checkout/orders/ORDER42");
  assert.equal(call.body, undefined);
  assert.ok(manifest.operations["payment.paypal.lookup"].result_fields.includes("payment_id"));
  for (const name of ["payment.paypal.order.create", "payment.paypal", "payment.paypal.refund"]) {
    const decode = manifest.operations[name].implementation.http.decode;
    assert.deepEqual(Object.keys(decode.at(-1).fields).sort(), name === "payment.paypal" ? ["order_id", "payment_id"] : [name.endsWith("create") ? "order_id" : "refund_id"]);
  }
});

test("sandbox selection binds authentication, requests and approval targets to the same host", () => {
  const api_host = "api-m.sandbox.paypal.com";
  const args = { ...collect, api_host };
  const operation = manifest.operations["payment.paypal"];
  const http = operation.implementation.http;
  assert.equal(renderOAuth2TokenEndpoint(http.auth.app.token_url, { arguments: args }, http.allowed_hosts), `https://${api_host}/v1/oauth2/token`);
  assert.equal(render("payment.paypal", "capture", args).url, `https://${api_host}/v2/checkout/orders/ORDER42/capture`);
  assert.equal(renderTemplate(operation.target.template, { arguments: args }), `paypal://${api_host}/${terms.account_id}/orders/ORDER42`);
  for (const bad of ["attacker.example", "api-m.paypal.com@attacker.example", "api-m.sandbox.paypal.com:8443", undefined]) {
    assert.equal(typeof operationInputError(operation, { ...args, api_host: bad }), "string");
  }
});
