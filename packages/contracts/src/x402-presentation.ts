import { TextDecoder } from "node:util";

import { canonicalJsonStringify, sha256Prefixed } from "./canonical-json.js";
import { asUnknownRecord } from "./internal.js";
import {
  validatePaidInvocationPaymentChallengeContract,
  type PaidInvocationPaymentChallengeContract,
} from "./schemas/paid-invocation.js";
import type { ReferenceContract } from "./schemas/spine.js";
import {
  X402_SCHEMA_IDS,
  runxX402InvocationExtensionInfoV1Schema,
  validateRunxX402InvocationExtensionInfoContract,
  validateX402PaymentPayloadContract,
  validateX402PaymentRequiredContract,
  validateX402SettleResponseContract,
  type RunxX402ExtensionInfoContract,
  type RunxX402DiscoveryExtensionInfoContract,
  type RunxX402InvocationExtensionContract,
  type X402PaymentPayloadContract,
  type X402PaymentRequiredContract,
  type X402PaymentRequirementsContract,
  type X402ResourceInfoContract,
  type X402SettleResponseContract,
} from "./schemas/x402.js";

export const X402_PAYMENT_REQUIRED_HEADER = "PAYMENT-REQUIRED" as const;
export const X402_PAYMENT_SIGNATURE_HEADER = "PAYMENT-SIGNATURE" as const;
export const X402_PAYMENT_RESPONSE_HEADER = "PAYMENT-RESPONSE" as const;
export const RUNX_X402_INVOCATION_EXTENSION_KEY = "runx.invocation" as const;
export const X402_BAZAAR_EXTENSION_KEY = "bazaar" as const;
export const X402_JSON_MEDIA_TYPE = "application/json" as const;
export const MAX_X402_HEADER_BYTES = 65_536 as const;
export const MAX_X402_DECODED_BYTES = 49_152 as const;

export type X402PresentationErrorCode =
  | "header_too_large"
  | "invalid_base64"
  | "invalid_payload"
  | "encoding_failed"
  | "reserved_extension"
  | "missing_discovery_extension"
  | "invalid_discovery_extension"
  | "missing_runx_invocation"
  | "runx_invocation_schema_mismatch"
  | "resource_mismatch"
  | "requirement_mismatch"
  | "runx_invocation_mismatch"
  | "challenge_kind_mismatch"
  | "challenge_digest_mismatch";

const ERROR_MESSAGES: Readonly<Record<X402PresentationErrorCode, string>> = {
  header_too_large: "x402 header exceeds the configured bound",
  invalid_base64: "x402 header is not standard base64",
  invalid_payload: "x402 header JSON is malformed or violates its contract",
  encoding_failed: "x402 value could not be encoded",
  reserved_extension: "runx.invocation is reserved and cannot be supplied by a vendor",
  missing_discovery_extension: "x402 discovery requires a bazaar extension",
  invalid_discovery_extension: "x402 bazaar extension must use the v2 info/schema declaration",
  missing_runx_invocation: "runx.invocation is absent",
  runx_invocation_schema_mismatch: "runx.invocation does not match its published v1 schema",
  resource_mismatch: "the retry resource does not match the challenge",
  requirement_mismatch: "the retry selected requirements are not an exact offered requirement",
  runx_invocation_mismatch: "the retry runx.invocation declaration changed",
  challenge_kind_mismatch: "the rail-neutral challenge is not an x402 v2 JSON challenge",
  challenge_digest_mismatch: "the rail-neutral challenge payload digest does not match",
};

/** Redacted by construction: payment material is never interpolated into errors. */
export class X402PresentationError extends Error {
  constructor(readonly code: X402PresentationErrorCode) {
    super(ERROR_MESSAGES[code]);
    this.name = "X402PresentationError";
  }
}

export type ValidatedX402Retry = Readonly<{
  requirementIndex: number;
  invocation: RunxX402ExtensionInfoContract;
}>;

