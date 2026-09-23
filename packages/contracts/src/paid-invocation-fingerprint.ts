import {
  RUNX_STABLE_JSON_V1,
  canonicalJsonStringify,
  sha256Prefixed,
} from "./canonical-json.js";
import {
  type CancelPaidInvocationRequestContract,
  type ExecutePaidInvocationRequestContract,
  type QuotePaidInvocationRequestContract,
  validateCancelPaidInvocationRequestContract,
  validateExecutePaidInvocationRequestContract,
  validateQuotePaidInvocationRequestContract,
} from "./schemas/paid-invocation.js";

export const PAID_INVOCATION_REQUEST_FINGERPRINT_SCHEMA =
  "runx.payment.request_fingerprint.v1" as const;

/**
 * The quote fingerprint covers the binding request: everything the quote
 * commits to. The vendor presentation is what a buyer sees, never what binds
 * the quote, so a replay that omits or restates it names the same quote.
 * Hosted admission replays a paid run's quote from the durable invocation,
 * which carries no presentation, under the caller's idempotency key.
 */
export function fingerprintQuotePaidInvocationRequest(
  request: QuotePaidInvocationRequestContract,
): string {
  const {
    attribution: _attribution,
    presentation: _presentation,
    ...binding
  } = validateQuotePaidInvocationRequestContract(request);
  assertRustRequestShape(binding);
  return fingerprintRequest(
    "QuotePaidInvocation",
    binding,
  );
}

export function fingerprintExecutePaidInvocationRequest(
  request: ExecutePaidInvocationRequestContract,
): string {
  const validated = validateExecutePaidInvocationRequestContract(request);
  assertRustRequestShape(validated);
  return fingerprintRequest(
    "ExecutePaidInvocation",
    validated,
  );
}

export function fingerprintCancelPaidInvocationRequest(
  request: CancelPaidInvocationRequestContract,
): string {
  const validated = validateCancelPaidInvocationRequestContract(request);
  assertRustRequestShape(validated);
  return fingerprintRequest(
    "CancelPaidInvocation",
    validated,
  );
}

function fingerprintRequest(operation: string, request: unknown): string {
  return sha256Prefixed(canonicalJsonStringify({
    canonicalization: RUNX_STABLE_JSON_V1,
    operation,
    request,
    schema: PAID_INVOCATION_REQUEST_FINGERPRINT_SCHEMA,
  }));
}

function assertRustRequestShape(value: unknown, path = "request"): void {
  if (value === undefined) {
    throw new Error(`${path} must omit undefined members`);
  }
  if (Array.isArray(value)) {
    value.forEach((item, index) => assertRustRequestShape(item, `${path}[${index}]`));
    return;
  }
  if (value === null || typeof value !== "object") {
    return;
  }
  for (const [key, member] of Object.entries(value)) {
    const memberPath = `${path}[${JSON.stringify(key)}]`;
    if (key === "schema") {
      throw new Error(`${memberPath} is not part of the Rust request type`);
    }
    assertRustRequestShape(member, memberPath);
  }
}
