// Provider-package checks: the connector as the native CLI inspects it, and the
// requests a cold caller renders from that inspection. `pnpm test:skills` runs
// them in verify:fast.
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { test } from "node:test";
import { tsImport } from "tsx/esm/api";
import { resolveNativeRunxBinary } from "../../../scripts/lib/native-parser.mjs";

// The declared requests render through the contracts package's source, so
// they're checked against today's renderer, never a stale build of it.
const { operationInputError, renderDeclaredRequest } = await tsImport("../../../packages/contracts/src/index.ts", import.meta.url);

const root = fileURLToPath(new URL("../../../", import.meta.url));
const isolated = mkdtempSync(path.join(tmpdir(), "stripe-connector-inspect-"));
let manifest;
try {
  const inspected = JSON.parse(execFileSync(resolveNativeRunxBinary(), ["connector", "inspect", "stripe", "--json"], {
    cwd: root,
    env: { PATH: process.env.PATH ?? "/usr/bin:/bin", RUNX_CWD: root, RUNX_PROJECT_DIR: isolated, NO_COLOR: "1" },
    encoding: "utf8",
  }));
  assert.equal(inspected.status, "success");
  manifest = inspected.connector;
} finally {
  rmSync(isolated, { recursive: true, force: true });
}

const effectKey = `runx:sha256:${"a".repeat(64)}`;
const input = { account_id: "acct_northstar", amount_minor: 2500, currency: "aud", payment_method_id: "pm_saved42", reference: "northstar-invoice-42" };

function render(operation, request, args, extra = {}) {
  const declaration = manifest.operations[operation];
  assert.equal(operationInputError(declaration, args), undefined);
  return renderDeclaredRequest(declaration.implementation.http, { arguments: args, request, idempotencyKey: effectKey, ...extra });
}

test("a cold caller can construct the exact Stripe collection from native inspection", () => {
  const call = render("payment.stripe", "collect", input);
  assert.equal(call.method, "POST");
  assert.equal(call.url, "https://api.stripe.com/v1/payment_intents");
  assert.equal(call.headers["content-type"], "application/x-www-form-urlencoded");
  assert.equal(call.headers["idempotency-key"], effectKey);
  assert.equal(call.headers["stripe-account"], input.account_id);
  assert.match(call.headers["stripe-version"], /^\d{4}-\d{2}-\d{2}\.[a-z]+$/);
  assert.deepEqual(Object.fromEntries(new URLSearchParams(call.body)), {
    amount: "2500", currency: "aud", payment_method: "pm_saved42", confirm: "true",
    "automatic_payment_methods[enabled]": "true", "automatic_payment_methods[allow_redirects]": "never",
    "metadata[runx_reference]": "northstar-invoice-42", "metadata[runx_effect_key]": effectKey,
  });
});

test("collection reads the selected account before the write and the original PaymentIntent afterwards", () => {
  const account = render("payment.stripe", "account", input);
  assert.equal(account.method, "GET");
  assert.equal(account.url, "https://api.stripe.com/v1/account");
  const readback = render("payment.stripe", "payment", input, { responses: { write: { json: { payment_id: "pi_original42" } } } });
  assert.equal(readback.method, "GET");
  assert.equal(readback.url, "https://api.stripe.com/v1/payment_intents/pi_original42");
  assert.equal(readback.body, undefined);
  assert.equal(account.headers["stripe-account"], input.account_id);
  assert.equal(readback.headers["stripe-account"], input.account_id);
});

test("a refund names the original payment and uses the same native key, without marketplace transfers", () => {
  const terms = { account_id: "acct_northstar", payment_id: "pi_original42", amount_minor: 500, currency: "aud" };
  const original = render("payment.stripe.refund", "original", terms);
  assert.equal(original.url, "https://api.stripe.com/v1/payment_intents/pi_original42");
  const call = render("payment.stripe.refund", "refund", terms);
  assert.deepEqual(Object.fromEntries(new URLSearchParams(call.body)), { amount: "500", payment_intent: "pi_original42", "metadata[runx_effect_key]": effectKey });
  assert.equal(call.headers["idempotency-key"], effectKey);
  assert.equal(call.headers["stripe-account"], terms.account_id);
  assert.equal(render("payment.stripe.refund", "refund_read", terms, { responses: { write: { json: { refund_id: "re_original42" } } } }).url, "https://api.stripe.com/v1/refunds/re_original42");
});

test("invalid amounts and identifiers fail the exposed input contract", () => {
  for (const patch of [{ amount_minor: 0 }, { amount_minor: 0.5 }, { amount_minor: 100000000 }, { currency: "USDC" }, { account_id: "acct_northstar/../../other" }, { payment_method_id: "pm_42?account=other" }]) {
    assert.equal(typeof operationInputError(manifest.operations["payment.stripe"], { ...input, ...patch }), "string");
  }
  assert.equal(typeof operationInputError(manifest.operations["payment.stripe.lookup"], { account_id: "acct_northstar", effect_key: "' OR amount>0" }), "string");
});

