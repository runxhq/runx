import { generateKeyPairSync, sign } from "node:crypto";
import { readdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import path from "node:path";

import {
  REGISTRY_SIGNED_MANIFEST_SCHEMA,
  registryContentDigest,
  registrySignedManifestPayload,
} from "@runxhq/contracts";

export function createRegistryTestSigningKey({
  keyId = "runx-test-registry-ed25519",
  signerId = "runx-test-registry",
} = {}) {
  const keyPair = generateKeyPairSync("ed25519");
  const publicKeyDer = keyPair.publicKey.export({ format: "der", type: "spki" });
  return {
    keyId,
    signerId,
    publicKeyBase64: Buffer.from(publicKeyDer).subarray(-32).toString("base64"),
    privateKey: keyPair.privateKey,
  };
}

/**
 * Sign the one skill version in a local registry with the envelope the
 * registry serves: its subject, version and the content digest of its manual,
 * profile and package files.
 */
export function signSingleRegistryVersion(registryDirectory, signingKey) {
  const versionPath = findSingleRegistryVersion(registryDirectory);
  const version = JSON.parse(readFileSync(versionPath, "utf8"));
  const files = [
    { path: "SKILL.md", content: requiredString(version, "markdown") },
    ...(optionalString(version, "profile_document")
      ? [{ path: "run.yaml", content: version.profile_document }]
      : []),
    ...(version.package_files ?? []),
  ];
  const manifest = {
    schema: REGISTRY_SIGNED_MANIFEST_SCHEMA,
    subject: { kind: "skill", ref: requiredString(version, "skill_id") },
    version: requiredString(version, "version"),
    content_digest: registryContentDigest(files),
    signer: { id: signingKey.signerId, key_id: signingKey.keyId },
  };
  const signature = sign(
    null,
    Buffer.from(registrySignedManifestPayload(manifest)),
    signingKey.privateKey,
  ).toString("base64url");
  version.signed_manifest = {
    ...manifest,
    signature: { alg: "ed25519", value: `base64:${signature}` },
  };
  writeFileSync(versionPath, `${JSON.stringify(version, null, 2)}\n`, "utf8");
  return versionPath;
}

function findSingleRegistryVersion(root) {
  const matches = [];
  const walk = (directory) => {
    for (const entry of readdirSync(directory)) {
      const entryPath = path.join(directory, entry);
      if (statSync(entryPath).isDirectory()) {
        walk(entryPath);
      } else if (entryPath.endsWith(".json")) {
        matches.push(entryPath);
      }
    }
  };
  walk(root);
  if (matches.length !== 1) {
    throw new Error(`expected one registry version, found ${matches.length}`);
  }
  return matches[0];
}

function requiredString(object, field) {
  const value = optionalString(object, field);
  if (!value) throw new Error(`registry version is missing ${field}`);
  return value;
}

function optionalString(object, field) {
  const value = object?.[field];
  return typeof value === "string" ? value : "";
}
