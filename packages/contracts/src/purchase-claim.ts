import { canonicalJsonStringify } from "./canonical-json.js";
import {
  type DeepReadonly,
  generatedSchema,
  validateContractSchema,
} from "./internal.js";

/**
 * How a purchaser collects what a fulfilled purchase bought but runx never
 * holds (S10). runx signs the purchase receipt, which binds the purchaser's
 * key; the purchaser signs each claim with that key. A seller verifies both
 * with `verifyPurchaseClaim`, offline, against runx's published trust root
 * (`GET /v1/trust/keys`). `fixtures/contracts/purchase-claim` holds claims
 * Rust signed that this verifier is tested against.
 */

export const PURCHASE_CLAIM_SCHEMA = "runx.payment.purchase_claim.v1";
export const PURCHASE_RECEIPT_SCHEMA = "runx.hosted_paid_invocation_terminal_receipt.v1";

export type PurchaseClaimContract = DeepReadonly<{
  receipt: Record<string, unknown>;
  claim_url: string;
  issued_at: string;
  signature: string;
}>;

export const purchaseClaimV1Schema =
  generatedSchema<PurchaseClaimContract>("purchase-claim.schema.json");

export function validatePurchaseClaimContract(value: unknown, label = "purchase_claim") {
  return validateContractSchema(purchaseClaimV1Schema, value, label);
}

const PURCHASER_KEY = /^ed25519:[A-Za-z0-9_-]{42}[AEIMQUYcgkosw048]$/u;

/** An Ed25519 public key a purchaser holds, spelled the one way runx accepts. */
export function isPurchaserKey(value: unknown): value is string {
  return typeof value === "string" && PURCHASER_KEY.test(value);
}

/** The bytes a purchaser signs for one claim. */
export function purchaseClaimMessage(input: {
  readonly claimUrl: string;
  readonly issuedAt: string;
  readonly receiptDigest: string;
}): Uint8Array<ArrayBuffer> {
  return new TextEncoder().encode(canonicalJsonStringify({
    schema: PURCHASE_CLAIM_SCHEMA,
    claim_url: input.claimUrl,
    issued_at: input.issuedAt,
    receipt_digest: input.receiptDigest,
  }));
}

/** A key in runx's published trust root (`runx.trust_root.v1`). */
export interface PurchaseTrustRootKey {
  readonly kty: string;
  readonly crv: string;
  readonly kid: string;
  readonly purpose: string;
  readonly x: string;
}

export type PurchaseClaimRefusal =
  | "malformed"
  | "receipt_digest"
  | "receipt_signature"
  | "seller"
  | "outcome"
  | "purchaser_key"
  | "claim_url"
  | "stale"
  | "purchaser_signature";

export class PurchaseClaimError extends Error {
  constructor(readonly code: PurchaseClaimRefusal, message: string) {
    super(message);
    this.name = "PurchaseClaimError";
  }
}

export interface VerifiedPurchaseClaim {
  /** The paid invocation the claim is for: the seller's own record of the sale. */
  readonly invocationId: string;
  readonly purchaserKey: string;
  readonly receiptDigest: string;
}

/**
 * Verify a claim a seller received at `claimUrl`: runx signed a fulfilled
 * purchase receipt for this seller that binds a purchaser key, and that key
 * signed this claim, for this endpoint, recently. Throws a
 * `PurchaseClaimError` naming the first check that fails.
 */
