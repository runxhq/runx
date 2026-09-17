import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";

import {
  RUNX_X402_INVOCATION_EXTENSION_KEY,
  RUNX_X402_INVOCATION_SCHEMA_REFERENCE,
  X402_BAZAAR_EXTENSION_KEY,
  X402_PAYMENT_REQUIRED_HEADER,
  X402PresentationError,
  X402_SCHEMA_IDS,
  X402_UPSTREAM_COMMIT,
  X402_UPSTREAM_PACKAGE,
  X402_UPSTREAM_PACKAGE_VERSION,
  assembleExternalX402PaymentRequired,
  assembleX402PaymentRequired,
  bindX402PaymentRequiredChallenge,
  decodeX402PaymentRequiredHeader,
  decodeX402PaymentResponseHeader,
  decodeX402PaymentSignatureHeader,
  declareExternalX402JsonPostDiscovery,
  encodeX402PaymentRequiredHeader,
  encodeX402PaymentResponseHeader,
  parseRunxX402InvocationDeclaration,
  encodeX402PaymentSignatureHeader,
  runxGeneratedSchemaArtifacts,
  runxX402InvocationExtensionInfoV1Schema,
  validateRunxX402InvocationExtensionInfoContract,
  validateX402PaymentPayloadContract,
  validateX402PaymentRequiredContract,
  validateX402PaymentRetry,
  validateX402SettleResponseContract,
  x402DiscoveryHttpProjection,
  x402ExternalDiscoveryHttpProjection,
  x402PaymentPayloadV2Schema,
  x402PaymentRequiredFromChallenge,
  x402PaymentRequiredV2Schema,
  x402PaymentRequirementsV2Schema,
  x402ResourceInfoV2Schema,
  x402SettleResponseV2Schema,
  type RunxX402ExtensionInfoContract,
  type RunxX402DiscoveryExtensionInfoContract,
  type X402PaymentPayloadContract,
} from "./index.js";

interface X402Fixture {
  readonly expectation: "valid" | "invalid";
  readonly header: string | null;
  readonly kind:
    | "payment-required"
    | "payment-payload"
    | "settle-response"
    | "runx_invocation_extension";
  readonly payload: unknown;
  readonly schema_id: string;
}

const fixtureRoot = new URL("../../../fixtures/contracts/x402-v2/", import.meta.url);
const upstreamPin = JSON.parse(
  readFileSync(new URL("upstream-pin.json", fixtureRoot), "utf8"),
) as { readonly package: { readonly name: string; readonly version: string }; readonly revision: string };

function fixture(file: string): X402Fixture {
  return JSON.parse(readFileSync(new URL(file, fixtureRoot), "utf8")) as X402Fixture;
}

function presentationErrorCode(operation: () => unknown): string | undefined {
  try {
    operation();
    return undefined;
  } catch (error) {
    return error instanceof X402PresentationError ? error.code : undefined;
  }
}

