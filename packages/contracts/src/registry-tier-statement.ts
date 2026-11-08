/**
 * The registry's tier statement: a version's tier, signed apart from its
 * manifest because a tier and a withdrawal change after publication,
 * mirroring `runx-contracts::registry_tier_statement` against the same
 * vectors (`fixtures/contracts/registry-tier-statement`). The registry signs
 * the canonical JSON of the statement without its signature, as it signs a
 * manifest. What a key may state of a tier is the runtime's to check, against
 * its own key set.
 */
import { canonicalJsonStringify } from "./canonical-json.js";
import type { TrustTierContract } from "./generated/contract-types.js";
import {
  REGISTRY_MANIFEST_SIGNATURE_ALG,
  REGISTRY_MANIFEST_SIGNATURE_PREFIX,
  exactObject,
  isRegistrySubjectName,
  isSha256Digest,
  isSignerTerm,
  type RegistrySubject,
} from "./registry-signed-manifest.js";
import { isRegistryVersion } from "./registry-version.js";

export const REGISTRY_TIER_STATEMENT_SCHEMA = "runx.registry.tier_statement.v1" as const;

export interface RegistryTierStatement {
  readonly schema: typeof REGISTRY_TIER_STATEMENT_SCHEMA;
  readonly subject: RegistrySubject;
  /** @pattern ^[A-Za-z0-9][A-Za-z0-9._+-]{0,127}$ */
  readonly version: string;
  /** @pattern ^sha256:[0-9a-f]{64}$ */
  readonly content_digest: `sha256:${string}`;
  readonly tier: TrustTierContract;
  /**
   * When the registry stated it, a UTC instant to the second.
   * @pattern ^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z$
   */
  readonly issued_at: string;
  /**
   * When it stops holding, after `issued_at`.
   * @pattern ^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z$
   */
  readonly expires_at: string;
  readonly signer: {
    /** @pattern ^[\x21-\x7e]{1,128}$ */
    readonly id: string;
    /** @pattern ^[\x21-\x7e]{1,128}$ */
    readonly key_id: string;
  };
  readonly signature: {
    readonly alg: typeof REGISTRY_MANIFEST_SIGNATURE_ALG;
    /** @pattern ^base64:[A-Za-z0-9_-]+$ */
    readonly value: string;
  };
}

export type UnsignedRegistryTierStatement = Omit<RegistryTierStatement, "signature">;

/** The bytes the registry signs: the canonical JSON of the statement without its signature. */
export function registryTierStatementPayload(statement: UnsignedRegistryTierStatement | RegistryTierStatement): string {
  return canonicalJsonStringify({
    schema: statement.schema,
    subject: { kind: statement.subject.kind, name: statement.subject.name },
    version: statement.version,
    content_digest: statement.content_digest,
    tier: statement.tier,
    issued_at: statement.issued_at,
    expires_at: statement.expires_at,
    signer: { id: statement.signer.id, key_id: statement.signer.key_id },
  });
}

/** Checks the statement's shape and every term; refuses any field the contract does not name. */
export function validateRegistryTierStatement(value: unknown): RegistryTierStatement {
  const statement = exactObject(value, "tier_statement", [
    "schema", "subject", "version", "content_digest", "tier", "issued_at", "expires_at", "signer", "signature",
  ]);
  const subject = exactObject(statement.subject, "tier_statement.subject", ["kind", "name"]);
  const signer = exactObject(statement.signer, "tier_statement.signer", ["id", "key_id"]);
  const signature = exactObject(statement.signature, "tier_statement.signature", ["alg", "value"]);
  if (statement.schema !== REGISTRY_TIER_STATEMENT_SCHEMA) throw new Error(`tier_statement.schema must be ${REGISTRY_TIER_STATEMENT_SCHEMA}.`);
  if (subject.kind !== "skill" && subject.kind !== "connector") throw new Error("tier_statement.subject.kind must be skill or connector.");
  if (typeof subject.name !== "string" || !isRegistrySubjectName(subject.kind, subject.name)) {
    throw new Error(`tier_statement.subject.name is not a ${subject.kind} name.`);
  }
  if (typeof statement.version !== "string" || !isRegistryVersion(statement.version)) throw new Error("tier_statement.version is not a registry version label.");
  if (!isSha256Digest(statement.content_digest)) throw new Error("tier_statement.content_digest must be sha256: and 64 lowercase hex digits.");
  if (!TIERS.includes(statement.tier as TrustTierContract)) throw new Error("tier_statement.tier must be first_party, verified or community.");
  if (!isUtcSecond(statement.issued_at) || !isUtcSecond(statement.expires_at)) {
    throw new Error("tier_statement.issued_at and expires_at must be YYYY-MM-DDTHH:MM:SSZ.");
  }
  // The form orders as the instants it names, so the strings compare.
  if (statement.issued_at >= statement.expires_at) throw new Error("tier_statement.expires_at must follow issued_at.");
  if (!isSignerTerm(signer.id) || !isSignerTerm(signer.key_id)) throw new Error("tier_statement.signer needs short printable id and key_id.");
  if (signature.alg !== REGISTRY_MANIFEST_SIGNATURE_ALG) throw new Error(`tier_statement.signature.alg must be ${REGISTRY_MANIFEST_SIGNATURE_ALG}.`);
  if (typeof signature.value !== "string" || !signature.value.startsWith(REGISTRY_MANIFEST_SIGNATURE_PREFIX)) {
    throw new Error(`tier_statement.signature.value must start with ${REGISTRY_MANIFEST_SIGNATURE_PREFIX}.`);
  }
  return value as RegistryTierStatement;
}

const TIERS: readonly TrustTierContract[] = ["first_party", "verified", "community"];

/** `YYYY-MM-DDTHH:MM:SSZ`, which orders as the instants it names. */
function isUtcSecond(value: unknown): value is string {
  return typeof value === "string" && /^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z$/u.test(value);
}