export async function verifyPurchaseClaim(input: {
  readonly claim: unknown;
  readonly trustRoot: { readonly keys: readonly PurchaseTrustRootKey[] };
  readonly sellerPrincipalUri: string;
  readonly claimUrl: string;
  readonly now: Date;
  readonly maxSkewSeconds?: number;
}): Promise<VerifiedPurchaseClaim> {
  let claim: PurchaseClaimContract;
  try {
    claim = validatePurchaseClaimContract(input.claim);
  } catch (error) {
    throw new PurchaseClaimError("malformed", error instanceof Error ? error.message : "The claim is malformed.");
  }
  const receipt = claim.receipt;
  const digest = text(receipt.digest);
  if (receipt.schema !== PURCHASE_RECEIPT_SCHEMA || !digest) {
    throw new PurchaseClaimError("malformed", "The claim does not present a runx purchase receipt.");
  }
  if (await receiptBodyDigest(receipt) !== digest) {
    throw new PurchaseClaimError("receipt_digest", "The receipt changed after runx signed it.");
  }
  if (!await receiptSignedByRunx(receipt, digest, input.trustRoot.keys)) {
    throw new PurchaseClaimError("receipt_signature", "runx did not sign this receipt.");
  }
  const binding = record(receipt.paid_invocation);
  const terminal = record(receipt.terminal);
  if (text(record(binding?.vendor_ref)?.uri) !== input.sellerPrincipalUri) {
    throw new PurchaseClaimError("seller", "The receipt is for another seller's purchase.");
  }
  if (terminal?.outcome !== "fulfilment") {
    throw new PurchaseClaimError("outcome", "The purchase was not fulfilled.");
  }
  const purchaserKey = text(binding?.purchaser_key);
  const invocationId = text(binding?.invocation_id);
  if (!purchaserKey || !invocationId) {
    throw new PurchaseClaimError("purchaser_key", "The purchase names no purchaser key to claim with.");
  }
  if (claim.claim_url !== input.claimUrl) {
    throw new PurchaseClaimError("claim_url", "The claim was signed for another endpoint.");
  }
  const issuedAt = Date.parse(claim.issued_at);
  const skewMs = (input.maxSkewSeconds ?? 300) * 1000;
  if (!Number.isFinite(issuedAt) || Math.abs(input.now.getTime() - issuedAt) > skewMs) {
    throw new PurchaseClaimError("stale", "The claim was not signed recently.");
  }
  const message = purchaseClaimMessage({ claimUrl: claim.claim_url, issuedAt: claim.issued_at, receiptDigest: digest });
  if (!await verifyEd25519(ed25519Bytes(purchaserKey), ed25519Bytes(claim.signature), message)) {
    throw new PurchaseClaimError("purchaser_signature", "The purchaser key did not sign this claim.");
  }
  return { invocationId, purchaserKey, receiptDigest: digest };
}

async function receiptBodyDigest(receipt: Readonly<Record<string, unknown>>): Promise<string> {
  const { signature: _signature, digest: _digest, metadata: _metadata, ...body } = receipt;
  const bytes = new TextEncoder().encode(canonicalJsonStringify(body));
  const hash = new Uint8Array(await crypto.subtle.digest("SHA-256", bytes));
  return `sha256:${[...hash].map((byte) => byte.toString(16).padStart(2, "0")).join("")}`;
}

async function receiptSignedByRunx(
  receipt: Readonly<Record<string, unknown>>,
  digest: string,
  keys: readonly PurchaseTrustRootKey[],
): Promise<boolean> {
  const signature = record(receipt.signature);
  const value = text(signature?.value);
  if (signature?.alg !== "Ed25519" || !value?.startsWith("base64:")) return false;
  const signatureBytes = base64UrlBytes(value.slice("base64:".length));
  if (!signatureBytes || signatureBytes.length !== 64) return false;
  const issuerKid = text(record(receipt.issuer)?.kid);
  const candidates = keys.filter((key) =>
    key.purpose === "receipt_signature"
    && key.kty === "OKP"
    && key.crv === "Ed25519"
    && (!issuerKid || key.kid === issuerKid));
  const message = new TextEncoder().encode(digest);
  for (const key of candidates) {
    const publicKey = base64UrlBytes(key.x);
    if (publicKey?.length === 32 && await verifyEd25519(publicKey, signatureBytes, message)) return true;
  }
  return false;
}

async function verifyEd25519(
  publicKey: Uint8Array<ArrayBuffer> | undefined,
  signature: Uint8Array<ArrayBuffer> | undefined,
  message: Uint8Array<ArrayBuffer>,
): Promise<boolean> {
  if (!publicKey || !signature) return false;
  try {
    const key = await crypto.subtle.importKey("raw", publicKey, { name: "Ed25519" }, false, ["verify"]);
    return await crypto.subtle.verify({ name: "Ed25519" }, key, signature, message);
  } catch {
    return false;
  }
}

function ed25519Bytes(value: string): Uint8Array<ArrayBuffer> | undefined {
  return value.startsWith("ed25519:") ? base64UrlBytes(value.slice("ed25519:".length)) : undefined;
}

function base64UrlBytes(value: string): Uint8Array<ArrayBuffer> | undefined {
  if (!/^[A-Za-z0-9_-]+$/u.test(value)) return undefined;
  const padded = value.replaceAll("-", "+").replaceAll("_", "/").padEnd(Math.ceil(value.length / 4) * 4, "=");
  try {
    return Uint8Array.from(atob(padded), (character) => character.charCodeAt(0));
  } catch {
    return undefined;
  }
}

function record(value: unknown): Readonly<Record<string, unknown>> | undefined {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    ? value as Readonly<Record<string, unknown>>
    : undefined;
}

function text(value: unknown): string | undefined {
  return typeof value === "string" && value.length > 0 ? value : undefined;
}
