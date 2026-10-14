/**
 * The registry's signed manifest: one envelope for skills and connectors,
 * mirroring `runx-contracts::registry_signed_manifest` against the same
 * vectors (`fixtures/contracts/registry-signed-manifest`). The registry signs
 * the canonical JSON of the envelope without its signature; `content_digest`
 * covers every file of the version, so every digest a kind derives comes from
 * bytes the signature already binds.
 */
import { canonicalJsonStringify, sha256Prefixed } from "./canonical-json.js";
import { isRegistryVersion } from "./registry-version.js";

export const REGISTRY_SIGNED_MANIFEST_SCHEMA = "runx.registry.signed_manifest.v1" as const;
export const REGISTRY_MANIFEST_SIGNATURE_ALG = "ed25519" as const;
export const REGISTRY_MANIFEST_SIGNATURE_PREFIX = "base64:" as const;

export type RegistrySubjectKind = "skill" | "connector";

export interface RegistrySubject {
  readonly kind: RegistrySubjectKind;
  /**
   * `publisher/name` for a skill, `publisher/id` for a connector.
   * @pattern ^[a-z0-9]([a-z0-9-]{0,62}[a-z0-9])?/[a-z0-9]([a-z0-9-]{0,62}[a-z0-9])?$
   */
  readonly ref: string;
}

