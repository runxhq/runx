import {
  generatedSchema,
  generatedSchemaAt,
  validatePrecompiledContract,
  validateContractSchema,
} from "../internal.js";
import validatePaidSkillListing from "../generated/paid-skill-listing-validator.js";
import type {
  CancelPurchaseRequestContract,
  CancelPurchaseResultContract,
  ExecutePurchaseRequestContract,
  ExecutePurchaseResultContract,
  GetPurchaseRequestContract,
  GetPurchaseResultContract,
  OfferVersionBindingContract,
  PaidSkillListingContract,
  PaymentIdempotencyBindingContract,
  PrincipalReferenceContract,
  PurchaseContract,
  PurchasePaymentChallengeContract,
  PurchasePaymentOptionBindingContract,
  PurchaseReadContract,
  QuotePurchaseRequestContract,
  QuotePurchaseResultContract,
} from "../generated/contract-types.js";
import { paidSkillActsOnProblem, paidSkillPriceProblem } from "../paid-skill-price.js";

declare const runxPrincipalIdBrand: unique symbol;

/** A strict hosted-auth identifier, not a generic principal-reference URI. */
export type RunxPrincipalId = string & {
  readonly [runxPrincipalIdBrand]: true;
};

/**
 * The settlement family runx's own ledger settles: an account buyer pays from
 * its runx balance, and the price moves inside runx.
 */
export const RUNX_BALANCE_SETTLEMENT_FAMILY = "runx_balance";

/** Resolve an exact quoted option; ambiguous or absent identities never select a default. */
export function selectPurchasePaymentOption(
  options: readonly PurchasePaymentChallengeContract[],
  selected: PurchasePaymentOptionBindingContract,
): PurchasePaymentChallengeContract | undefined {
  const matches = options.filter((option) => option.settlement_family === selected.settlement_family
    && option.payload_digest === selected.payload_digest);
  return matches.length === 1 ? matches[0] : undefined;
}

export const purchaseV1Schema =
  generatedSchema<PurchaseContract>("purchase.schema.json");
export const paidSkillListingV1Schema =
  generatedSchema<PaidSkillListingContract>("paid-skill-listing.schema.json");
export const offerVersionBindingV1Schema =
  generatedSchema<OfferVersionBindingContract>("offer-version-binding.schema.json");
export const quotePurchaseRequestV1Schema =
  generatedSchema<QuotePurchaseRequestContract>("quote-purchase-request.schema.json");
export const quotePurchaseResultV1Schema =
  generatedSchema<QuotePurchaseResultContract>("quote-purchase-result.schema.json");
/** Explicit content negotiation for the canonical quote, rather than a rail's wire projection. */
export const quotePurchaseMediaType =
  `application/json; profile="${quotePurchaseResultV1Schema.$id}"`;
export const executePurchaseRequestV1Schema =
  generatedSchema<ExecutePurchaseRequestContract>("execute-purchase-request.schema.json");
export const executePurchaseResultV1Schema =
  generatedSchema<ExecutePurchaseResultContract>("execute-purchase-result.schema.json");
export const getPurchaseRequestV1Schema =
  generatedSchema<GetPurchaseRequestContract>("get-purchase-request.schema.json");
export const getPurchaseResultV1Schema =
  generatedSchema<GetPurchaseResultContract>("get-purchase-result.schema.json");
export const purchaseReadV1Schema =
  generatedSchema<PurchaseReadContract>("purchase-read.schema.json");
export const cancelPurchaseRequestV1Schema =
  generatedSchema<CancelPurchaseRequestContract>("cancel-purchase-request.schema.json");
export const cancelPurchaseResultV1Schema =
  generatedSchema<CancelPurchaseResultContract>("cancel-purchase-result.schema.json");

export const paymentIdempotencyBindingSchema =
  generatedSchemaAt<PaymentIdempotencyBindingContract>(
    quotePurchaseRequestV1Schema,
    ["properties", "idempotency"],
    "quote_purchase_request.idempotency",
  );
export const purchasePaymentChallengeSchema =
  generatedSchemaAt<PurchasePaymentChallengeContract>(
    quotePurchaseResultV1Schema,
    ["$defs", "PurchasePaymentChallenge"],
    "quote_purchase_result.$defs.PurchasePaymentChallenge",
  );

export const purchasePaymentOptionBindingSchema =
  generatedSchemaAt<PurchasePaymentOptionBindingContract>(
    quotePurchaseResultV1Schema,
    ["$defs", "PurchasePaymentOptionBinding"],
    "quote_purchase_result.$defs.PurchasePaymentOptionBinding",
  );

