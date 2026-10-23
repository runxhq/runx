import { generatedSchema, validateContractSchema } from "../internal.js";
import type {
  RunxX402PurchaseExtensionInfoContract,
  X402PaymentPayloadContract,
  X402PaymentRequiredContract,
  X402PaymentRequirementsContract,
  X402ResourceInfoContract,
  X402SettleResponseContract,
} from "../generated/contract-types.js";

/** The info an x402 discovery declares for one offer. */
export type RunxX402DiscoveryExtensionInfoContract = Extract<
  RunxX402PurchaseExtensionInfoContract,
  { readonly purpose: "discovery" }
>;

/**
 * The standard x402 extension declaration carrying Runx-owned strict info.
 * The TS presentation assembles it; its `info` is the Rust-owned contract.
 */
export type RunxX402PurchaseExtensionContract = {
  readonly info: RunxX402PurchaseExtensionInfoContract;
  readonly schema: Readonly<Record<string, unknown>>;
};

export const X402_PROTOCOL_VERSION = 2 as const;
export const X402_UPSTREAM_PACKAGE = "@x402/core" as const;
export const X402_UPSTREAM_PACKAGE_VERSION = "2.27.0" as const;
export const X402_UPSTREAM_COMMIT = "71eb9a55e081e7b81ba3046d0bd17c3eb9c7bf81" as const;

export const X402_SCHEMA_IDS = {
  resourceInfo: "https://schemas.runx.ai/external/x402/v2/resource-info.schema.json",
  paymentRequirements: "https://schemas.runx.ai/external/x402/v2/payment-requirements.schema.json",
  paymentRequired: "https://schemas.runx.ai/external/x402/v2/payment-required.schema.json",
  paymentPayload: "https://schemas.runx.ai/external/x402/v2/payment-payload.schema.json",
  settleResponse: "https://schemas.runx.ai/external/x402/v2/settle-response.schema.json",
  runxPurchaseExtension: "https://schemas.runx.ai/runx/x402/purchase-extension/v1.json",
} as const;

export const x402ResourceInfoV2Schema =
  generatedSchema<X402ResourceInfoContract>("x402-v2-resource-info.schema.json");
export const x402PaymentRequirementsV2Schema =
  generatedSchema<X402PaymentRequirementsContract>("x402-v2-payment-requirements.schema.json");
export const x402PaymentRequiredV2Schema =
  generatedSchema<X402PaymentRequiredContract>("x402-v2-payment-required.schema.json");
export const x402PaymentPayloadV2Schema =
  generatedSchema<X402PaymentPayloadContract>("x402-v2-payment-payload.schema.json");
export const x402SettleResponseV2Schema =
  generatedSchema<X402SettleResponseContract>("x402-v2-settle-response.schema.json");
export const runxX402PurchaseExtensionInfoV1Schema =
  generatedSchema<RunxX402PurchaseExtensionInfoContract>("runx-x402-purchase-extension-v1.schema.json");

export function validateX402ResourceInfoContract(value: unknown, label = "x402.resource") {
  return validateContractSchema(x402ResourceInfoV2Schema, value, label);
}

export function validateX402PaymentRequirementsContract(
  value: unknown,
  label = "x402.paymentRequirements",
) {
  return validateContractSchema(x402PaymentRequirementsV2Schema, value, label);
}

export function validateX402PaymentRequiredContract(value: unknown, label = "x402.paymentRequired") {
  return validateContractSchema(x402PaymentRequiredV2Schema, value, label);
}

export function validateX402PaymentPayloadContract(value: unknown, label = "x402.paymentPayload") {
  return validateContractSchema(x402PaymentPayloadV2Schema, value, label);
}

export function validateX402SettleResponseContract(value: unknown, label = "x402.settleResponse") {
  return validateContractSchema(x402SettleResponseV2Schema, value, label);
}

export function validateRunxX402PurchaseExtensionInfoContract(
  value: unknown,
  label = "runx.purchase.info",
) {
  return validateContractSchema(runxX402PurchaseExtensionInfoV1Schema, value, label);
}