export type X402DiscoveryDescriptor = Readonly<{
  resource: X402ResourceInfoContract;
  accepts: readonly X402PaymentRequirementsContract[];
  offerRevision: RunxX402DiscoveryExtensionInfoContract["offer_revision"];
  packageDigest: RunxX402DiscoveryExtensionInfoContract["package_digest"];
  extensions: Readonly<Record<string, unknown>>;
}>;

export type X402DiscoveryHttpProjection = Readonly<{
  status: 402;
  body: X402PaymentRequiredContract;
  headers: Readonly<Record<typeof X402_PAYMENT_REQUIRED_HEADER, string>>;
}>;

export type X402ExternalDiscoveryDescriptor = Readonly<{
  resource: X402ResourceInfoContract;
  accepts: readonly X402PaymentRequirementsContract[];
  extensions: Readonly<Record<string, unknown>>;
  error?: string | null;
}>;

/**
 * Project the x402 Foundation Bazaar v2 declaration for one JSON POST body.
 * Keeping this in the external-protocol adapter prevents product repositories
 * from copying Bazaar schemas or taking a broad runtime dependency on every
 * x402 extension.
 */
export function declareExternalX402JsonPostDiscovery(input: Readonly<{
  example: Readonly<Record<string, unknown>>;
  inputSchema: Readonly<Record<string, unknown>>;
  outputExample: unknown;
  outputSchema: Readonly<Record<string, unknown>>;
}>): Readonly<Record<string, unknown>> {
  const inputSchema = rebaseEmbeddedJsonSchema(
    input.inputSchema,
    "#/properties/input/properties/body",
  );
  const outputSchema = rebaseEmbeddedJsonSchema(
    input.outputSchema,
    "#/properties/output/properties/example",
  );
  return {
    info: {
      input: {
        type: "http",
        method: "POST",
        bodyType: "json",
        body: input.example,
      },
      output: { type: "json", example: input.outputExample },
    },
    schema: {
      $schema: "https://json-schema.org/draft/2020-12/schema",
      type: "object",
      properties: {
        input: {
          type: "object",
          properties: {
            type: { type: "string", const: "http" },
            method: { type: "string", enum: ["POST"] },
            bodyType: { type: "string", enum: ["json", "form-data", "text"] },
            body: inputSchema,
          },
          required: ["type", "method", "bodyType", "body"],
          additionalProperties: false,
        },
        output: {
          type: "object",
          properties: {
            type: { type: "string" },
            example: { type: "object", ...outputSchema },
          },
          required: ["type"],
        },
      },
      required: ["input"],
    },
  };
}

/**
 * Bazaar embeds each supplied JSON Schema below its own discovery schema. A
 * root-local reference such as `#/$defs/Foo` would otherwise resolve against
 * the Bazaar root and silently lose the supplied schema's definitions. Rebase
 * only document-local references; external references remain untouched and
 * the normal Bazaar validator decides whether to admit them.
 */
function rebaseEmbeddedJsonSchema(
  schema: Readonly<Record<string, unknown>>,
  basePointer: string,
): Readonly<Record<string, unknown>> {
  const visit = (value: unknown): unknown => {
    if (Array.isArray(value)) return value.map(visit);
    if (value === null || typeof value !== "object") return value;
    return Object.fromEntries(
      Object.entries(value).map(([key, entry]) => [
        key,
        key === "$ref" && typeof entry === "string" && entry.startsWith("#/")
          ? `${basePointer}${entry.slice(1)}`
          : visit(entry),
      ]),
    );
  };
  return visit(schema) as Readonly<Record<string, unknown>>;
}

/**
 * Assemble a standard external x402 response without asserting any Runx
 * invocation semantics. Product owners supply only their public resource and
 * already-authorized payment requirements; Runx remains the protocol
 * validator and header encoder.
 */
