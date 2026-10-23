import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";

import {
  RUNX_STABLE_JSON_V1,
  canonicalJsonStringify,
  digestCancelPurchaseRequest,
  digestExecutePurchaseRequest,
  digestQuotePurchaseRequest,
  sha256Prefixed,
  type CancelPurchaseRequestContract,
  type ExecutePurchaseRequestContract,
  type QuotePurchaseRequestContract,
} from "./index.js";
import { PURCHASE_REQUEST_DIGEST_SCHEMA } from "./purchase-request-digest.js";

interface DigestOracle {
  readonly canonicalization: string;
  readonly cases: readonly DigestCase[];
  readonly schema: string;
}

interface DigestCase {
  readonly canonical_json: string;
  readonly expected_digest: string;
  readonly name: string;
  readonly operation:
    | "QuotePurchase"
    | "ExecutePurchase"
    | "CancelPurchase";
  readonly preimage: unknown;
  readonly request: unknown;
}

const oracle = JSON.parse(readFileSync(new URL(
  "../../../fixtures/contracts/canonical-json/runx-purchase-request-digest-v1.oracles.json",
  import.meta.url,
), "utf8")) as DigestOracle;

describe("purchase request digests", () => {
  it("pins the digest and canonicalization contracts", () => {
    expect(PURCHASE_REQUEST_DIGEST_SCHEMA)
      .toBe("runx.payment.request_digest.v1");
    expect(oracle.schema).toBe("runx.canonical_json_oracle.v1");
    expect(oracle.canonicalization).toBe(RUNX_STABLE_JSON_V1);
  });

  it.each(oracle.cases.map((testCase) => [testCase.name, testCase] as const))(
    "matches Rust canonical bytes and digest for %s",
    (_name, testCase) => {
      const actual = requestDigest(testCase);

      expect(canonicalJsonStringify(testCase.preimage)).toBe(testCase.canonical_json);
      expect(sha256Prefixed(testCase.canonical_json)).toBe(testCase.expected_digest);
      expect(actual).toBe(testCase.expected_digest);
    },
  );

  it("rejects amounts outside the shared portable integer contract", () => {
    const base = oracle.cases.find(({ name }) => name === "quote-base");
    expect(base).toBeDefined();

    expect(() => digestQuotePurchaseRequest({
      ...(base?.request as QuotePurchaseRequestContract),
      amount_minor: Number.MAX_SAFE_INTEGER + 1,
    })).toThrow(/amount_minor/u);
  });

  it("rejects identity discriminants that Rust request types do not admit", () => {
    const base = quoteBaseRequest();

    // The schema names only the fields the Rust type reads, so it refuses them.
    expect(() => digestQuotePurchaseRequest({
      ...base,
      schema: "runx.payment.quote_purchase.request.v1",
    } as QuotePurchaseRequestContract)).toThrow(/request must match/u);
    expect(() => digestQuotePurchaseRequest({
      ...base,
      buyer_ref: {
        ...base.buyer_ref,
        schema: "runx.reference.v1",
      },
    } as QuotePurchaseRequestContract)).toThrow(/request\.buyer_ref must match/u);
  });

  it("rejects present undefined members instead of normalizing the replay preimage", () => {
    const base = quoteBaseRequest();
    expect(() => digestQuotePurchaseRequest({
      ...base,
      skill_ref: { ...base.skill_ref, locator: undefined },
    })).toThrow(/skill_ref.*locator.*omit undefined/u);
  });
});

function requestDigest(testCase: DigestCase): string {
  switch (testCase.operation) {
    case "QuotePurchase":
      return digestQuotePurchaseRequest(
        testCase.request as QuotePurchaseRequestContract,
      );
    case "ExecutePurchase":
      return digestExecutePurchaseRequest(
        testCase.request as ExecutePurchaseRequestContract,
      );
    case "CancelPurchase":
      return digestCancelPurchaseRequest(
        testCase.request as CancelPurchaseRequestContract,
      );
  }
}

function quoteBaseRequest(): QuotePurchaseRequestContract {
  const base = oracle.cases.find(({ name }) => name === "quote-base");
  if (base === undefined) {
    throw new Error("request digest oracle is missing quote-base");
  }
  return base.request as QuotePurchaseRequestContract;
}
