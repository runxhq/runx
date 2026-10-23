import { contractSchemaMatches } from "../internal.js";
import { describe, expect, it } from "vitest";

import type { CredentialDeliveryObservationContract } from "../generated/contract-types.js";
import { credentialDeliveryObservationV1Schema } from "./credential-delivery.js";

const credentialRef = { type: "credential", uri: "runx:credential:github-installation-1" } as const;

const observation: CredentialDeliveryObservationContract = {
  schema: "runx.credential_delivery.observation.v1",
  observation_id: "cred_obs_1",
  request_id: "cred_req_1",
  status: "delivered",
  harness_ref: { type: "harness", uri: "runx:harness:credential-smoke" },
  host_ref: { type: "host", uri: "runx:host:local" },
  profile_id: "github-provider-api-env",
  provider: "github",
  purpose: "provider_api",
  delivery_mode: "process_env",
  credential_refs: [credentialRef],
  material_id_digest: "sha256:4ab3",
  delivered_roles: ["api_key"],
  redaction_refs: [{ type: "redaction_policy", uri: "runx:redaction-policy:credentials-v1" }],
  observed_at: "2026-05-22T00:30:02Z",
};

describe("credential-delivery schemas", () => {
  it("accepts a receipt-safe observation", () => {
    expect(contractSchemaMatches(credentialDeliveryObservationV1Schema, observation)).toBe(true);
  });

  it("rejects raw secret-like fields on an observation", () => {
    expect(contractSchemaMatches(credentialDeliveryObservationV1Schema, {
      ...observation,
      api_key: "sk-contract-test",
    })).toBe(false);
  });

  it("rejects non-env delivery modes until a future contract explicitly adds them", () => {
    expect(contractSchemaMatches(credentialDeliveryObservationV1Schema, {
      ...observation,
      delivery_mode: "file",
    })).toBe(false);
  });
});
