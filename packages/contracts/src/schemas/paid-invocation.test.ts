import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";

import {
  RUNX_CONTRACT_IDS,
  RUNX_LOGICAL_SCHEMAS,
  cancelPaidInvocationRequestV1Schema,
  cancelPaidInvocationResultV1Schema,
  executePaidInvocationRequestV1Schema,
  executePaidInvocationResultV1Schema,
  getPaidInvocationRequestV1Schema,
  getPaidInvocationResultV1Schema,
  offerRevisionRefV1Schema,
  paidInvocationPaymentChallengeSchema,
  paidSkillListingV1Schema,
  paidInvocationV1Schema,
  paymentIdempotencyBindingSchema,
  principalReferenceFromRunxPrincipalId,
  quotePaidInvocationRequestV1Schema,
  quotePaidInvocationResultV1Schema,
  runxContractSchemas,
  runxGeneratedSchemaArtifacts,
  validateCancelPaidInvocationRequestContract,
  validateCancelPaidInvocationResultContract,
  validateExecutePaidInvocationRequestContract,
  validateExecutePaidInvocationResultContract,
  validateGetPaidInvocationRequestContract,
  validateGetPaidInvocationResultContract,
  validateOfferRevisionRefContract,
  validatePaidInvocationContract,
  validatePaidInvocationPaymentChallengeContract,
  validatePaidSkillListingContract,
  validatePaymentIdempotencyBindingContract,
  validateQuotePaidInvocationRequestContract,
  validateQuotePaidInvocationResultContract,
  validateRunxPrincipalId,
  type QuotePaidInvocationResultContract,
} from "../index.js";

