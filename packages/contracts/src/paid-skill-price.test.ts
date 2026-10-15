import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";

import {
  PaidSkillClaimError,
  PaidSkillPriceError,
  paidSkillClaimProblem,
  paidSkillPriceAmount,
  paidSkillPricedArtifactRef,
  paidSkillPriceProblem,
  resolvePaidSkillClaim,
} from "./paid-skill-price.js";
import type { PaidSkillClaimContract, PaidSkillPriceContract } from "./schemas/paid-invocation.js";
import { paidSkillListingV1Schema } from "./schemas/paid-invocation.js";
import { validateContractSchema } from "./internal.js";

type Expected = { readonly amount_minor?: number; readonly url?: string; readonly error?: string };

interface PriceVector {
  readonly name: string;
  readonly expectation: "valid" | "invalid";
  readonly price: PaidSkillPriceContract;
  readonly calls?: readonly {
    readonly name: string;
    readonly inputs: Readonly<Record<string, unknown>>;
    readonly artifact_size_bytes?: number;
    readonly expected: Expected;
    readonly priced_artifact_ref: string | null;
  }[];
}

interface ClaimVector {
  readonly name: string;
  readonly expectation: "valid" | "invalid";
  readonly claim: PaidSkillClaimContract;
  readonly resolutions?: readonly { readonly name: string; readonly result: unknown; readonly expected: Expected }[];
}

const vectors = JSON.parse(readFileSync(
  new URL("../../../fixtures/contracts/paid-skill-price/paid-skill-price-v1.vectors.json", import.meta.url),
  "utf8",
)) as { readonly prices: readonly PriceVector[]; readonly claims: readonly ClaimVector[] };

/** A price is valid when its schema and its own invariants both accept it, as in Rust. */
function priceIsValid(price: PaidSkillPriceContract): boolean {
  const offer = {
    offer_revision: {
      offer_id: "acme/skill#run",
      revision: "1",
      revision_digest: `sha256:${"a".repeat(64)}`,
      input_schema_digest: `sha256:${"b".repeat(64)}`,
      output_schema_digest: `sha256:${"c".repeat(64)}`,
    },
    price,
    currency: "USD",
    accepted_settlement_families: ["x402"],
  };
  try {
    validateContractSchema(paidSkillListingV1Schema, {
      skill_id: "acme/skill",
      version: "1",
      skill_digest: `sha256:${"a".repeat(64)}`,
      profile_digest: `sha256:${"b".repeat(64)}`,
      package_digest: `sha256:${"c".repeat(64)}`,
      vendor_ref: { type: "principal", uri: "runx:principal:acme" },
      offers: { run: offer },
    }, "listing");
  } catch {
    return false;
  }
  return paidSkillPriceProblem(price) === undefined;
}

function priced(vector: NonNullable<PriceVector["calls"]>[number], price: PaidSkillPriceContract): Expected {
  try {
    return { amount_minor: paidSkillPriceAmount(price, vector.inputs, vector.artifact_size_bytes) };
  } catch (error) {
    if (error instanceof PaidSkillPriceError) return { error: error.code };
    throw error;
  }
}

function resolved(claim: PaidSkillClaimContract, result: unknown): Expected {
  try {
    return { url: resolvePaidSkillClaim(claim, result) };
  } catch (error) {
    if (error instanceof PaidSkillClaimError) return { error: error.code };
    throw error;
  }
}

describe("paid skill prices agree with the Rust contract", () => {
  for (const vector of vectors.prices) {
    it(`${vector.name} is ${vector.expectation}`, () => {
      expect(priceIsValid(vector.price)).toBe(vector.expectation === "valid");
      for (const call of vector.calls ?? []) {
        expect(priced(call, vector.price), call.name).toEqual(call.expected);
        expect(paidSkillPricedArtifactRef(vector.price, call.inputs) ?? null, call.name)
          .toEqual(call.priced_artifact_ref);
      }
    });
  }
});

describe("paid skill claims agree with the Rust contract", () => {
  for (const vector of vectors.claims) {
    it(`${vector.name} is ${vector.expectation}`, () => {
      expect(paidSkillClaimProblem(vector.claim) === undefined).toBe(vector.expectation === "valid");
      for (const resolution of vector.resolutions ?? []) {
        expect(resolved(vector.claim, resolution.result), resolution.name).toEqual(resolution.expected);
      }
    });
  }
});
