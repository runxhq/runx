import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";

import {
  offerVersionBindingV1Schema,
  purchasePaymentChallengeSchema,
  paidSkillListingV1Schema,
  purchaseV1Schema,
  paymentIdempotencyBindingSchema,
  principalReferenceFromRunxPrincipalId,
  quotePurchaseResultV1Schema,
  runxContractSchemas,
  runxGeneratedSchemaArtifacts,
  validatePurchaseReadContract,
  validateOfferVersionBindingContract,
  validatePurchaseContract,
  validatePurchasePaymentChallengeContract,
  validatePaidSkillListingContract,
  validatePaymentIdempotencyBindingContract,
  validateQuotePurchaseResultContract,
  validateRunxPrincipalId,
  type QuotePurchaseResultContract,
} from "../index.js";

interface PurchaseManifest {
  readonly vectors: readonly {
    readonly expectation: "valid" | "invalid";
    readonly file: string;
    readonly schema_id: string;
  }[];
}

interface PrincipalIdFixture {
  readonly schema: "runx.principal_id.fixtures.v1";
  readonly grammar: string;
  readonly cases: readonly {
    readonly name: string;
    readonly input: string;
    readonly expectation: "valid" | "invalid";
    readonly reference: { readonly type: "principal"; readonly uri: string } | null;
  }[];
}

const purchaseFixtureRoot = new URL(
  "../../../../fixtures/contracts/purchase/",
  import.meta.url,
);
const manifest = JSON.parse(
  readFileSync(new URL("manifest.json", purchaseFixtureRoot), "utf8"),
) as PurchaseManifest;
const principalFixture = JSON.parse(
  readFileSync(
    new URL("../../../../fixtures/contracts/principal-id/principal-id-v1.vectors.json", import.meta.url),
    "utf8",
  ),
) as PrincipalIdFixture;

const validatorsBySchemaId = {
  "runx.marketplace.paid_skill_listing.v1": validatePaidSkillListingContract,
  "runx.payment.quote_purchase.result.v1": validateQuotePurchaseResultContract,
  "runx.payment.purchase_read.v1": validatePurchaseReadContract,
} as const;

function fixturePayload(file: string): unknown {
  const document = JSON.parse(
    readFileSync(new URL(file, purchaseFixtureRoot), "utf8"),
  ) as { readonly payload: unknown };
  return document.payload;
}

function validatorForSchemaId(schemaId: string): (value: unknown) => unknown {
  if (!Object.hasOwn(validatorsBySchemaId, schemaId)) {
    throw new Error(`No purchase validator is registered for ${schemaId}.`);
  }
  return validatorsBySchemaId[schemaId as keyof typeof validatorsBySchemaId];
}

function quoteResultMarker(result: QuotePurchaseResultContract): string {
  return result.status === "admitted"
    ? result.value.purchase.purchase_id
    : result.code;
}

