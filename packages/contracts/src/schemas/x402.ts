import {
  type DeepReadonly,
  generatedSchema,
  validateContractSchema,
} from "../internal.js";
import type {
  OfferRevisionRefContract,
  PaidInvocationCanonicalizerVersionContract,
  ParentInvocationBindingContract,
  PaymentIdempotencyBindingContract,
  Sha256DigestContract,
} from "./paid-invocation.js";
import type { ReferenceContract } from "./spine.js";

type ExternalObject = DeepReadonly<Record<string, unknown>>;

/** x402 is an external protocol. Readers preserve fields added by its owners. */
export type X402ResourceInfoContract = ExternalObject & DeepReadonly<{
  url: string;
  description?: string | null;
  mimeType?: string | null;
  serviceName?: string | null;
  tags?: readonly string[] | null;
  iconUrl?: string | null;
}>;

export type X402PaymentRequirementsContract = ExternalObject & DeepReadonly<{
  scheme: string;
  network: string;
  amount: string;
  asset: string;
  payTo: string;
  maxTimeoutSeconds: number;
  extra?: Readonly<Record<string, unknown>> | null;
}>;

export type X402PaymentRequiredContract = ExternalObject & DeepReadonly<{
  x402Version: 2;
  error?: string | null;
  resource: X402ResourceInfoContract;
  accepts: readonly X402PaymentRequirementsContract[];
  extensions?: Readonly<Record<string, unknown>> | null;
}>;

export type X402PaymentPayloadContract = ExternalObject & DeepReadonly<{
  x402Version: 2;
  resource?: X402ResourceInfoContract | null;
  accepted: X402PaymentRequirementsContract;
  payload: Readonly<Record<string, unknown>>;
  extensions?: Readonly<Record<string, unknown>> | null;
}>;

export type X402SettleResponseContract = ExternalObject & DeepReadonly<{
  success: boolean;
  transaction: string;
  network: string;
  errorReason?: string | null;
  errorMessage?: string | null;
  payer?: string | null;
  amount?: string | null;
  extensions?: Readonly<Record<string, unknown>> | null;
  extra?: Readonly<Record<string, unknown>> | null;
}>;

export type RunxX402DiscoveryExtensionInfoContract = DeepReadonly<{
  purpose: "discovery";
  offer_revision: OfferRevisionRefContract;
  package_digest: Sha256DigestContract;
}>;

export type RunxX402InvocationExtensionInfoContract = DeepReadonly<{
  purpose: "invocation";
  invocation_id: string;
  quote_ref: ReferenceContract;
  offer_revision: OfferRevisionRefContract;
  package_digest: Sha256DigestContract;
  input_digest: Sha256DigestContract;
  canonicalizer_version: PaidInvocationCanonicalizerVersionContract;
  idempotency: PaymentIdempotencyBindingContract;
  parent?: ParentInvocationBindingContract;
}>;

export type RunxX402ExtensionInfoContract =
  | RunxX402DiscoveryExtensionInfoContract
  | RunxX402InvocationExtensionInfoContract;

/** Standard x402 extension declaration carrying Runx-owned strict info. */
export type RunxX402InvocationExtensionContract = DeepReadonly<{
  info: RunxX402ExtensionInfoContract;
  schema: Readonly<Record<string, unknown>>;
}>;

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
  runxInvocationExtension: "https://schemas.runx.ai/runx/x402/invocation-extension/v1.json",
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
export const runxX402InvocationExtensionInfoV1Schema =
  generatedSchema<RunxX402ExtensionInfoContract>("runx-x402-invocation-extension-v1.schema.json");

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

export function validateRunxX402InvocationExtensionInfoContract(
  value: unknown,
  label = "runx.invocation.info",
) {
  return validateContractSchema(runxX402InvocationExtensionInfoV1Schema, value, label);
}
