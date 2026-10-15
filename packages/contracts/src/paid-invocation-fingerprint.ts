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

export function fingerprintQuotePaidInvocationRequest(
  request: QuotePaidInvocationRequestContract,
): string {
  const validated = validateQuotePaidInvocationRequestContract(request);
  assertRustRequestShape(validated);
  return fingerprintRequest(
    "QuotePaidInvocation",
    validated,
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
    assertRustRequestShape(member, `${path}[${JSON.stringify(key)}]`);
  }
}