test("merchant collection is not advertised as a buyer wallet or an unlimited replay guarantee", () => {
  assert.equal(manifest.pays, undefined);
  assert.equal(manifest.operations["payment.stripe"].guarantees?.provider_idempotency ?? "none", "none");
  assert.equal(manifest.operations["payment.stripe.refund"].guarantees?.provider_idempotency ?? "none", "none");
  assert.equal(manifest.operations["payment.stripe"].approval, "required");
  assert.equal(manifest.operations["payment.stripe.refund"].approval, "required");
  assert.equal(manifest.operations["payment.stripe"].amount, undefined);
  assert.deepEqual(manifest.operations["payment.stripe.refund"].amount, {
    units: { $argument: "/amount_minor" },
    currency: { $argument: "/currency" },
  });
});

test("all requests pin one stable provider API and charge reads expose cumulative refunds", () => {
  const versions = new Set();
  for (const op of Object.values(manifest.operations)) {
    const http = op.implementation.http;
    for (const req of [...http.requests, ...(http.preconditions ?? []).map((item) => item.request), ...(http.readbacks ?? []).map((item) => item.request)]) {
      assert.match(req.headers["stripe-version"], /^\d{4}-\d{2}-\d{2}\.[a-z]+$/);
      versions.add(req.headers["stripe-version"]);
      if (op !== manifest.operations["account.read"]) assert.deepEqual(req.headers["stripe-account"], { $argument: "/account_id" });
    }
  }
  assert.equal(versions.size, 1);
  const call = render("payment.stripe.charge.read", "charge", { account_id: "acct_northstar", charge_id: "ch_original42" });
  assert.equal(call.url, "https://api.stripe.com/v1/charges/ch_original42");
  assert.ok(manifest.operations["payment.stripe.charge.read"].result_fields.includes("amount_refunded"));
});

const disputeTerms = {
  account_id: "acct_northstar", dispute_id: "du_case42", charge_id: "ch_original42",
  amount_minor: 2500, currency: "aud", expected_status: "needs_response",
  amount_refunded: 0,
  expected_evidence: {},
  prior_submission_count: 0, submission_count: 1,
  evidence_form: {
    submit: true,
    "evidence[product_description]": "The purchased report.",
    "evidence[access_activity_log]": "Report downloaded at the recorded time.",
    "evidence[service_documentation]": "file_delivery42",
  },
};

test("dispute filing submits exact structured evidence and never stages or concedes", () => {
  const call = render("dispute.file", "file", disputeTerms);
  assert.equal(call.method, "POST");
  assert.equal(call.url, "https://api.stripe.com/v1/disputes/du_case42");
  assert.equal(call.headers["idempotency-key"], effectKey);
  assert.deepEqual(Object.fromEntries(new URLSearchParams(call.body)), {
    ...disputeTerms.evidence_form, submit: "true",
  });
  assert.equal(manifest.operations["dispute.file"].approval, "required");
  assert.equal(render("dispute.file", "before", disputeTerms).method, "GET");
  assert.equal(render("dispute.file", "after", disputeTerms).method, "GET");
  assert.equal(render("dispute.file", "after", disputeTerms).url, call.url);
  assert.ok(!Object.keys(manifest.operations).some((name) => /accept|concede|close/.test(name)));
});

test("dispute checks bind the selected case and payment before filing and require a new submission afterwards", () => {
  const http = manifest.operations["dispute.file"].implementation.http;
  const before = http.preconditions.find((item) => item.request.id === "before");
  for (const field of ["dispute_id", "charge_id", "amount_minor", "currency", "status", "submission_count", "past_due", "evidence"]) {
    assert.ok(before.matches.some((item) => item.observed === field), field);
  }
  const after = http.readbacks[0];
  assert.ok(after.matches.some((item) => item.observed === "submission_count" && item.expected.path === "submission_count"));
  assert.ok(after.matches.some((item) => item.observed === "has_evidence" && item.expected.value === true));
  assert.equal(http.result_paths.evidence, "readbacks.0.evidence");
  assert.equal(http.result_paths.submission_count, "readbacks.0.submission_count");
});

test("dispute operation admission refuses draft staging, arbitrary form fields and unsafe provider IDs", () => {
  for (const patch of [
    { dispute_id: "du_case42/close" },
    { evidence_form: { ...disputeTerms.evidence_form, submit: false } },
    { evidence_form: { submit: true } },
    { evidence_form: { ...disputeTerms.evidence_form, "metadata[pretend_verified]": "true" } },
    { evidence_form: { submit: true, "evidence[receipt]": "https://unverified.example/receipt.pdf" } },
    { submission_count: 0 }, { expected_status: "won" },
  ]) {
    assert.equal(typeof operationInputError(manifest.operations["dispute.file"], { ...disputeTerms, ...patch }), "string");
  }
});