export function assembleExternalX402PaymentRequired(input: Readonly<{
  resource: X402ResourceInfoContract;
  accepts: readonly X402PaymentRequirementsContract[];
  error?: string | null;
  extensions?: Readonly<Record<string, unknown>>;
}>): X402PaymentRequiredContract {
  const value = {
    x402Version: 2,
    ...(input.error === undefined ? {} : { error: input.error }),
    resource: input.resource,
    accepts: input.accepts,
    ...(input.extensions === undefined ? {} : { extensions: input.extensions }),
  } as const;
  return validateX402PaymentRequiredContract(value);
}

/**
 * The schema a `runx.invocation` declaration advertises: the published v1
 * document by its resolvable `$id`. The external `{ info, schema }` form is
 * kept, the five-kilobyte document is not; a challenge has to fit one
 * portable HTTP header next to the vendor's own discovery declaration.
 */
export const RUNX_X402_INVOCATION_SCHEMA_REFERENCE: Readonly<Record<string, unknown>> = Object.freeze({
  $ref: X402_SCHEMA_IDS.runxInvocationExtension,
});

export function assembleX402PaymentRequired(input: Readonly<{
  resource: X402ResourceInfoContract;
  accepts: readonly X402PaymentRequirementsContract[];
  invocation: RunxX402ExtensionInfoContract;
  error?: string | null;
  extensions?: Readonly<Record<string, unknown>>;
}>): X402PaymentRequiredContract {
  validateRunxX402InvocationExtensionInfoContract(input.invocation);
  const extensions = { ...(input.extensions ?? {}) };
  if (Object.hasOwn(extensions, RUNX_X402_INVOCATION_EXTENSION_KEY)) {
    throw new X402PresentationError("reserved_extension");
  }
  const declaration: RunxX402InvocationExtensionContract = {
    info: input.invocation,
    schema: RUNX_X402_INVOCATION_SCHEMA_REFERENCE,
  };
  return assembleExternalX402PaymentRequired({
    resource: input.resource,
    accepts: input.accepts,
    ...(input.error === undefined ? {} : { error: input.error }),
    extensions: {
      ...extensions,
      [RUNX_X402_INVOCATION_EXTENSION_KEY]: declaration,
    },
  });
}

/** Effect-free discovery for a vendor resource that is not a Runx invocation. */
export function x402ExternalDiscoveryHttpProjection(
  descriptor: X402ExternalDiscoveryDescriptor,
): X402DiscoveryHttpProjection {
  assertBazaarDiscoveryExtension(descriptor.extensions);
  const body = assembleExternalX402PaymentRequired({
    resource: descriptor.resource,
    accepts: descriptor.accepts,
    extensions: descriptor.extensions,
    ...(descriptor.error === undefined ? {} : { error: descriptor.error }),
  });
  return {
    status: 402,
    body,
    headers: {
      [X402_PAYMENT_REQUIRED_HEADER]: encodeX402PaymentRequiredHeader(body),
    },
  };
}

/**
 * Build the inert, floor-priced discovery response a vendor can mount before
 * auth or body validation. This function has no clock, store, provider, or
 * network dependency and therefore cannot create product or payment state.
 */
export function x402DiscoveryHttpProjection(
  descriptor: X402DiscoveryDescriptor,
): X402DiscoveryHttpProjection {
  assertBazaarDiscoveryExtension(descriptor.extensions);
  const body = assembleX402PaymentRequired({
    resource: descriptor.resource,
    accepts: descriptor.accepts,
    invocation: {
      purpose: "discovery",
      offer_revision: descriptor.offerRevision,
      package_digest: descriptor.packageDigest,
    },
    extensions: descriptor.extensions,
  });
  return {
    status: 402,
    body,
    headers: {
      [X402_PAYMENT_REQUIRED_HEADER]: encodeX402PaymentRequiredHeader(body),
    },
  };
}

export function bindX402PaymentRequiredChallenge(
  paymentRequired: X402PaymentRequiredContract,
  quoteRef: ReferenceContract,
  quoteExpiresAt: string,
): PaidInvocationPaymentChallengeContract {
  const payload = validateX402PaymentRequiredContract(paymentRequired);
  return validatePaidInvocationPaymentChallengeContract({
    settlement_family: "x402",
    protocol_version: "2",
    media_type: X402_JSON_MEDIA_TYPE,
    payload,
    payload_digest: sha256Prefixed(canonicalJsonStringify(payload)),
    quote_ref: quoteRef,
    quote_expires_at: quoteExpiresAt,
  });
}

