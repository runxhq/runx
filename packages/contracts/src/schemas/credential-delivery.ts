import { Type, type Static } from "../internal.js";
import {
  JSON_SCHEMA_DRAFT_2020_12,
  RUNX_CONTRACT_IDS,
  RUNX_LOGICAL_SCHEMAS,
  type DeepReadonly,
  dateTimeStringSchema,
  generatedSchema,
  stringEnum,
  validateContractSchema,
} from "../internal.js";
import { referenceSchema } from "./spine.js";

const credentialDeliveryModes = ["process_env"] as const;
const credentialDeliveryPurposes = [
  "provider_api",
  "registry",
  "artifact_store",
  "webhook_verification",
] as const;
const credentialMaterialRoles = [
  "personal_token",
  "api_key",
  "client_secret",
  "session_token",
] as const;
const credentialDeliveryObservationStatuses = [
  "delivered",
  "denied",
  "not_delivered",
] as const;

export const credentialDeliveryModeSchema = stringEnum(credentialDeliveryModes);
export const credentialDeliveryPurposeSchema = stringEnum(credentialDeliveryPurposes);
export const credentialMaterialRoleSchema = stringEnum(credentialMaterialRoles);
export const credentialDeliveryObservationStatusSchema = stringEnum(
  credentialDeliveryObservationStatuses,
);

const credentialDeliveryObservationV1TypeSchema = Type.Object(
  {
    schema: Type.Literal(RUNX_LOGICAL_SCHEMAS.credentialDeliveryObservation),
    observation_id: Type.String({ minLength: 1 }),
    request_id: Type.String({ minLength: 1 }),
    response_id: Type.Optional(Type.String({ minLength: 1 })),
    status: credentialDeliveryObservationStatusSchema,
    harness_ref: referenceSchema,
    host_ref: Type.Optional(referenceSchema),
    profile_id: Type.String({ minLength: 1 }),
    provider: Type.String({ minLength: 1 }),
    purpose: credentialDeliveryPurposeSchema,
    delivery_mode: Type.Optional(credentialDeliveryModeSchema),
    credential_refs: Type.Array(referenceSchema),
    material_ref_hash: Type.Optional(Type.String({ minLength: 1 })),
    delivered_roles: Type.Array(credentialMaterialRoleSchema),
    redaction_refs: Type.Optional(Type.Array(referenceSchema)),
    observed_at: dateTimeStringSchema(),
  },
  {
    $schema: JSON_SCHEMA_DRAFT_2020_12,
    $id: RUNX_CONTRACT_IDS.credentialDeliveryObservation,
    "x-runx-schema": RUNX_LOGICAL_SCHEMAS.credentialDeliveryObservation,
    additionalProperties: false,
  },
);

export type CredentialDeliveryObservationContract =
  DeepReadonly<Static<typeof credentialDeliveryObservationV1TypeSchema>>;

export const credentialDeliveryObservationV1Schema =
  generatedSchema<CredentialDeliveryObservationContract>(
    "credential-delivery-observation.schema.json",
  );

export function validateCredentialDeliveryObservationContract(
  value: unknown,
  label = "credential_delivery_observation",
): CredentialDeliveryObservationContract {
  return validateContractSchema(credentialDeliveryObservationV1Schema, value, label);
}
