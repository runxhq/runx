import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { fileURLToPath } from "node:url";
import { createHash } from "node:crypto";

const exec = promisify(execFile);
const CLI = "link-cli";
export const CLI_VERSION = "0.29.0";
const MAX_BYTES = 262144;

// The official CLI alone owns auth storage, refresh and credential retrieval.
// Raw CLI stderr and unexpected output are never exposed to the graph.
export async function runCli(args) {
  let stdout;
  try {
    ({ stdout } = await exec(CLI, args, {
      encoding: "utf8", timeout: 20000, maxBuffer: MAX_BYTES,
      windowsHide: true, killSignal: "SIGKILL",
    }));
  } catch { throw new Error("link_cli_failed_or_timed_out"); }
  if (args[0] === "--version") {
    if (!new RegExp(`(?:^|[^0-9])${CLI_VERSION.replaceAll(".", "\\.")}(?:$|[^0-9.])`).test(stdout.trim())) {
      throw new Error("link_cli_version_mismatch");
    }
    return undefined;
  }
  let output;
  try { output = JSON.parse(stdout); } catch { throw new Error("link_cli_invalid_json"); }
  // Link CLI 0.29 emits one JSON result per yielded command result in an array.
  if (!Array.isArray(output) || output.length !== 1 || !record(output[0]) || output[0].error) {
    throw new Error("link_cli_did_not_return_one_result");
  }
  return output[0];
}

export async function execute(input, cli = runCli) {
  let dispatched = false;
  try {
    await cli(["--version"]);
    const status = await cli(["auth", "status", "--format", "json"]);
    if (status.authenticated !== true) throw new Error("link_authentication_required");
    const account = await cli(["user-info", "retrieve", "--format", "json"]);
    if (typeof account.id !== "string" || `runx:principal:link:${account.id}` !== input.principal_ref) {
      throw new Error("link_session_account_changed");
    }
    if (input.action === "account") return { account_ref: account.id };
    if (input.action === "request") {
      const args = ["spend-request", "create", "--credential-type", "shared_payment_token",
        "--amount", String(input.amount_minor), "--currency", input.currency.toLowerCase(),
        "--payment-method-id", input.payment_method_id, "--network-id", input.network_id,
        "--context", input.context, "--idempotency-key", input.idempotency_key,
        "--metadata", `runx_resource_binding:${resourceBinding(input)}`,
        "--request-approval", "--format", "json"];
      let request;
      if (input.phase === "readback") request = await read(input.acknowledgement?.spend_request_id, cli);
      else { dispatched = true; request = await cli(args); }
      requireTerms(request, input);
      return project(request);
    }
    const request = await read(input.spend_request_id, cli);
    if (input.action === "read") return project(request);
    if (input.action !== "pay") throw new Error("link_operation_unknown");
    requireTerms(request, input);
    const observed = paymentObservation(request, input);
    if (input.phase === "readback") return observed;
    if (input.phase !== "perform") throw new Error("link_payment_phase_required");
    if (request.payment_status_details?.outcome === "success") {
      if (observed.status !== "settled") throw new Error("link_payment_observation_mismatch");
      return { spend_request_id: request.id };
    }
    if (request.payment_status_details?.outcome === "failure") throw new Error("link_original_payment_failed");
    if (request.status !== "approved" || request.credential_type !== "shared_payment_token" || request.recurring) {
      throw new Error("link_one_time_spend_request_not_approved");
    }
    const url = new URL(input.resource_url);
    if (url.protocol !== "https:" || url.username || url.password || url.hash) throw new Error("link_payment_target_invalid");
    const decoded = await cli(["mpp", "decode", "--challenge", input.challenge, "--format", "json"]);
    const expiresAt = typeof decoded.expires === "string" ? Date.parse(decoded.expires) : NaN;
    if (decoded.method !== "stripe" || decoded.intent !== "charge"
      || decoded.network_id !== input.network_id
      || decoded.request_json?.amount !== String(input.amount_minor)
      || decoded.request_json?.currency !== input.currency.toLowerCase()
      || !Number.isFinite(expiresAt) || expiresAt <= Date.now()) {
      throw new Error("link_challenge_does_not_match_approved_terms");
    }
    if (!["GET", "POST"].includes(input.method) || (input.method === "GET" && input.body !== undefined)) {
      throw new Error("link_request_method_or_body_invalid");
    }
    // The CLI retrieves this exact spend request under the current session,
    // privately obtains its SPT, and enforces the pinned challenge/destination.
    // A session switch cannot substitute another account's spend request.
    const args = ["mpp", "pay", input.resource_url, "--spend-request-id", request.id,
      "--approved-challenge", input.challenge, "--method", input.method, "--format", "json"];
    if (input.body !== undefined) args.push("--data", input.body);
    dispatched = true;
    await cli(args); // HTTP response/body are untrusted and are not payment proof.
    return { spend_request_id: request.id };
  } catch (error) {
    // The process protocol distinguishes definite refusal from a call that
    // may have acted. Readback failures never erase an acknowledged effect.
    if (!dispatched && input.phase !== "readback") {
      return { refused: diagnosticCode(error) };
    }
    throw error;
  }
}