describe("purchase V1 contract facade", () => {
  it("binds every top-level schema to the exact Rust-generated artifact", () => {
    const bindings = [
      [paidSkillListingV1Schema, "paid-skill-listing.schema.json", "paidSkillListing"],
      [purchaseV1Schema, "purchase.schema.json", "purchase"],
      [offerVersionBindingV1Schema, "offer-version-binding.schema.json", "offerVersionBinding"],
      [quotePurchaseResultV1Schema, "quote-purchase-result.schema.json", "quotePurchaseResult"],
    ] as const;

    for (const [schema, file, registryKey] of bindings) {
      expect(schema).toBe(runxGeneratedSchemaArtifacts[file]);
      expect(schema).toBe(runxContractSchemas[registryKey]);
    }
  });

  it("keeps marketplace listings rail-neutral while advertising compatible families", () => {
    const digest = (character: string): `sha256:${string}` =>
      `sha256:${character.repeat(64)}`;
    const listing = {
      skill_id: "acme/transcribe",
      version: "1.0.0",
      skill_digest: digest("a"),
      profile_digest: digest("b"),
      bundle_digest: digest("c"),
      seller_ref: { type: "principal", uri: "runx:principal:acme" },
      offers: { transcribe: {
        offer_version: {
          offer_id: "acme/transcribe#transcribe",
          version: "1.0.0",
          profile_digest: digest("b"),
          input_schema_digest: digest("d"),
          output_schema_digest: digest("e"),
        },
        price: { basis: "fixed", amount_minor: 125 },
        currency: "USD",
        accepts: ["x402", "stripe-spt"],
      } },
    } as const;
    expect(validatePaidSkillListingContract(listing)).toBe(listing);
    expect(() => validatePaidSkillListingContract({
      ...listing,
      stripe_price_id: "price_private",
    })).toThrow();
    // The invariants a schema cannot state still refuse the listing.
    expect(() => validatePaidSkillListingContract({
      ...listing,
      offers: { transcribe: {
        ...listing.offers.transcribe,
        price: { basis: "by_input", cases: [
          { inputs: { tier: "basic" }, amount_minor: 1 },
          { inputs: { tier: "basic" }, amount_minor: 2 },
        ] },
      } },
    })).toThrow(/has one case/u);
    expect(() => validatePaidSkillListingContract({
      ...listing,
      offers: { transcribe: {
        ...listing.offers.transcribe,
        requires_lease: { connector: "acme/inbox", credential: "lease", target: { $result: "/inbox_id" } },
      } },
    })).toThrow(/method's inputs alone/u);
  });

  it("binds nested values through their one generated definition without new ids", () => {
    const definitions = runxGeneratedSchemaArtifacts["definitions.schema.json"].$defs as Record<string, unknown>;
    expect(paymentIdempotencyBindingSchema).toBe(definitions.PaymentIdempotencyBinding);
    expect(purchasePaymentChallengeSchema).toBe(definitions.PurchasePaymentChallenge);
    expect((purchaseV1Schema.properties as Record<string, unknown>).idempotency)
      .toEqual({ $ref: `${runxGeneratedSchemaArtifacts["definitions.schema.json"].$id}#/$defs/PaymentIdempotencyBinding` });
    expect(paymentIdempotencyBindingSchema.$id).toBeUndefined();
    expect(purchasePaymentChallengeSchema.$id).toBeUndefined();

    const result = fixturePayload("quote-direct-admission.json") as {
      readonly status: string;
      readonly value: {
        readonly payment_options: readonly unknown[];
        readonly purchase: { readonly idempotency: unknown };
      };
    };
    const request = result.value.purchase;
    expect(validatePaymentIdempotencyBindingContract(request.idempotency)).toBe(request.idempotency);
    expect(validatePurchasePaymentChallengeContract(result.value.payment_options[0]))
      .toBe(result.value.payment_options[0]);
    expect(() => validatePaymentIdempotencyBindingContract({
      ...(request.idempotency as object),
      unknown_key: "not-accepted",
    })).toThrow();
  });

  it("binds aggregate and nested-record validators to their generated schemas", () => {
    const result = fixturePayload("quote-direct-admission.json") as {
      readonly value: { readonly purchase: unknown };
    };
    const request = result.value.purchase as { readonly offer_version: unknown };

    expect(validatePurchaseContract(result.value.purchase))
      .toBe(result.value.purchase);
    expect(validateOfferVersionBindingContract(request.offer_version))
      .toBe(request.offer_version);
    expect(() => validatePurchaseContract({
      ...(result.value.purchase as object),
      unknown_state: "not-accepted",
    })).toThrow();
    expect(() => validateOfferVersionBindingContract({
      ...(request.offer_version as object),
      unknown_version: "not-accepted",
    })).toThrow();
  });

  it.each(manifest.vectors.map((vector) => [vector.file, vector] as const))(
    "validates the canonical expectation for %s",
    (_file, vector) => {
      const validate = validatorForSchemaId(vector.schema_id);
      const payload = fixturePayload(vector.file);
      if (vector.expectation === "valid") {
        expect(validate(payload)).toBe(payload);
      } else {
        expect(() => validate(payload)).toThrow();
      }
    },
  );

  it("preserves discriminated admitted and refused result branches", () => {
    const admitted = validateQuotePurchaseResultContract(
      fixturePayload("quote-direct-admission.json"),
    );
    const refused = validateQuotePurchaseResultContract(
      fixturePayload("quote-replay-conflict.json"),
    );

    expect(quoteResultMarker(admitted)).toBe("paid_direct");
    expect(quoteResultMarker(refused)).toBe("replay_conflict");
  });

  it("keeps hosted principal construction strict without narrowing generic principal URIs", () => {
    expect(principalFixture.schema).toBe("runx.principal_id.fixtures.v1");
    expect(principalFixture.grammar).toBe("^[A-Za-z0-9][A-Za-z0-9._:-]{0,255}$");

    for (const testCase of principalFixture.cases) {
      if (testCase.expectation === "valid") {
        expect(validateRunxPrincipalId(testCase.input)).toBe(testCase.input);
        expect(principalReferenceFromRunxPrincipalId(testCase.input)).toEqual(testCase.reference);
      } else {
        expect(() => validateRunxPrincipalId(testCase.input)).toThrow();
        expect(() => principalReferenceFromRunxPrincipalId(testCase.input)).toThrow();
      }
    }

    const purchase = structuredClone(
      (fixturePayload("quote-direct-admission.json") as { value: { purchase: unknown } }).value.purchase,
    ) as { seller_ref: { uri: string } };
    purchase.seller_ref.uri = "did:example:external-seller";
    expect(validatePurchaseContract(purchase)).toBe(purchase);
  });
});