describe("x402 v2 TypeScript facade", () => {
  it("binds external readers and the strict Runx extension to generated Rust schemas", () => {
    const bindings = [
      [x402ResourceInfoV2Schema, "x402-v2-resource-info.schema.json", X402_SCHEMA_IDS.resourceInfo],
      [x402PaymentRequirementsV2Schema, "x402-v2-payment-requirements.schema.json", X402_SCHEMA_IDS.paymentRequirements],
      [x402PaymentRequiredV2Schema, "x402-v2-payment-required.schema.json", X402_SCHEMA_IDS.paymentRequired],
      [x402PaymentPayloadV2Schema, "x402-v2-payment-payload.schema.json", X402_SCHEMA_IDS.paymentPayload],
      [x402SettleResponseV2Schema, "x402-v2-settle-response.schema.json", X402_SCHEMA_IDS.settleResponse],
      [runxX402InvocationExtensionInfoV1Schema, "runx-x402-invocation-extension-v1.schema.json", X402_SCHEMA_IDS.runxInvocationExtension],
    ] as const;

    for (const [schema, file, id] of bindings) {
      expect(schema).toBe(runxGeneratedSchemaArtifacts[file]);
      expect(schema.$id).toBe(id);
    }
    expect(X402_UPSTREAM_PACKAGE).toBe(upstreamPin.package.name);
    expect(X402_UPSTREAM_PACKAGE_VERSION).toBe(upstreamPin.package.version);
    expect(X402_UPSTREAM_COMMIT).toBe(upstreamPin.revision);
  });

  it("round-trips the pinned official HTTP headers byte for byte", () => {
    const required = fixture("official-payment-required.json");
    const signature = fixture("official-payment-payload.json");
    const response = fixture("official-settle-success.json");
    expect(required.header).not.toBeNull();
    expect(signature.header).not.toBeNull();
    expect(response.header).not.toBeNull();

    expect(encodeX402PaymentRequiredHeader(
      decodeX402PaymentRequiredHeader(required.header!),
    )).toBe(required.header);
    expect(encodeX402PaymentSignatureHeader(
      decodeX402PaymentSignatureHeader(signature.header!),
    )).toBe(signature.header);
    expect(encodeX402PaymentResponseHeader(
      decodeX402PaymentResponseHeader(response.header!),
    )).toBe(response.header);
  });

  it("validates every pinned vector through its single generated facade", () => {
    const manifest = JSON.parse(
      readFileSync(new URL("manifest.json", fixtureRoot), "utf8"),
    ) as { readonly vectors: readonly { readonly file: string }[] };
    const validators = {
      "payment-required": validateX402PaymentRequiredContract,
      "payment-payload": validateX402PaymentPayloadContract,
      "settle-response": validateX402SettleResponseContract,
      runx_invocation_extension: validateRunxX402InvocationExtensionInfoContract,
    } as const;

    for (const vector of manifest.vectors) {
      const item = fixture(vector.file);
      const validate = validators[item.kind];
      if (item.expectation === "valid") {
        expect(validate(item.payload)).toBe(item.payload);
      } else {
        expect(() => validate(item.payload)).toThrow();
      }
    }
  });

  it("assembles the reserved Runx binding and rejects changed retry commitments", () => {
    const officialRequired = validateX402PaymentRequiredContract(
      fixture("official-payment-required.json").payload,
    );
    const invocation = fixture("runx-invocation-extension.json")
      .payload as RunxX402ExtensionInfoContract;
    const challenge = assembleX402PaymentRequired({
      resource: officialRequired.resource,
      accepts: officialRequired.accepts,
      invocation,
      extensions: { "vendor.example": { future: true } },
    });
    expect(challenge.extensions?.["vendor.example"]).toEqual({ future: true });
    expect(challenge.extensions?.[RUNX_X402_INVOCATION_EXTENSION_KEY]).toEqual({
      info: invocation,
      schema: { $ref: "https://schemas.runx.ai/runx/x402/invocation-extension/v1.json" },
    });
    expect(RUNX_X402_INVOCATION_SCHEMA_REFERENCE).toEqual({
      $ref: runxX402InvocationExtensionInfoV1Schema.$id,
    });

    const retry: X402PaymentPayloadContract = {
      x402Version: 2,
      resource: challenge.resource,
      accepted: challenge.accepts[0]!,
      payload: { signature: "opaque" },
      extensions: challenge.extensions,
    };
    expect(validateX402PaymentRetry(challenge, retry)).toEqual({
      requirementIndex: 0,
      invocation,
    });

    // A challenge assembled before the reference form carried the whole
    // document; an echo of it names the same schema and still validates.
    const inlineEcho: X402PaymentPayloadContract = {
      ...structuredClone(retry),
      extensions: {
        [RUNX_X402_INVOCATION_EXTENSION_KEY]: {
          info: invocation,
          schema: runxX402InvocationExtensionInfoV1Schema,
        },
      },
    };
    expect(validateX402PaymentRetry(challenge, inlineEcho)).toEqual({
      requirementIndex: 0,
      invocation,
    });
    expect(parseRunxX402InvocationDeclaration(inlineEcho.extensions)).toEqual({
      info: invocation,
      schema: RUNX_X402_INVOCATION_SCHEMA_REFERENCE,
    });

    const foreignReference: X402PaymentPayloadContract = {
      ...structuredClone(retry),
      extensions: {
        [RUNX_X402_INVOCATION_EXTENSION_KEY]: {
          info: invocation,
          schema: { $ref: "https://schemas.runx.ai/runx/x402/invocation-extension/v2.json" },
        },
      },
    };
    expect(presentationErrorCode(() => validateX402PaymentRetry(challenge, foreignReference)))
      .toBe("runx_invocation_schema_mismatch");

    const changedAmount = structuredClone(retry);
    (changedAmount.accepted as { amount: string }).amount = "10001";
    expect(presentationErrorCode(() => validateX402PaymentRetry(challenge, changedAmount)))
      .toBe("requirement_mismatch");

    const changedResource = structuredClone(retry);
    (changedResource.resource as { url: string }).url = "https://api.example.com/other";
    expect(presentationErrorCode(() => validateX402PaymentRetry(challenge, changedResource)))
      .toBe("resource_mismatch");

    expect(() => assembleX402PaymentRequired({
      resource: challenge.resource,
      accepts: challenge.accepts,
      invocation,
      extensions: { [RUNX_X402_INVOCATION_EXTENSION_KEY]: { attacker: true } },
    })).toThrow("reserved");
  });

  it("projects one exact Bazaar JSON POST declaration without product semantics", () => {
    expect(declareExternalX402JsonPostDiscovery({
      example: { request_id: "example" },
      inputSchema: { type: "object", required: ["request_id"] },
      outputExample: { status: "accepted" },
      outputSchema: { type: "object", required: ["status"] },
    })).toMatchObject({
      info: {
        input: { type: "http", method: "POST", bodyType: "json" },
        output: { type: "json" },
      },
      schema: {
        properties: {
          input: {
            properties: { method: { enum: ["POST"] } },
            required: ["type", "method", "bodyType", "body"],
          },
        },
      },
    });
  });

  it("rebases root-local JSON Schema references after Bazaar embedding", () => {
    const declaration = declareExternalX402JsonPostDiscovery({
      example: { request_id: "example" },
      inputSchema: {
        type: "object",
        properties: { request_id: { $ref: "#/$defs/RequestId" } },
        required: ["request_id"],
        $defs: { RequestId: { type: "string", minLength: 1 } },
      },
      outputExample: { status: "accepted" },
      outputSchema: {
        type: "object",
        properties: { status: { $ref: "#/$defs/Status" } },
        required: ["status"],
        $defs: { Status: { type: "string", const: "accepted" } },
      },
    }) as {
      schema: {
        properties: {
          input: { properties: { body: { properties: { request_id: { $ref: string } } } } };
          output: { properties: { example: { properties: { status: { $ref: string } } } } };
        };
      };
    };

    expect(
      declaration.schema.properties.input.properties.body.properties.request_id.$ref,
    ).toBe("#/properties/input/properties/body/$defs/RequestId");
    expect(
      declaration.schema.properties.output.properties.example.properties.status.$ref,
    ).toBe("#/properties/output/properties/example/$defs/Status");
  });

  it("binds and recovers a rail-neutral challenge while detecting tampering", () => {
    const official = validateX402PaymentRequiredContract(
      fixture("official-payment-required.json").payload,
    );
    const challenge = bindX402PaymentRequiredChallenge(
      official,
      { type: "receipt", uri: "runx:receipt:quote-1" },
      "2026-08-24T10:00:00Z",
    );
    expect(x402PaymentRequiredFromChallenge(challenge)).toBe(official);

    expect(presentationErrorCode(() => x402PaymentRequiredFromChallenge({
      ...challenge,
      protocol_version: "3",
    }))).toBe("challenge_kind_mismatch");
    expect(presentationErrorCode(() => x402PaymentRequiredFromChallenge({
      ...challenge,
      payload: { x402Version: 2 },
    }))).toBe("challenge_digest_mismatch");
  });

  it("projects a deterministic, inert x402 v2 discovery challenge", () => {
    const official = validateX402PaymentRequiredContract(
      fixture("official-payment-required.json").payload,
    );
    const discovery = fixture("runx-discovery-extension.json")
      .payload as RunxX402DiscoveryExtensionInfoContract;
    const bazaar = {
      info: {
        input: {
          type: "object",
          properties: { document: { type: "string" } },
          required: ["document"],
        },
        output: {
          type: "object",
          properties: { text: { type: "string" } },
          required: ["text"],
        },
        resource: {
          method: "POST",
          routeTemplate: "/v1/skills/vendor/documents/run",
        },
      },
      schema: { type: "object", additionalProperties: true },
    } as const;
    const descriptor = {
      resource: official.resource,
      accepts: official.accepts,
      offerRevision: discovery.offer_revision,
      packageDigest: discovery.package_digest,
      extensions: { [X402_BAZAAR_EXTENSION_KEY]: bazaar },
    } as const;

    const projection = x402DiscoveryHttpProjection(descriptor);
    expect(x402DiscoveryHttpProjection(descriptor)).toEqual(projection);
    expect(projection.status).toBe(402);
    expect(decodeX402PaymentRequiredHeader(
      projection.headers["PAYMENT-REQUIRED"],
    )).toEqual(projection.body);
    expect(projection.body.x402Version).toBe(2);
    expect(projection.body.accepts).toEqual(official.accepts);
    expect(projection.body.extensions?.[X402_BAZAAR_EXTENSION_KEY]).toEqual(bazaar);
    expect(projection.body.extensions?.[RUNX_X402_INVOCATION_EXTENSION_KEY]).toEqual({
      info: discovery,
      schema: RUNX_X402_INVOCATION_SCHEMA_REFERENCE,
    });
  });

  it("projects an external vendor resource without inventing Runx invocation identity", () => {
    const official = validateX402PaymentRequiredContract(
      fixture("official-payment-required.json").payload,
    );
    const bazaar = {
      info: { input: { method: "POST" }, output: { type: "object" } },
      schema: { type: "object" },
    } as const;
    const projection = x402ExternalDiscoveryHttpProjection({
      resource: official.resource,
      accepts: official.accepts,
      extensions: { [X402_BAZAAR_EXTENSION_KEY]: bazaar },
    });

    expect(projection.body).toEqual(assembleExternalX402PaymentRequired({
      resource: official.resource,
      accepts: official.accepts,
      extensions: { [X402_BAZAAR_EXTENSION_KEY]: bazaar },
    }));
    expect(projection.body.extensions?.[RUNX_X402_INVOCATION_EXTENSION_KEY]).toBeUndefined();
    expect(decodeX402PaymentRequiredHeader(
      projection.headers[X402_PAYMENT_REQUIRED_HEADER],
    )).toEqual(projection.body);
  });

  it("refuses absent or obsolete Bazaar discovery declarations", () => {
    const official = validateX402PaymentRequiredContract(
      fixture("official-payment-required.json").payload,
    );
    const discovery = fixture("runx-discovery-extension.json")
      .payload as RunxX402DiscoveryExtensionInfoContract;
    const descriptor = {
      resource: official.resource,
      accepts: official.accepts,
      offerRevision: discovery.offer_revision,
      packageDigest: discovery.package_digest,
    } as const;

    expect(presentationErrorCode(() => x402DiscoveryHttpProjection({
      ...descriptor,
      extensions: {},
    }))).toBe("missing_discovery_extension");
    expect(presentationErrorCode(() => x402DiscoveryHttpProjection({
      ...descriptor,
      extensions: {
        [X402_BAZAAR_EXTENSION_KEY]: {
          info: { discoverable: true },
          schema: { type: "object" },
        },
      },
    }))).toBe("invalid_discovery_extension");
  });

  it("uses bounded standard base64 and never echoes payment material in errors", () => {
    const secret = "do-not-echo";
    for (const encoded of ["not a signature!", "eyJwYXlsb2FkIjoi" + secret]) {
      try {
        decodeX402PaymentSignatureHeader(encoded);
        throw new Error("expected decoding to fail");
      } catch (error) {
        expect(String(error)).not.toContain(secret);
      }
    }
    expect(presentationErrorCode(() => decodeX402PaymentSignatureHeader("-_")))
      .toBe("invalid_base64");
  });
});
