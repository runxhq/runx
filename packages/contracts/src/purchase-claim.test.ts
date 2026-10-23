import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";

import {
  isBuyerKey,
  purchaseClaimMessage,
  purchaseReadRequestMessage,
  validatePurchaseClaimContract,
  verifyBuyerSignature,
} from "./purchase-claim.js";

interface SignatureVectors {
  readonly buyer_key: string;
  readonly cases: readonly {
    readonly name: string;
    readonly kind: "claim" | "read";
    readonly request: Readonly<Record<string, string>>;
    readonly verifies: boolean;
  }[];
}

const vectors = JSON.parse(readFileSync(
  new URL("../../../fixtures/contracts/purchase-claim/purchase-claim-v1.vectors.json", import.meta.url),
  "utf8",
)) as SignatureVectors;

describe("buyer signatures Rust made verify as Rust expects", () => {
  for (const vector of vectors.cases) {
    it(vector.name, async () => {
      const { request } = vector;
      const message = vector.kind === "claim"
        ? purchaseClaimMessage({
          claimUrl: validatePurchaseClaimContract(request).claim_url,
          issuedAt: request.issued_at!,
          receiptDigest: request.receipt_digest!,
        })
        : purchaseReadRequestMessage({ method: request.method!, url: request.url!, issuedAt: request.issued_at! });
      await expect(verifyBuyerSignature({
        buyerKey: vectors.buyer_key,
        signature: request.signature!,
        message,
      })).resolves.toBe(vector.verifies);
    });
  }

  it("spells a buyer key one way, as Rust does", () => {
    expect(isBuyerKey(vectors.buyer_key)).toBe(true);
    expect(isBuyerKey(`ed25519:${"A".repeat(42)}B`)).toBe(false);
    expect(isBuyerKey(`ed25519:${"A".repeat(44)}`)).toBe(false);
    expect(isBuyerKey(undefined)).toBe(false);
  });

  it("refuses a claim that is not shaped as one", () => {
    const claim = vectors.cases.find((vector) => vector.name === "claim")!.request;
    expect(() => validatePurchaseClaimContract({ ...claim, receipt: {} })).toThrow();
    expect(() => validatePurchaseClaimContract({ ...claim, receipt_digest: "sha256:short" })).toThrow();
  });
});