interface PaidInvocationManifest {
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

const paidInvocationFixtureRoot = new URL(
  "../../../../fixtures/contracts/paid-invocation/",
  import.meta.url,
);
const manifest = JSON.parse(
  readFileSync(new URL("manifest.json", paidInvocationFixtureRoot), "utf8"),
) as PaidInvocationManifest;
const principalFixture = JSON.parse(
  readFileSync(
    new URL("../../../../fixtures/contracts/principal-id/principal-id-v1.vectors.json", import.meta.url),
    "utf8",
  ),
) as PrincipalIdFixture;

const validatorsBySchemaId = {
  "runx.marketplace.paid_skill_listing.v1": validatePaidSkillListingContract,
  "runx.payment.quote_paid_invocation.request.v1": validateQuotePaidInvocationRequestContract,
  "runx.payment.quote_paid_invocation.result.v1": validateQuotePaidInvocationResultContract,
  "runx.payment.execute_paid_invocation.request.v1": validateExecutePaidInvocationRequestContract,
  "runx.payment.execute_paid_invocation.result.v1": validateExecutePaidInvocationResultContract,
  "runx.payment.get_paid_invocation.request.v1": validateGetPaidInvocationRequestContract,
  "runx.payment.get_paid_invocation.result.v1": validateGetPaidInvocationResultContract,
  "runx.payment.cancel_paid_invocation.request.v1": validateCancelPaidInvocationRequestContract,
  "runx.payment.cancel_paid_invocation.result.v1": validateCancelPaidInvocationResultContract,
} as const;

function fixturePayload(file: string): unknown {
  const document = JSON.parse(
    readFileSync(new URL(file, paidInvocationFixtureRoot), "utf8"),
  ) as { readonly payload: unknown };
  return document.payload;
}

function validatorForSchemaId(schemaId: string): (value: unknown) => unknown {
  if (!Object.hasOwn(validatorsBySchemaId, schemaId)) {
    throw new Error(`No paid-invocation validator is registered for ${schemaId}.`);
  }
  return validatorsBySchemaId[schemaId as keyof typeof validatorsBySchemaId];
}

function quoteResultMarker(result: QuotePaidInvocationResultContract): string {
  return result.status === "admitted"
    ? result.value.invocation.invocation_id
    : result.code;
}

describe("paid-invocation V1 contract facade", () => {
  it("binds every top-level schema to the exact Rust-generated artifact", () => {
    const bindings = [
      [paidSkillListingV1Schema, "paid-skill-listing.schema.json", "paidSkillListing"],
      [paidInvocationV1Schema, "paid-invocation.schema.json", "paidInvocation"],
      [offerRevisionRefV1Schema, "offer-revision-ref.schema.json", "offerRevisionRef"],
      [quotePaidInvocationRequestV1Schema, "quote-paid-invocation-request.schema.json", "quotePaidInvocationRequest"],
      [quotePaidInvocationResultV1Schema, "quote-paid-invocation-result.schema.json", "quotePaidInvocationResult"],
      [executePaidInvocationRequestV1Schema, "execute-paid-invocation-request.schema.json", "executePaidInvocationRequest"],
      [executePaidInvocationResultV1Schema, "execute-paid-invocation-result.schema.json", "executePaidInvocationResult"],
      [getPaidInvocationRequestV1Schema, "get-paid-invocation-request.schema.json", "getPaidInvocationRequest"],
      [getPaidInvocationResultV1Schema, "get-paid-invocation-result.schema.json", "getPaidInvocationResult"],
      [cancelPaidInvocationRequestV1Schema, "cancel-paid-invocation-request.schema.json", "cancelPaidInvocationRequest"],
      [cancelPaidInvocationResultV1Schema, "cancel-paid-invocation-result.schema.json", "cancelPaidInvocationResult"],
    ] as const;

    for (const [schema, file, registryKey] of bindings) {
      expect(schema).toBe(runxGeneratedSchemaArtifacts[file]);
      expect(schema).toBe(runxContractSchemas[registryKey]);
      expect(schema.$id).toBe(RUNX_CONTRACT_IDS[registryKey]);
      expect(schema["x-runx-schema"]).toBe(RUNX_LOGICAL_SCHEMAS[registryKey]);
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
      package_digest: digest("c"),
      vendor_ref: { type: "principal", uri: "runx:principal:acme" },
      offers: { transcribe: {
        offer_revision: {
          offer_id: "acme/transcribe#transcribe",
          revision: "1.0.0",
          revision_digest: digest("b"),
          input_schema_digest: digest("d"),
          output_schema_digest: digest("e"),
        },
        amount_minor: 125,
        currency: "USD",
        accepted_settlement_families: ["x402", "stripe-spt"],
      } },
    } as const;
    expect(validatePaidSkillListingContract(listing)).toBe(listing);
    expect(() => validatePaidSkillListingContract({
      ...listing,
      stripe_price_id: "price_private",
    })).toThrow();
  });

  it("binds nested values through exact generated fragments without new ids", () => {
    expect(paymentIdempotencyBindingSchema)
      .toBe((quotePaidInvocationRequestV1Schema.properties as Record<string, unknown>).idempotency);
    expect(paidInvocationPaymentChallengeSchema)
      .toBe((quotePaidInvocationResultV1Schema.$defs as Record<string, unknown>).PaidInvocationPaymentChallenge);
    expect(paymentIdempotencyBindingSchema.$id).toBeUndefined();
    expect(paidInvocationPaymentChallengeSchema.$id).toBeUndefined();

    const request = fixturePayload("quote-independent-purchase.json") as {
      readonly idempotency: unknown;
    };
    const result = fixturePayload("quote-direct-admission.json") as {
      readonly status: string;
      readonly value: { readonly challenge: unknown };
    };
    expect(validatePaymentIdempotencyBindingContract(request.idempotency)).toBe(request.idempotency);
    expect(validatePaidInvocationPaymentChallengeContract(result.value.challenge))
      .toBe(result.value.challenge);
    expect(() => validatePaymentIdempotencyBindingContract({
      ...(request.idempotency as object),
      unknown_key: "not-accepted",
    })).toThrow();
  });

  it("binds aggregate and nested-record validators to their generated schemas", () => {
    const result = fixturePayload("quote-direct-admission.json") as {
      readonly value: { readonly invocation: unknown };
    };
    const request = fixturePayload("quote-independent-purchase.json") as {
      readonly offer_revision: unknown;
    };

    expect(validatePaidInvocationContract(result.value.invocation))
      .toBe(result.value.invocation);
    expect(validateOfferRevisionRefContract(request.offer_revision))
      .toBe(request.offer_revision);
    expect(() => validatePaidInvocationContract({
      ...(result.value.invocation as object),
      unknown_state: "not-accepted",
    })).toThrow();
    expect(() => validateOfferRevisionRefContract({
      ...(request.offer_revision as object),
      unknown_revision: "not-accepted",
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
    const admitted = validateQuotePaidInvocationResultContract(
      fixturePayload("quote-direct-admission.json"),
    );
    const refused = validateQuotePaidInvocationResultContract(
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

    const request = structuredClone(
      fixturePayload("quote-independent-purchase.json"),
    ) as { vendor_ref: { uri: string } };
    request.vendor_ref.uri = "did:example:external-vendor";
    expect(validateQuotePaidInvocationRequestContract(request)).toBe(request);
  });
});
