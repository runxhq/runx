import assert from "node:assert/strict";
import { test } from "node:test";
import { execute, resourceBinding } from "../tools/link/payment/link.mjs";

const input = {
  action: "pay", phase: "perform", principal_ref: "runx:principal:link:account_1",
  spend_request_id: "sr_one", amount_minor: 150, currency: "USD",
  payment_method_id: "pm_one", network_id: "net_merchant",
  resource_url: "https://merchant.example/invoke", challenge: "public-challenge",
  method: "POST", body: "{\"job\":\"ocr\"}",
};
const request = {
  metadata: {runx_resource_binding:resourceBinding(input)},
  id: "sr_one", amount: 150, currency: "usd", payment_method: "pm_one",
  network_id: "net_merchant", status: "approved", credential_type: "shared_payment_token",
};
function harness(overrides = {}) {
  const calls = [];
  const cli = async (args) => {
    calls.push(args);
    if (args[0] === "--version") return;
    if (args[0] === "auth") return { authenticated: true };
    if (args[0] === "user-info") return { id: "account_1", ...overrides.account };
    if (args[0] === "spend-request") return { ...request, ...overrides.request };
    if (args[1] === "decode") return {
      method: "stripe", intent: "charge", network_id: "net_merchant",
      request_json: { amount: "150", currency: "usd" }, expires: "2099-01-01T00:00:00Z",
      ...overrides.challenge,
    };
    if (args[1] === "pay") {
      if (overrides.lost) throw new Error("link_cli_failed_or_timed_out");
      return { status: 200, body: "untrusted body", headers: { "secret-header": "never-return" } };
    }
    throw new Error("unexpected command");
  };
  return { cli, calls, payments: () => calls.filter((args) => args[0] === "mpp" && args[1] === "pay") };
}

test("presents only the pinned public challenge and reference, with no credential retrieval command", async () => {
  const h = harness();
  assert.deepEqual(await execute(input, h.cli), { spend_request_id: "sr_one" });
  assert.equal(h.payments().length, 1);
  assert.deepEqual(h.payments()[0], ["mpp", "pay", input.resource_url, "--spend-request-id", "sr_one",
    "--approved-challenge", "public-challenge", "--method", "POST", "--format", "json", "--data", input.body]);
  assert.ok(h.calls.every((args) => !args.includes("--include")));
});

test("account and exact spend terms refuse before presentation", async () => {
  for (const overrides of [
    { account: { id: "another_account" } },
    ...[{ amount: 151 }, { currency: "eur" }, { network_id: "another_merchant" },
      { payment_method: "another_method" }, { credential_type: "card" }, { status: "pending_approval" },
      { recurring: { interval: "month" } }].map((request) => ({ request })),
  ]) {
    const h = harness(overrides);
    assert.match((await execute(input, h.cli)).refused, /^link_/);
    assert.equal(h.payments().length, 0);
  }
});

test("changed protocol terms and an expired challenge refuse before presentation", async () => {
  for (const challenge of [
    { method: "other" }, { intent: "session" }, { network_id: "other" },
    { request_json: { amount: "151", currency: "usd" } },
    { request_json: { amount: "150", currency: "eur" } }, { expires: "2000-01-01T00:00:00Z" }, { expires: "not-a-time" },
  ]) {
    const h = harness({ challenge });
    assert.match((await execute(input, h.cli)).refused, /^link_/);
    assert.equal(h.payments().length, 0);
  }
});

test("a successful HTTP response does not substitute for independent payment readback", async () => {
  const h = harness();
  assert.equal((await execute({ ...input, phase: "readback" }, h.cli)).status, "pending");
  assert.equal(h.payments().length, 0);
  const failed = harness({ request: { payment_status_details: { outcome: "failure", amount: 150, currency: "usd" } } });
  assert.equal((await execute({ ...input, phase: "readback" }, failed.cli)).status, "failed");
  const mismatched = harness({ request: { link_transaction_id: "tx_one", payment_status_details: { outcome: "success", amount: 149, currency: "usd" } } });
  assert.equal((await execute({ ...input, phase: "readback" }, mismatched.cli)).status, "mismatch");
});

