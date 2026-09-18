import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";

import {
  RUNX_STABLE_JSON_V1,
  canonicalJsonStringify,
  fingerprintCancelPaidInvocationRequest,
  fingerprintExecutePaidInvocationRequest,
  fingerprintQuotePaidInvocationRequest,
  sha256Prefixed,
  type CancelPaidInvocationRequestContract,
  type ExecutePaidInvocationRequestContract,
  type QuotePaidInvocationRequestContract,
} from "./index.js";
import { PAID_INVOCATION_REQUEST_FINGERPRINT_SCHEMA } from "./paid-invocation-fingerprint.js";

interface FingerprintOracle {
  readonly canonicalization: string;
  readonly cases: readonly FingerprintCase[];
  readonly schema: string;
}

interface FingerprintCase {
  readonly canonical_json: string;
  readonly expected_sha256: string;
  readonly name: string;
  readonly operation:
    | "QuotePaidInvocation"
    | "ExecutePaidInvocation"
    | "CancelPaidInvocation";
  readonly preimage: unknown;
  readonly request: unknown;
}

const oracle = JSON.parse(readFileSync(new URL(
  "../../../fixtures/contracts/canonical-json/runx-paid-invocation-request-fingerprint-v1.oracles.json",
  import.meta.url,
), "utf8")) as FingerprintOracle;

describe("paid invocation request fingerprints", () => {
  it("pins the fingerprint and canonicalization contracts", () => {
    expect(PAID_INVOCATION_REQUEST_FINGERPRINT_SCHEMA)
      .toBe("runx.payment.request_fingerprint.v1");
    expect(oracle.schema).toBe("runx.canonical_json_oracle.v1");
    expect(oracle.canonicalization).toBe(RUNX_STABLE_JSON_V1);
  });

  it.each(oracle.cases.map((testCase) => [testCase.name, testCase] as const))(
    "matches Rust canonical bytes and digest for %s",
    (_name, testCase) => {
      const actual = fingerprint(testCase);

      expect(canonicalJsonStringify(testCase.preimage)).toBe(testCase.canonical_json);
      expect(sha256Prefixed(testCase.canonical_json)).toBe(testCase.expected_sha256);
      expect(actual).toBe(testCase.expected_sha256);
    },
  );

  it("binds a quote without its vendor presentation", () => {
    const base = oracle.cases.find(({ name }) => name === "quote-base");
    const presented = oracle.cases.find(({ name }) => name === "quote-presentation");
    expect(base).toBeDefined();
    expect(presented).toBeDefined();
    expect((presented?.request as { presentation?: unknown }).presentation).toBeDefined();
    // The presentation is what a buyer sees, never what binds the quote: a
    // replay that omits it (hosted admission rebuilds the quote from the
    // durable invocation) names the same quote.
    expect(presented?.expected_sha256).toBe(base?.expected_sha256);
    expect(fingerprintQuotePaidInvocationRequest(presented?.request as QuotePaidInvocationRequestContract))
      .toBe(fingerprintQuotePaidInvocationRequest(base?.request as QuotePaidInvocationRequestContract));
  });

  it("rejects amounts outside the shared portable integer contract", () => {
    const base = oracle.cases.find(({ name }) => name === "quote-base");
    expect(base).toBeDefined();

    expect(() => fingerprintQuotePaidInvocationRequest({
      ...(base?.request as QuotePaidInvocationRequestContract),
      amount_minor: Number.MAX_SAFE_INTEGER + 1,
    })).toThrow(/amount_minor/u);
  });

  it("rejects identity discriminants that Rust request types do not admit", () => {
    const base = quoteBaseRequest();

    expect(() => fingerprintQuotePaidInvocationRequest({
      ...base,
      schema: "runx.payment.quote_paid_invocation.request.v1",
    } as QuotePaidInvocationRequestContract)).toThrow(/request\["schema"\]/u);
    expect(() => fingerprintQuotePaidInvocationRequest({
      ...base,
      principal: {
        ...base.principal,
        schema: "runx.reference.v1",
      },
    } as QuotePaidInvocationRequestContract)).toThrow(/principal.*schema/u);
  });

  it("rejects present undefined members instead of normalizing the replay preimage", () => {
    expect(() => fingerprintQuotePaidInvocationRequest({
      ...quoteBaseRequest(),
      parent: undefined,
    })).toThrow(/parent.*omit undefined/u);
  });
});

function fingerprint(testCase: FingerprintCase): string {
  switch (testCase.operation) {
    case "QuotePaidInvocation":
      return fingerprintQuotePaidInvocationRequest(
        testCase.request as QuotePaidInvocationRequestContract,
      );
    case "ExecutePaidInvocation":
      return fingerprintExecutePaidInvocationRequest(
        testCase.request as ExecutePaidInvocationRequestContract,
      );
    case "CancelPaidInvocation":
      return fingerprintCancelPaidInvocationRequest(
        testCase.request as CancelPaidInvocationRequestContract,
      );
  }
}

function quoteBaseRequest(): QuotePaidInvocationRequestContract {
  const base = oracle.cases.find(({ name }) => name === "quote-base");
  if (base === undefined) {
    throw new Error("fingerprint oracle is missing quote-base");
  }
  return base.request as QuotePaidInvocationRequestContract;
}
