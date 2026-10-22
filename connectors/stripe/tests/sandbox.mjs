// Explicit, opt-in provider proof. Never part of core CI or the package test glob.
// Uses test money only. The native operation lifecycle is tested separately.
import { createHash, randomUUID } from "node:crypto";
import { execFileSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { operationInputError, renderDeclaredRequest } from "../../../packages/contracts/dist/index.js";

const key = process.env.STRIPE_TEST_SECRET_KEY;
if (!/^(?:sk|rk)_test_[A-Za-z0-9]+$/.test(key ?? "")) throw new Error("STRIPE_TEST_SECRET_KEY must be a Stripe test key");
const evidenceFile = process.argv[2];
if (!evidenceFile || existsSync(evidenceFile)) throw new Error("Supply a new evidence file; existing attempts must be reconciled, not resubmitted");
const root = fileURLToPath(new URL("../../../", import.meta.url));
const isolated = mkdtempSync(path.join(tmpdir(), "stripe-sandbox-inspect-"));
let manifest;
try {
  const inspected = JSON.parse(execFileSync(process.env.RUNX_BIN ?? path.join(root, "crates/target/debug/runx"), ["connector", "inspect", "stripe", "--json"], {
    cwd: root,
    env: { PATH: process.env.PATH ?? "/usr/bin:/bin", RUNX_CWD: root, RUNX_PROJECT_DIR: isolated, NO_COLOR: "1" },
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
  }));
  if (inspected.status !== "success") throw new Error("Native connector inspection failed");
  manifest = inspected.connector;
} finally {
  rmSync(isolated, { recursive: true, force: true });
}

const run = `runx-sandbox-${randomUUID()}`;
const evidence = {
  kind: "stripe_sandbox_provider_proof", run, started_at: new Date().toISOString(),
  manifest_sha256: createHash("sha256").update(readFileSync(path.join(root, "connectors/stripe/connector.yaml"))).digest("hex"),
  mode: "test", status: "running", checks: [],
};
function save() { writeFileSync(evidenceFile, `${JSON.stringify(evidence, null, 2)}\n`, { mode: 0o600 }); }
function check(ok, label) { if (!ok) throw new Error(label); evidence.checks.push(label); save(); }
function effect(label) { return `runx:sha256:${createHash("sha256").update(`${run}:${label}`).digest("hex")}`; }
let step = "account";

async function request(operation, id, args, label, responses) {
  const declaration = manifest.operations[operation];
  if (operationInputError(declaration, args)) throw new Error("Inputs do not satisfy the native declaration");
  const rendered = renderDeclaredRequest(declaration.implementation.http, {
    arguments: args, request: id, idempotencyKey: effect(label), ...(responses ? { responses } : {}),
  });
  const destination = new URL(rendered.url);
  if (destination.origin !== "https://api.stripe.com" || destination.username || destination.password) throw new Error("Unexpected provider destination");
  const response = await fetch(rendered.url, {
    method: rendered.method, headers: { ...rendered.headers, authorization: `Bearer ${key}` },
    ...(rendered.body === undefined ? {} : { body: rendered.body }),
    redirect: "error", signal: AbortSignal.timeout(20_000),
  });
  const body = await response.json();
  if (!response.ok) return { ok: false, status: response.status, code: typeof body.error?.code === "string" ? body.error.code : "provider_refused" };
  if (body.livemode === true) throw new Error("Provider returned a live-mode object");
  return { ok: true, body };
}
function success(response) {
  if (!response.ok) throw new Error(`Provider refused (${response.status}, ${response.code})`);
  return response.body;
}
function publicPayment(body) {
  return { payment_ref: body.id, charge_ref: body.latest_charge, amount_minor: body.amount, amount_received: body.amount_received, currency: body.currency, status: body.status };
}
function publicRefund(body) {
  return { refund_ref: body.id, payment_ref: body.payment_intent, amount_minor: body.amount, currency: body.currency, status: body.status };
}
async function refund(amount, label) {
  const args = { account_id: evidence.account_ref, payment_ref: evidence.payment.payment_ref, amount_minor: amount, currency: "usd" };
  const original = success(await request("payment.stripe.refund", "original", args, label));
  if (original.id !== args.payment_ref || original.currency !== "usd" || original.status !== "succeeded") throw new Error("Original refund payment did not match");
  const result = await request("payment.stripe.refund", "refund", args, label);
  if (!result.ok) return result;
  const write = publicRefund(result.body);
  evidence.refunds.push(write); save();
  const read = success(await request("payment.stripe.refund", "refund_read", args, label, { write: { json: write } }));
  check(read.id === write.refund_ref && read.payment_intent === args.payment_ref && read.amount === amount && read.currency === "usd" && read.status === "succeeded", `${label}: independent refund readback`);
  return { ok: true, refund: write, args };
}
async function chargeRead() {
  return success(await request("payment.stripe.charge.read", "charge", {
    account_id: evidence.account_ref, charge_ref: evidence.payment.charge_ref,
  }, "charge-read"));
}

save();
try {
  const account = success(await request("account.read", "account", {}, "account"));
  check(/^acct_[A-Za-z0-9]+$/.test(account.id), "credential identifies the merchant account");
  evidence.account_ref = account.id; save();
  const args = { account_id: account.id, amount_minor: 150, currency: "usd", payment_method_ref: "pm_card_visa", reference: run };
  const before = success(await request("payment.stripe", "account", args, "collect"));
  check(before.id === args.account_id, "collection account precondition");
  step = "collection submission";
  evidence.collection_key = effect("collect"); evidence.collection_submission_started = true; save();
  const collection = success(await request("payment.stripe", "collect", args, "collect"));
  evidence.payment = publicPayment(collection); evidence.refunds = []; save();
  check(collection.livemode === false, "collection is explicitly test mode");
  step = "collection readback";
  const read = success(await request("payment.stripe", "payment", args, "collect", { write: { json: evidence.payment } }));
  check(read.id === collection.id && read.status === "succeeded" && read.amount === 150 && read.amount_received === 150 && read.currency === "usd" && read.metadata.runx_reference === run, "independent collection readback matches all terms");
  const duplicate = success(await request("payment.stripe", "collect", args, "collect"));
  check(duplicate.id === collection.id, "same collection key returns the original payment");
  step = "partial refund";
  const first = await refund(50, "first-refund");
  check(first.ok, "first partial refund succeeds");
  const repeated = success(await request("payment.stripe.refund", "refund", first.args, "first-refund"));
  check(repeated.id === first.refund.refund_ref, "same refund key returns the original refund");
  step = "concurrent partial refunds";
  const competing = await Promise.all([refund(50, "competing-50"), refund(75, "competing-75")]);
  check(competing.filter((item) => item.ok).length === 1 && competing.filter((item) => !item.ok).length === 1, "competing partial refunds cannot both exceed the remaining amount");
  evidence.concurrent_refusal = competing.find((item) => !item.ok); save();
  const after = await chargeRead();
  check(after.payment_intent === collection.id && after.amount === 150 && after.amount_refunded <= 150, "processor cumulative refunds remain within the original payment");
  step = "final refund";
  if (after.amount_refunded < 150) check((await refund(150 - after.amount_refunded, "final-refund")).ok, "remaining test balance refunded");
  const final = await chargeRead();
  check(final.amount_refunded === 150 && final.refunded === true, "original test payment is fully refunded");
  evidence.status = "passed";
} catch (error) {
  evidence.status = "failed";
  evidence.failed_step = step;
  // Never print the provider response, auth headers, client secret or key.
  evidence.failure = error instanceof Error && error.constructor === Error ? error.message : "Transport or contract failure; inspect the original attempt before retrying";
  process.exitCode = 1;
} finally {
  evidence.finished_at = new Date().toISOString(); save();
  console.log(JSON.stringify({ status: evidence.status, checks: evidence.checks.length, evidence: evidenceFile, ...(evidence.failed_step ? { failed_step: evidence.failed_step, failure: evidence.failure } : {}) }));
}
