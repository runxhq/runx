import {
  RUNX_STABLE_JSON_V1,
  canonicalJsonStringify,
  sha256Prefixed,
} from "./canonical-json.js";
import type {
  CancelPurchaseRequestContract,
  ExecutePurchaseRequestContract,
  QuotePurchaseRequestContract,
} from "./generated/contract-types.js";
import {
  validateCancelPurchaseRequestContract,
  validateExecutePurchaseRequestContract,
  validateQuotePurchaseRequestContract,
} from "./schemas/purchase.js";

export const PURCHASE_REQUEST_DIGEST_SCHEMA =
  "runx.payment.request_digest.v1" as const;

export function digestQuotePurchaseRequest(
  request: QuotePurchaseRequestContract,
): string {
  const validated = validateQuotePurchaseRequestContract(request);
  assertRustRequestShape(validated);
  return digestRequest(
    "QuotePurchase",
    validated,
  );
}

export function digestExecutePurchaseRequest(
  request: ExecutePurchaseRequestContract,
): string {
  const validated = validateExecutePurchaseRequestContract(request);
  assertRustRequestShape(validated);
  return digestRequest(
    "ExecutePurchase",
    validated,
  );
}

export function digestCancelPurchaseRequest(
  request: CancelPurchaseRequestContract,
): string {
  const validated = validateCancelPurchaseRequestContract(request);
  assertRustRequestShape(validated);
  return digestRequest(
    "CancelPurchase",
    validated,
  );
}

function digestRequest(operation: string, request: unknown): string {
  return sha256Prefixed(canonicalJsonStringify({
    canonicalization: RUNX_STABLE_JSON_V1,
    operation,
    request,
    schema: PURCHASE_REQUEST_DIGEST_SCHEMA,
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
