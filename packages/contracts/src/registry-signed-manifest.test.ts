import { createPublicKey, verify } from "node:crypto";
import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";

import {
  registryContentDigest,
  registrySignedManifestPayload,
  registrySignedManifestSigns,
  validateRegistrySignedManifest,
  type RegistryContentFile,
  type RegistrySubject,
} from "./registry-signed-manifest.js";

const fixture = JSON.parse(readFileSync(
  new URL("../../../fixtures/contracts/registry-signed-manifest/registry-signed-manifest-v1.vectors.json", import.meta.url),
  "utf8",
)) as {
  readonly public_key_base64: string;
  readonly content_digests: readonly { readonly name: string; readonly files: readonly RegistryContentFile[]; readonly digest: string }[];
  readonly invalid_content: readonly { readonly name: string; readonly files: readonly RegistryContentFile[] }[];
  readonly envelopes: readonly { readonly name: string; readonly files: readonly RegistryContentFile[]; readonly manifest: unknown; readonly signing_payload: string }[];
  readonly invalid_envelopes: readonly { readonly name: string; readonly manifest: unknown }[];
  readonly subject_bindings: readonly {
    readonly name: string;
    readonly envelope: string;
    readonly subject: RegistrySubject;
    readonly version: string;
    readonly signs: boolean;
  }[];
  readonly unknown_field_envelope: unknown;
};

describe("registry signed manifest", () => {
  it("digests a version's files as the Rust contract does", () => {
    for (const testCase of fixture.content_digests) expect(registryContentDigest(testCase.files), testCase.name).toBe(testCase.digest);
    for (const testCase of fixture.invalid_content) expect(() => registryContentDigest(testCase.files), testCase.name).toThrow();
  });

  it("signs the canonical envelope, and the signature binds every file", () => {
    const key = createPublicKey({
      key: Buffer.concat([Buffer.from("302a300506032b6570032100", "hex"), Buffer.from(fixture.public_key_base64, "base64")]),
      format: "der",
      type: "spki",
    });
    for (const testCase of fixture.envelopes) {
      const manifest = validateRegistrySignedManifest(testCase.manifest);
      const payload = registrySignedManifestPayload(manifest);
      expect(payload, testCase.name).toBe(testCase.signing_payload);
      const signature = Buffer.from(manifest.signature.value.slice("base64:".length), "base64url");
      expect(verify(null, Buffer.from(payload, "utf8"), key, signature), testCase.name).toBe(true);
      expect(registryContentDigest(testCase.files), testCase.name).toBe(manifest.content_digest);
    }
  });

  it("signs only its own kind, subject and version", () => {
    for (const testCase of fixture.subject_bindings) {
      const envelope = fixture.envelopes.find((candidate) => candidate.name === testCase.envelope);
      const manifest = validateRegistrySignedManifest(envelope?.manifest);
      expect(registrySignedManifestSigns(manifest, testCase.subject, testCase.version), testCase.name).toBe(testCase.signs);
    }
  });

  it("refuses every malformed term and any field the contract does not name", () => {
    for (const testCase of fixture.invalid_envelopes) expect(() => validateRegistrySignedManifest(testCase.manifest), testCase.name).toThrow();
    expect(() => validateRegistrySignedManifest(fixture.unknown_field_envelope)).toThrow("no field digest");
  });
});