test("the original provider observation confirms payment without another credential presentation", async () => {
  const h = harness({ request: { link_transaction_id: "tx_one", payment_status_details: { outcome: "success", amount: 150, currency: "usd" }, shared_payment_token: { id: "must-not-leave-cli" } } });
  const observed = await execute({ ...input, phase: "readback" }, h.cli);
  assert.deepEqual(observed, { spend_request_id: "sr_one", amount_minor: 150, currency: "USD", status: "settled", payment_method_id: "pm_one", network_id: "net_merchant", resource_binding: resourceBinding(input), transaction_ref: "tx_one", payment_outcome: "success", paid_amount_minor: 150, paid_currency: "USD" });
  assert.deepEqual(await execute(input, h.cli), { spend_request_id: "sr_one" });
  assert.equal(h.payments().length, 0);
  assert.ok(!JSON.stringify(observed).includes("must-not-leave-cli"));
});

test("a lost submission response stays unknown and does not retry or create a spend request", async () => {
  const h = harness({ lost: true });
  await assert.rejects(execute(input, h.cli), /link_cli_failed/u);
  assert.equal(h.payments().length, 1);
  assert.ok(h.calls.every((args) => args[1] !== "create"));
});


test("an existing partial, mismatched, failed or refunded payment never triggers another presentation", async () => {
  for (const details of [
    { outcome: "success", amount: 149, currency: "usd" },
    { outcome: "success", amount: 150, currency: "eur" },
    { outcome: "failure", amount: 150, currency: "usd" },
    { outcome: "success", amount: 150, currency: "usd", refund_details: { amount: 150, currency: "usd", state: "succeeded" } },
  ]) {
    const h = harness({ request: { link_transaction_id: "tx_original", payment_status_details: details } });
    if (details.refund_details) {
      assert.match((await execute(input, h.cli)).refused, /^link_/);
      const read = await execute({ ...input, phase: "readback" }, h.cli);
      assert.equal(read.status, "refund_observed");
      assert.equal(read.refund_amount_minor, 150);
    } else assert.match((await execute(input, h.cli)).refused, /^link_/);
    assert.equal(h.payments().length, 0);
  }
});

test("spend request creation and readback preserve one idempotent authorization without reporting payment", async () => {
  const h = harness({ request: { status: "pending_approval", approval_url: "https://link.com/approve/sr_one", shared_payment_token: "must-not-leave-cli" } });
  const create = { ...input, action: "request", context: "Approve the Northstar document purchase", idempotency_key: "original-native-effect" };
  const result = await execute(create, h.cli);
  assert.equal(result.status, "pending_approval");
  assert.equal(result.approval_url, "https://link.com/approve/sr_one");
  assert.equal(result.spend_request_id, "sr_one");
  assert.ok(!JSON.stringify(result).includes("must-not-leave-cli"));
  const command = h.calls.find((args) => args[0] === "spend-request" && args[1] === "create");
  assert.equal(command[command.indexOf("--idempotency-key") + 1], "original-native-effect");
  const after = await execute({ ...create, phase: "readback", acknowledgement: { spend_request_id: "sr_one" } }, h.cli);
  assert.deepEqual(after, result);
  assert.equal(h.calls.filter((args) => args[0] === "spend-request" && args[1] === "create").length, 1);
  assert.equal(h.payments().length, 0);
});

test("readback refuses a substituted spend request and strips unsafe approval destinations", async () => {
  const wrong = harness({ request: { id: "sr_substituted" } });
  assert.equal((await execute({ ...input, action: "read" }, wrong.cli)).refused, "link_spend_request_identity_changed");
  const h = harness({ request: { status: "pending_approval", approval_url: "https://user:secret@link.com/approve" } });
  const read = await execute({ ...input, action: "read" }, h.cli);
  assert.equal(read.status, "pending_approval");
  assert.equal(read.approval_url, undefined);
  assert.equal(h.payments().length, 0);
});

test("a spent authorization cannot be relabeled as another resource, body or challenge", async () => {
  for (const patch of [{resource_url:'https://merchant.example/another-invoice'}, {body:'{"invoice_ref":"other"}'}, {challenge:'another-public-challenge'}]) {
    const h=harness({request:{link_transaction_id:'tx_original',payment_status_details:{outcome:'success',amount:150,currency:'usd'}}});
    assert.equal((await execute({...input,...patch},h.cli)).refused,"link_spend_request_resource_mismatch");
    assert.equal(h.payments().length,0);
  }
});