const RUNX_PRINCIPAL_ID_PATTERN = /^[A-Za-z0-9][A-Za-z0-9._:-]{0,255}$/u;

export function validateRunxPrincipalId(
  value: unknown,
  label = "principal_id",
): RunxPrincipalId {
  if (typeof value !== "string" || !RUNX_PRINCIPAL_ID_PATTERN.test(value)) {
    throw new Error(
      `${label} must match ^[A-Za-z0-9][A-Za-z0-9._:-]{0,255}$.`,
    );
  }
  return value as RunxPrincipalId;
}

export function principalReferenceFromRunxPrincipalId(
  value: unknown,
  label = "principal_id",
): PrincipalReferenceContract {
  const principalId = validateRunxPrincipalId(value, label);
  return {
    type: "principal",
    uri: `runx:principal:${principalId}`,
  };
}

export function validatePurchaseContract(value: unknown, label = "purchase") {
  const purchase = validateContractSchema(purchaseV1Schema, value, label);
  if (purchase.external_job_ref
    && (purchase.external_job_ref.type !== "target"
      || !purchase.external_job_ref.uri.startsWith("runx:external-job:"))) {
    throw new Error(`${label}.external_job_ref must identify a Runx external job target.`);
  }
  if (purchase.execution_state === "waiting_external" && !purchase.external_job_ref) {
    throw new Error(`${label}.waiting_external requires external_job_ref.`);
  }
  return purchase;
}

export function validatePaidSkillListingContract(value: unknown, label = "paid_skill_listing") {
  const listing = validatePrecompiledContract(
    paidSkillListingV1Schema,
    value,
    label,
    validatePaidSkillListing,
  );
  // The invariants a schema cannot state, checked as the Rust contract does.
  for (const [runner, offer] of Object.entries(listing.offers)) {
    const priceProblem = paidSkillPriceProblem(offer.price);
    if (priceProblem) throw new Error(`${label}.offers.${runner}.price: ${priceProblem}.`);
    const actsOnProblem = offer.acts_on ? paidSkillActsOnProblem(offer.acts_on) : undefined;
    if (actsOnProblem) throw new Error(`${label}.offers.${runner}.acts_on: ${actsOnProblem}.`);
  }
  return listing;
}

export function validateOfferVersionBindingContract(value: unknown, label = "offer_version") {
  return validateContractSchema(offerVersionBindingV1Schema, value, label);
}

export function validatePaymentIdempotencyBindingContract(value: unknown, label = "idempotency") {
  return validateContractSchema(paymentIdempotencyBindingSchema, value, label);
}

export function validatePurchasePaymentChallengeContract(value: unknown, label = "challenge") {
  return validateContractSchema(purchasePaymentChallengeSchema, value, label);
}

export function validatePurchasePaymentOptionBindingContract(value: unknown, label = "payment_option") {
  return validateContractSchema(purchasePaymentOptionBindingSchema, value, label);
}

export function validateQuotePurchaseRequestContract(value: unknown, label = "request") {
  return validateContractSchema(quotePurchaseRequestV1Schema, value, label);
}

export function validateQuotePurchaseResultContract(value: unknown, label = "result") {
  return validateContractSchema(quotePurchaseResultV1Schema, value, label);
}

export function validateExecutePurchaseRequestContract(value: unknown, label = "request") {
  return validateContractSchema(executePurchaseRequestV1Schema, value, label);
}

export function validateExecutePurchaseResultContract(value: unknown, label = "result") {
  return validateContractSchema(executePurchaseResultV1Schema, value, label);
}

export function validateGetPurchaseRequestContract(value: unknown, label = "request") {
  return validateContractSchema(getPurchaseRequestV1Schema, value, label);
}

export function validateGetPurchaseResultContract(value: unknown, label = "result") {
  return validateContractSchema(getPurchaseResultV1Schema, value, label);
}

export function validatePurchaseReadContract(value: unknown, label = "purchase") {
  return validateContractSchema(purchaseReadV1Schema, value, label);
}

export function validateCancelPurchaseRequestContract(value: unknown, label = "request") {
  return validateContractSchema(cancelPurchaseRequestV1Schema, value, label);
}

export function validateCancelPurchaseResultContract(value: unknown, label = "result") {
  return validateContractSchema(cancelPurchaseResultV1Schema, value, label);
}
