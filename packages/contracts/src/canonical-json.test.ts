import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";

import {
  RUNX_STABLE_JSON_V1,
  canonicalJsonStringify,
  sha256Hex,
  sha256Prefixed,
} from "./index.js";

interface CanonicalJsonFixture {
  readonly canonicalization: string;
  readonly cases: readonly CanonicalJsonCase[];
}

interface CanonicalJsonCase {
  readonly name: string;
  readonly value: unknown;
  readonly expected_canonical_json: string;
  readonly expected_utf8_hex: string;
  readonly expected_sha256_hex: string;
  readonly expected_sha256: string;
}

interface ReceiptOracleFixture {
  readonly canonicalization: string;
  readonly cases: readonly ReceiptOracleCase[];
}

interface ReceiptOracleCase {
  readonly name: string;
  readonly fixture: string;
  readonly full_canonical_json: string;
  readonly full_sha256: string;
  readonly body_canonical_json: string;
  readonly body_sha256: string;
}

interface HarnessSpineFixture {
  readonly expected: unknown;
}

const fixtureUrl = new URL(
  "../../../fixtures/contracts/canonical-json/runx-stable-json-v1.cases.json",
  import.meta.url,
);

const fixture = JSON.parse(readFileSync(fixtureUrl, "utf8")) as CanonicalJsonFixture;

const numbersFixtureUrl = new URL(
  "../../../fixtures/contracts/canonical-json/runx-stable-json-v1.numbers.cases.json",
  import.meta.url,
);

const numbersFixture = JSON.parse(readFileSync(numbersFixtureUrl, "utf8")) as CanonicalJsonFixture;

const canonicalJsonCases = [fixture, numbersFixture].flatMap((fixture) => fixture.cases);

const receiptOracleUrl = new URL(
  "../../../fixtures/contracts/canonical-json/runx-receipt-c14n-v1.oracles.json",
  import.meta.url,
);

const receiptOracle = JSON.parse(
  readFileSync(receiptOracleUrl, "utf8"),
) as ReceiptOracleFixture;

describe("runx.stable-json.v1 canonical JSON", () => {
  it("exports the canonicalization tag", () => {
    expect(RUNX_STABLE_JSON_V1).toBe("runx.stable-json.v1");
    expect(fixture.canonicalization).toBe(RUNX_STABLE_JSON_V1);
    expect(numbersFixture.canonicalization).toBe(RUNX_STABLE_JSON_V1);
  });

  it("hashes strings and bytes with SHA-256", () => {
    const digest = "8186b7035bea2f66ebe27c1f5cf7de4e94ef935e259a2f3160352adffc752f28";

    expect(sha256Hex("runx")).toBe(digest);
    expect(sha256Hex(Buffer.from("runx", "utf8"))).toBe(digest);
    expect(sha256Prefixed("runx")).toBe(`sha256:${digest}`);
  });

  it.each(canonicalJsonCases.map((testCase) => [testCase.name, testCase] as const))(
    "matches fixture bytes and digests for %s",
    (_name, testCase) => {
      const actual = canonicalJsonStringify(testCase.value);

      expect(actual).toBe(testCase.expected_canonical_json);
      expect(Buffer.from(actual, "utf8").toString("hex")).toBe(testCase.expected_utf8_hex);
      expect(sha256Hex(actual)).toBe(testCase.expected_sha256_hex);
      expect(sha256Prefixed(actual)).toBe(testCase.expected_sha256);
    },
  );

  it.each([
    ["undefined root", undefined, "runx.stable-json.v1: unsupported undefined at $"],
    [
      "undefined object field",
      { value: undefined },
      "runx.stable-json.v1: unsupported undefined at $[\"value\"]",
    ],
    ["array hole", [, "present"], "runx.stable-json.v1: unsupported array hole at $[0]"],
    [
      "function",
      { value: () => undefined },
      "runx.stable-json.v1: unsupported function at $[\"value\"]",
    ],
    [
      "symbol",
      { value: Symbol("value") },
      "runx.stable-json.v1: unsupported symbol at $[\"value\"]",
    ],
    ["NaN", NaN, "runx.stable-json.v1: unsupported NaN at $"],
    ["Infinity", Infinity, "runx.stable-json.v1: unsupported Infinity at $"],
    ["-Infinity", -Infinity, "runx.stable-json.v1: unsupported -Infinity at $"],
    [
      "unpaired surrogate",
      "\uD800",
      "runx.stable-json.v1: unsupported unpaired surrogate at $[0]",
    ],
  ] as const)("rejects unsupported value: %s", (_name, value, message) => {
    expect(captureErrorMessage(() => canonicalJsonStringify(value))).toBe(message);
  });
});

describe("runx.stable-json.v1 integers", () => {
  it("writes a 64-bit integer exactly, as the Rust writer does", () => {
    expect(canonicalJsonStringify({ units: 9007199254740993n })).toBe('{"units":9007199254740993}');
  });
});

describe("runx.receipt.c14n.v1 conformance", () => {
  it("uses Rust receipt canonicalization as the oracle", () => {
    expect(receiptOracle.canonicalization).toBe("runx.receipt.c14n.v1");
  });

  it.each(receiptOracle.cases.map((testCase) => [testCase.name, testCase] as const))(
    "matches Rust full receipt canonical JSON and digest for %s",
    (_name, testCase) => {
      const fixture = readHarnessSpineFixture(testCase.fixture);
      const actual = canonicalJsonStringify(fixture.expected);

      expect(actual).toBe(testCase.full_canonical_json);
      expect(sha256Prefixed(actual)).toBe(testCase.full_sha256);
    },
  );

  it.each(receiptOracle.cases.map((testCase) => [testCase.name, testCase] as const))(
    "matches Rust body receipt canonical JSON and digest for %s",
    (_name, testCase) => {
      const fixture = readHarnessSpineFixture(testCase.fixture);
      const actual = canonicalJsonStringify(stripBodyProofFields(fixture.expected, true));

      expect(actual).toBe(testCase.body_canonical_json);
      expect(sha256Prefixed(actual)).toBe(testCase.body_sha256);
    },
  );
});

function captureErrorMessage(action: () => unknown): string {
  try {
    action();
  } catch (error) {
    return error instanceof Error ? error.message : String(error);
  }
  throw new Error("expected action to throw");
}

function readHarnessSpineFixture(fixture: string): HarnessSpineFixture {
  const fixtureUrl = new URL(`../../../fixtures/contracts/${fixture}`, import.meta.url);
  return JSON.parse(readFileSync(fixtureUrl, "utf8")) as HarnessSpineFixture;
}

function stripBodyProofFields(value: unknown, isRoot: boolean): unknown {
  // The signed body commits every flat field except the envelope's own
  // signature and digest. metadata is a runtime read aid, never signed.
  if (isRoot && isJsonRecord(value)) {
    const stripped: Record<string, unknown> = {};
    for (const key of Object.keys(value)) {
      if (key === "signature" || key === "digest" || key === "metadata") {
        continue;
      }
      stripped[key] = value[key];
    }
    return stripped;
  }
  return value;
}

function isJsonRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value && typeof value === "object" && !Array.isArray(value));
}