export function x402PaymentRequiredFromChallenge(
  challenge: PaidInvocationPaymentChallengeContract,
): X402PaymentRequiredContract {
  if (
    challenge.settlement_family !== "x402"
    || challenge.protocol_version !== "2"
    || challenge.media_type !== X402_JSON_MEDIA_TYPE
  ) {
    throw new X402PresentationError("challenge_kind_mismatch");
  }
  let actualDigest: string;
  try {
    actualDigest = sha256Prefixed(canonicalJsonStringify(challenge.payload));
  } catch {
    throw new X402PresentationError("invalid_payload");
  }
  if (actualDigest !== challenge.payload_digest) {
    throw new X402PresentationError("challenge_digest_mismatch");
  }
  try {
    return validateX402PaymentRequiredContract(challenge.payload);
  } catch {
    throw new X402PresentationError("invalid_payload");
  }
}

export function validateX402PaymentRetry(
  challengeValue: X402PaymentRequiredContract,
  retryValue: X402PaymentPayloadContract,
): ValidatedX402Retry {
  const challenge = validateX402PaymentRequiredContract(challengeValue);
  const retry = validateX402PaymentPayloadContract(retryValue);
  if (retry.resource == null || !jsonEquals(retry.resource, challenge.resource)) {
    throw new X402PresentationError("resource_mismatch");
  }
  const requirementIndex = challenge.accepts.findIndex((candidate) =>
    jsonEquals(candidate, retry.accepted)
  );
  if (requirementIndex < 0) {
    throw new X402PresentationError("requirement_mismatch");
  }
  const declared = parseRunxX402InvocationDeclaration(challenge.extensions);
  const echoed = parseRunxX402InvocationDeclaration(retry.extensions);
  if (!jsonEquals(echoed, declared)) {
    throw new X402PresentationError("runx_invocation_mismatch");
  }
  return { requirementIndex, invocation: declared.info };
}

export function selectedX402Requirement(
  challenge: X402PaymentRequiredContract,
  validated: ValidatedX402Retry,
): X402PaymentRequirementsContract | undefined {
  return challenge.accepts[validated.requirementIndex];
}

export function encodeX402PaymentRequiredHeader(value: X402PaymentRequiredContract): string {
  return encodeHeader(validateX402PaymentRequiredContract(value));
}

export function decodeX402PaymentRequiredHeader(value: string): X402PaymentRequiredContract {
  return decodeHeader(value, validateX402PaymentRequiredContract);
}

export function encodeX402PaymentSignatureHeader(value: X402PaymentPayloadContract): string {
  return encodeHeader(validateX402PaymentPayloadContract(value));
}

export function decodeX402PaymentSignatureHeader(value: string): X402PaymentPayloadContract {
  return decodeHeader(value, validateX402PaymentPayloadContract);
}

export function encodeX402PaymentResponseHeader(value: X402SettleResponseContract): string {
  return encodeHeader(validateX402SettleResponseContract(value));
}

export function decodeX402PaymentResponseHeader(value: string): X402SettleResponseContract {
  return decodeHeader(value, validateX402SettleResponseContract);
}

function encodeHeader(value: unknown): string {
  let bytes: Buffer;
  try {
    bytes = Buffer.from(JSON.stringify(value), "utf8");
  } catch {
    throw new X402PresentationError("encoding_failed");
  }
  if (bytes.byteLength > MAX_X402_DECODED_BYTES) {
    throw new X402PresentationError("header_too_large");
  }
  const encoded = bytes.toString("base64");
  if (encoded.length > MAX_X402_HEADER_BYTES) {
    throw new X402PresentationError("header_too_large");
  }
  return encoded;
}

