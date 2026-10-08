import { contractSchemaMatches } from "../internal.js";
import { describe, expect, it } from "vitest";

import {
  authorityProofSchema,
  authorityProofSchemaVersion,
  credentialEnvelopeSchema,
  scopeAdmissionSchema,
  type AuthorityProofContract,
  type CredentialEnvelopeContract,
  type ScopeAdmissionContract,
} from "./credentials.js";

const validScopeAdmission: ScopeAdmissionContract = {
  status: "allow",
  requested_scopes: ["repo:read"],
  granted_scopes: ["repo:read", "user:read"],
  grant_id: "grant_1",
  decision_summary: "matching active grant admitted",
};

const validCredentialEnvelope: CredentialEnvelopeContract = {
  kind: "runx.credential-envelope.v1",
  grant_id: "grant_1",
  provider: "github",
  auth_mode: "api_key",
  material_kind: "api_key",
  provider_reference: "local_per_run",
  scopes: ["repo:read"],
  grant_reference: {
    grant_id: "grant_1",
    scope_family: "github_repo",
    authority_kind: "constructive",
    target_repo: "runxhq/runx",
  },
  material_ref: "local:github:grant_1",
};

const validAuthorityProof: AuthorityProofContract = {
  schema_version: authorityProofSchemaVersion,
  run_id: "rx_abc",
  skill_name: "connected-review",
  source_type: "agent-task",
  requested: {
    connected_auth: true,
    scopes: ["repo:read"],
    scope_family: "github_repo",
    authority_kind: "constructive",
    target_repo: "runxhq/runx",
  },
  scope_admission: validScopeAdmission,
  credential_material: {
    status: "resolved",
    grant_id: "grant_1",
    provider: "github",
    provider_reference: "local_per_run",
    scopes: ["repo:read"],
    grant_reference: validCredentialEnvelope.grant_reference,
    material_ref_hash: "sha256-ref",
    scope_family: "github_repo",
    authority_kind: "constructive",
    target_repo: "runxhq/runx",
  },
  execution_boundary: {
    kind: "remote_provider",
  },
  redaction: {
    status: "applied",
    secret_material: "omitted",
    stdout: "hashed",
    stderr: "hashed",
    metadata_secret_keys: ["token-like metadata keys", "api-key-like metadata keys"],
  },
};

describe("credential and authority proof schemas", () => {
  it("accepts scoped credential envelopes and scope admissions", () => {
    expect(contractSchemaMatches(credentialEnvelopeSchema, validCredentialEnvelope)).toBe(true);
    expect(contractSchemaMatches(scopeAdmissionSchema, validScopeAdmission)).toBe(true);
  });

  it("accepts a complete authority proof without raw secret material", () => {
    expect(contractSchemaMatches(authorityProofSchema, validAuthorityProof)).toBe(true);
    expect(JSON.stringify(validAuthorityProof)).not.toContain("sk-contract-test");
    expect(JSON.stringify(validAuthorityProof)).not.toContain("super-secret-token");
  });

  it("rejects unknown authority proof fields", () => {
    expect(contractSchemaMatches(authorityProofSchema, { ...validAuthorityProof, raw_token: "secret" })).toBe(false);
  });

  it("rejects raw secret-like fields inside credential material", () => {
    expect(
      contractSchemaMatches(authorityProofSchema, {
        ...validAuthorityProof,
        credential_material: {
          ...validAuthorityProof.credential_material,
          access_token: "secret",
        },
      }),
    ).toBe(false);
  });
});
