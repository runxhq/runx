import {
  type UnknownRecord,
  generatedSchema,
  validateContractSchema,
} from "../internal.js";

// Rust owns the auxiliary wire contract. This binding deliberately consumes
// the generated schema instead of maintaining a second TypeBox definition.
export type RegistryBindingContract = UnknownRecord;

export const registryBindingSchema =
  generatedSchema<RegistryBindingContract>("registry-binding.schema.json");

export function validateRegistryBindingContract(value: unknown, label = "registry_binding"): RegistryBindingContract {
  return validateContractSchema(registryBindingSchema, value, label);
}
