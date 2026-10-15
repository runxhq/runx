import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";

import {
  isPurchaserKey,
  PurchaseClaimError,
  type PurchaseTrustRootKey,
  verifyPurchaseClaim,
} from "./purchase-claim.js";

interface ClaimVectors {
  readonly claim_url: string;
  readonly max_skew_seconds: number;
  readonly now: string;
  readonly seller_principal_uri: string;
  readonly trust_root: { readonly keys: readonly PurchaseTrustRootKey[] };
  readonly cases: readonly {
    readonly name: string;
    readonly claim: unknown;
    readonly expected: { readonly invocation_id?: string; readonly purchaser_key?: string; readonly error?: string };
  }[];
}

const vectors = JSON.parse(readFileSync(
  new URL("../../../fixtures/contracts/purchase-claim/purchase-claim-v1.vectors.json", import.meta.url),
  "utf8",
)) as ClaimVectors;

async function verified(claim: unknown) {
  try {
    const result = await verifyPurchaseClaim({
      claim,
      trustRoot: vectors.trust_root,
      sellerPrincipalUri: vectors.seller_principal_uri,
      claimUrl: vectors.claim_url,
      now: new Date(vectors.now),
      maxSkewSeconds: vectors.max_skew_seconds,
    });
    return { invocation_id: result.invocationId, purchaser_key: result.purchaserKey };
  } catch (error) {
    if (error instanceof PurchaseClaimError) return { error: error.code };
    throw error;
  }
}

describe("purchase claims Rust signed verify as Rust expects", () => {
  for (const vector of vectors.cases) {
    it(vector.name, async () => {
      await expect(verified(vector.claim)).resolves.toEqual(vector.expected);
    });
  }

  it("spells a purchaser key one way, as Rust does", () => {
    const valid = vectors.cases.find((vector) => vector.name === "valid")?.expected.purchaser_key;
    expect(isPurchaserKey(valid)).toBe(true);
    expect(isPurchaserKey(`ed25519:${"A".repeat(42)}B`)).toBe(false);
    expect(isPurchaserKey(`ed25519:${"A".repeat(44)}`)).toBe(false);
    expect(isPurchaserKey(undefined)).toBe(false);
  });

  it("refuses a claim that is not shaped as one", async () => {
    await expect(verified({ receipt: {}, claim_url: vectors.claim_url })).resolves.toEqual({ error: "malformed" });
  });
});
