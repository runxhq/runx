import { generatedSchema, validateContractSchema } from "../internal.js";
import type { CredentialDeliveryObservationContract } from "../generated/contract-types.js";

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