function decodeHeader<T>(value: string, validate: (value: unknown) => T): T {
  if (value.length > MAX_X402_HEADER_BYTES) {
    throw new X402PresentationError("header_too_large");
  }
  if (!isStandardBase64(value)) {
    throw new X402PresentationError("invalid_base64");
  }
  const bytes = Buffer.from(paddedBase64(value), "base64");
  if (bytes.byteLength > MAX_X402_DECODED_BYTES) {
    throw new X402PresentationError("header_too_large");
  }
  try {
    return validate(JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(bytes)));
  } catch {
    throw new X402PresentationError("invalid_payload");
  }
}

/**
 * Read and normalize the `runx.invocation` declaration under `extensions`.
 * The advertised schema must be the published v1 schema, by reference or as
 * the inline document challenges carried before the reference form; both
 * name the same schema, so the declaration comes back in the reference form
 * and two echoes of one challenge compare equal whichever way it was
 * assembled. Inline acceptance retires once every emitter is on the
 * reference form.
 */
export function parseRunxX402InvocationDeclaration(
  extensions: Readonly<Record<string, unknown>> | null | undefined,
): RunxX402InvocationExtensionContract {
  const value = extensions?.[RUNX_X402_INVOCATION_EXTENSION_KEY];
  const record = asUnknownRecord(value);
  if (!record) {
    throw new X402PresentationError("missing_runx_invocation");
  }
  const keys = Object.keys(record).sort();
  if (keys.length !== 2 || keys[0] !== "info" || keys[1] !== "schema") {
    throw new X402PresentationError("invalid_payload");
  }
  let info: RunxX402ExtensionInfoContract;
  try {
    info = validateRunxX402InvocationExtensionInfoContract(record.info);
  } catch {
    throw new X402PresentationError("invalid_payload");
  }
  if (
    !jsonEquals(record.schema, RUNX_X402_INVOCATION_SCHEMA_REFERENCE)
    && !jsonEquals(record.schema, runxX402InvocationExtensionInfoV1Schema)
  ) {
    throw new X402PresentationError("runx_invocation_schema_mismatch");
  }
  return { info, schema: RUNX_X402_INVOCATION_SCHEMA_REFERENCE };
}

function assertBazaarDiscoveryExtension(extensions: Readonly<Record<string, unknown>>): void {
  const declaration = asUnknownRecord(extensions[X402_BAZAAR_EXTENSION_KEY]);
  if (!declaration) throw new X402PresentationError("missing_discovery_extension");
  const keys = Object.keys(declaration).sort();
  if (keys.length !== 2 || keys[0] !== "info" || keys[1] !== "schema") {
    throw new X402PresentationError("invalid_discovery_extension");
  }
  const info = asUnknownRecord(declaration.info);
  if (!info || !asUnknownRecord(declaration.schema)) {
    throw new X402PresentationError("invalid_discovery_extension");
  }
  if (Object.hasOwn(info, "discoverable")) {
    throw new X402PresentationError("invalid_discovery_extension");
  }
}

function jsonEquals(left: unknown, right: unknown): boolean {
  try {
    return canonicalJsonStringify(left) === canonicalJsonStringify(right);
  } catch {
    return false;
  }
}

function isStandardBase64(value: string): boolean {
  const padding = value.length - value.replace(/=+$/u, "").length;
  if (padding > 2 || (padding > 0 && value.length % 4 !== 0)) {
    return false;
  }
  if (padding === 0 && value.length % 4 === 1) {
    return false;
  }
  const dataLength = value.length - padding;
  for (let index = 0; index < value.length; index += 1) {
    const character = value[index]!;
    if (index >= dataLength) {
      if (character !== "=") return false;
    } else if (!/[A-Za-z0-9+/]/u.test(character)) {
      return false;
    }
  }
  return true;
}

function paddedBase64(value: string): string {
  if (value.endsWith("=") || value.length % 4 === 0) return value;
  return value + (value.length % 4 === 2 ? "==" : "=");
}