async function read(id, cli) {
  if (typeof id !== "string" || !/^[A-Za-z0-9_-]{1,200}$/.test(id)) throw new Error("link_spend_request_id_invalid");
  // Never request include=shared_payment_token or another credential field.
  const request = await cli(["spend-request", "retrieve", id, "--interval", "1", "--max-attempts", "1", "--format", "json"]);
  if (request.id !== id) throw new Error("link_spend_request_identity_changed");
  return request;
}

function requireTerms(request, input) {
  if (!Number.isSafeInteger(input.amount_minor) || input.amount_minor <= 0
    || !/^[A-Z]{3}$/.test(input.currency)
    || request.amount !== input.amount_minor || request.currency !== input.currency.toLowerCase()
    || request.payment_method !== input.payment_method_id || request.network_id !== input.network_id
    || request.credential_type !== "shared_payment_token" || request.recurring) {
    throw new Error("link_spend_request_terms_mismatch");
  }
  if (request.metadata?.runx_resource_binding !== resourceBinding(input)) {
    throw new Error("link_spend_request_resource_mismatch");
  }
}

/** Provider-owned metadata binds one authorization to its exact original request. */
export function resourceBinding(input) {
  const url = new URL(input.resource_url);
  if (url.protocol !== "https:" || url.username || url.password || url.hash
    || !["GET", "POST"].includes(input.method)
    || (input.method === "GET" && input.body !== undefined)
    || !bounded(input.challenge, 16384)) throw new Error("link_payment_target_invalid");
  return `sha256:${createHash("sha256").update(JSON.stringify([
    input.principal_ref, input.resource_url, input.method, input.body ?? null, input.challenge,
  ])).digest("hex")}`;
}

function project(request) {
  if (!bounded(request.id, 200) || !bounded(request.status, 80)
    || !Number.isSafeInteger(request.amount) || request.amount <= 0
    || !/^[a-z]{3}$/.test(request.currency) || !bounded(request.payment_method, 200)
    || !bounded(request.network_id, 200)) throw new Error("link_public_spend_observation_invalid");
  return {
    spend_request_id: request.id, amount_minor: request.amount,
    currency: typeof request.currency === "string" ? request.currency.toUpperCase() : undefined,
    status: request.status, payment_method_id: request.payment_method, network_id: request.network_id,
    ...(bounded(request.metadata?.runx_resource_binding, 71) ? { resource_binding: request.metadata.runx_resource_binding } : {}),
    ...(typeof request.approval_url === "string" && safeApprovalUrl(request.approval_url)
      ? { approval_url: request.approval_url } : {}),
    ...(bounded(request.link_transaction_id, 200) ? { transaction_ref: request.link_transaction_id } : {}),
    ...publicPaymentDetails(request.payment_status_details),
  };
}

function publicPaymentDetails(payment) {
  if (!record(payment) || !["success", "failure"].includes(payment.outcome)
    || !Number.isSafeInteger(payment.amount) || payment.amount < 0 || !/^[a-z]{3}$/.test(payment.currency)) return {};
  const refund = payment.refund_details;
  return {
    payment_outcome: payment.outcome, paid_amount_minor: payment.amount, paid_currency: payment.currency.toUpperCase(),
    ...(record(refund) && Number.isSafeInteger(refund.amount) && refund.amount > 0
      && /^[a-z]{3}$/.test(refund.currency) && bounded(refund.state, 80)
      ? { refund_amount_minor: refund.amount, refund_currency: refund.currency.toUpperCase(), refund_status: refund.state } : {}),
  };
}

function paymentObservation(request, input) {
  const payment = request.payment_status_details;
  const exact = payment?.outcome === "success" && payment.amount === input.amount_minor
    && payment.currency === input.currency.toLowerCase() && bounded(request.link_transaction_id, 200);
  return {
    spend_request_id: request.id, amount_minor: input.amount_minor, currency: input.currency,
    status: exact ? payment.refund_details ? "refund_observed" : "settled" : payment?.outcome === "success" ? "mismatch" : payment?.outcome === "failure" ? "failed" : "pending", payment_method_id: request.payment_method,
    network_id: request.network_id,
    resource_binding: request.metadata.runx_resource_binding,
    ...(exact ? { transaction_ref: request.link_transaction_id } : {}),
    ...publicPaymentDetails(payment),
  };
}

function diagnosticCode(error) {
  return /^link_[a-z_]+$/.test(error?.message ?? "") ? error.message : "link_operation_failed";
}

function safeApprovalUrl(value) {
  try { if (!bounded(value, 2048)) return false; const url = new URL(value); return url.protocol === "https:" && !url.username && !url.password; } catch { return false; }
}
function bounded(value, max) { return typeof value === "string" && value.length > 0 && value.length <= max && !/[\x00-\x1f\x7f]/.test(value); }
function record(value) { return value !== null && typeof value === "object" && !Array.isArray(value); }

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  try {
    let bytes = 0; const chunks = [];
    for await (const chunk of process.stdin) {
      bytes += chunk.length;
      if (bytes > MAX_BYTES) throw new Error("link_input_too_large");
      chunks.push(chunk);
    }
    const result = await execute(JSON.parse(Buffer.concat(chunks).toString("utf8")));
    process.stdout.write(`${JSON.stringify(result)}\n`);
  } catch (error) {
    // Only our fixed diagnostic codes may leave the trusted process.
    const code = diagnosticCode(error);
    process.stderr.write(`${code}\n`); process.exitCode = 1;
  }
}