export interface RegistrySignedManifest {
  readonly schema: typeof REGISTRY_SIGNED_MANIFEST_SCHEMA;
  readonly subject: RegistrySubject;
  /** @pattern ^[A-Za-z0-9][A-Za-z0-9._+-]{0,127}$ */
  readonly version: string;
  /** @pattern ^sha256:[0-9a-f]{64}$ */
  readonly content_digest: `sha256:${string}`;
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

export type UnsignedRegistryManifest = Omit<RegistrySignedManifest, "signature">;

export interface RegistryContentFile {
  readonly path: string;
  readonly content: string;
}

/**
 * `sha256:` over the canonical JSON of every file of a version, in path byte
 * order. A skill's files include its `SKILL.md` and `run.yaml`; a
 * connector's its `connector.yaml`, `plugin.json`, tools, README and mark.
 */
export function registryContentDigest(files: readonly RegistryContentFile[]): `sha256:${string}` {
  if (files.length === 0) throw new Error("A registry version has no files.");
  const sorted = [...files].sort((left, right) => (left.path < right.path ? -1 : left.path > right.path ? 1 : 0));
  for (let index = 1; index < sorted.length; index += 1) {
    if (sorted[index]!.path === sorted[index - 1]!.path) throw new Error(`Registry version lists ${sorted[index]!.path} more than once.`);
  }
  return sha256Prefixed(canonicalJsonStringify({ files: sorted.map((file) => ({ content: file.content, path: file.path })) }));
}

/** The bytes the registry signs: the canonical JSON of the envelope without its signature. */
export function registrySignedManifestPayload(manifest: UnsignedRegistryManifest | RegistrySignedManifest): string {
  return canonicalJsonStringify({
    schema: manifest.schema,
    subject: { kind: manifest.subject.kind, ref: manifest.subject.ref },
    version: manifest.version,
    content_digest: manifest.content_digest,
    signer: { id: manifest.signer.id, key_id: manifest.signer.key_id },
  });
}

/**
 * Whether an envelope signs the given version of the given subject. The kind
 * is part of the identity: a connector's envelope never stands for a skill,
 * nor a skill's for a connector, even where the ref fits both grammars.
 */
export function registrySignedManifestSigns(manifest: RegistrySignedManifest, subject: RegistrySubject, version: string): boolean {
  return manifest.subject.kind === subject.kind && manifest.subject.ref === subject.ref && manifest.version === version;
}

/** A subject of the given kind: a skill's `publisher/name`, or a connector's name. */
export function isRegistrySubjectRef(kind: RegistrySubjectKind, value: string): boolean {
  if (kind === "connector") return isConnectorName(value);
  const slash = value.indexOf("/");
  if (slash < 0) return false;
  return PUBLISHER_NAMESPACE.test(value.slice(0, slash)) && SKILL_NAME.test(value.slice(slash + 1));
}

/**
 * A connector's name: a registry namespace (`stripe`), or a namespace and one
 * surface (`google/calendar`); `versions` is reserved in either place.
 */
export function isConnectorName(value: string): boolean {
  return CONNECTOR_NAME.test(value);
}

/** Checks the envelope's shape and every term; refuses any field the contract does not name. */
export function validateRegistrySignedManifest(value: unknown): RegistrySignedManifest {
  const manifest = object(value, "signed_manifest", ["schema", "subject", "version", "content_digest", "signer", "signature"]);
  const subject = object(manifest.subject, "signed_manifest.subject", ["kind", "ref"]);
  const signer = object(manifest.signer, "signed_manifest.signer", ["id", "key_id"]);
  const signature = object(manifest.signature, "signed_manifest.signature", ["alg", "value"]);
  if (manifest.schema !== REGISTRY_SIGNED_MANIFEST_SCHEMA) throw new Error(`signed_manifest.schema must be ${REGISTRY_SIGNED_MANIFEST_SCHEMA}.`);
  if (subject.kind !== "skill" && subject.kind !== "connector") throw new Error("signed_manifest.subject.kind must be skill or connector.");
  if (typeof subject.ref !== "string" || !isRegistrySubjectRef(subject.kind, subject.ref)) {
    throw new Error(`signed_manifest.subject.ref is not a ${subject.kind} reference.`);
  }
  if (typeof manifest.version !== "string" || !isRegistryVersion(manifest.version)) throw new Error("signed_manifest.version is not a registry version label.");
  if (typeof manifest.content_digest !== "string" || !SHA256.test(manifest.content_digest)) throw new Error("signed_manifest.content_digest must be sha256: and 64 lowercase hex digits.");
  if (!isSignerTerm(signer.id) || !isSignerTerm(signer.key_id)) throw new Error("signed_manifest.signer needs short printable id and key_id.");
  if (signature.alg !== REGISTRY_MANIFEST_SIGNATURE_ALG) throw new Error(`signed_manifest.signature.alg must be ${REGISTRY_MANIFEST_SIGNATURE_ALG}.`);
  if (typeof signature.value !== "string" || !signature.value.startsWith(REGISTRY_MANIFEST_SIGNATURE_PREFIX)) {
    throw new Error(`signed_manifest.signature.value must start with ${REGISTRY_MANIFEST_SIGNATURE_PREFIX}.`);
  }
  return value as RegistrySignedManifest;
}

const PUBLISHER_NAMESPACE = /^[a-z0-9](?:[a-z0-9-]{0,62}[a-z0-9])?$/u;
const SKILL_NAME = /^(?=.{1,64}$)[a-z0-9]+(?:-[a-z0-9]+)*$/u;
const CONNECTOR_NAME = /^(?!versions(?:\/|$))[a-z0-9](?:[a-z0-9-]{0,62}[a-z0-9])?(?:\/(?!versions$)[a-z][a-z0-9-]{0,62}[a-z0-9])?$/u;
const SHA256 = /^sha256:[0-9a-f]{64}$/u;

function isSignerTerm(value: unknown): value is string {
  return typeof value === "string" && value.length >= 1 && value.length <= 128 && /^[\x21-\x7e]+$/u.test(value);
}

function object(value: unknown, label: string, fields: readonly string[]): Readonly<Record<string, unknown>> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) throw new Error(`${label} must be an object.`);
  const unknown = Object.keys(value).find((key) => !fields.includes(key));
  if (unknown !== undefined) throw new Error(`${label} has no field ${unknown}.`);
  return value as Readonly<Record<string, unknown>>;
}
