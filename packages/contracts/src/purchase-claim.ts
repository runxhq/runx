import { canonicalJsonStringify } from "./canonical-json.js";
import {
  type DeepReadonly,
  generatedSchema,
  validateContractSchema,
} from "./internal.js";

/**
 * How a purchase bound to a buyer key is read and claimed (S10). runx holds
 * what the purchase bought; its buyer signs each read with the key, and each
 * claim of what it holds with the key over the digest of the purchase's
 * receipt. runx verifies both; no seller does. `fixtures/contracts/purchase-claim`
 * holds requests Rust signed that this verifier is tested against.
 */

export const PURCHASE_CLAIM_SCHEMA = "runx.payment.purchase_claim.v1";
export const PURCHASE_READ_REQUEST_SCHEMA = "runx.payment.purchase_read_request.v1";

export type PurchaseClaimContract = DeepReadonly<{
  receipt_digest: string;
  claim_url: string;
  issued_at: string;
  signature: string;
}>;

export const purchaseClaimV1Schema =
  generatedSchema<PurchaseClaimContract>("purchase-claim.schema.json");

export function validatePurchaseClaimContract(value: unknown, label = "purchase_claim") {
  return validateContractSchema(purchaseClaimV1Schema, value, label);
}

const BUYER_KEY = /^ed25519:[A-Za-z0-9_-]{42}[AEIMQUYcgkosw048]$/u;

/** An Ed25519 public key a buyer holds, spelled the one way runx accepts. */
export function isBuyerKey(value: unknown): value is string {
  return typeof value === "string" && BUYER_KEY.test(value);
}

/**
 * The bytes a buyer signs to read its purchase, which may have no
 * receipt yet: the read schema, the method and URL, and the time. runx
 * refuses a read signed outside a short window around its clock.
 */
export function purchaseReadRequestMessage(input: {
  readonly method: string;
  readonly url: string;
  readonly issuedAt: string;
}): Uint8Array<ArrayBuffer> {
  return new TextEncoder().encode(canonicalJsonStringify({
    schema: PURCHASE_READ_REQUEST_SCHEMA,
    method: input.method,
    url: input.url,
    issued_at: input.issuedAt,
  }));
}

/** The bytes a buyer signs for one claim. */
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

/**
 * Whether `signature` is the buyer key's own signature over `message`:
 * how runx admits a read or claim of a purchase a key owns.
 */
export async function verifyBuyerSignature(input: {
  readonly buyerKey: string;
  readonly signature: string;
  readonly message: Uint8Array<ArrayBuffer>;
}): Promise<boolean> {
  return await verifyEd25519(ed25519Bytes(input.buyerKey), ed25519Bytes(input.signature), input.message);
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
