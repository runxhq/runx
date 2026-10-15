import {
  type DeepReadonly,
  generatedSchema,
  generatedSchemaAt,
  validatePrecompiledContract,
  validateContractSchema,
} from "../internal.js";
import validatePaidSkillListing from "../generated/paid-skill-listing-validator.js";
import { paidSkillClaimProblem, paidSkillPriceProblem } from "../paid-skill-price.js";
import { RECEIPT_CANONICALIZATION } from "./receipt.js";
import type { ReferenceContract } from "./spine.js";

declare const runxPrincipalIdBrand: unique symbol;

/** A strict hosted-auth identifier, not a generic principal-reference URI. */
export type RunxPrincipalId = string & {
  readonly [runxPrincipalIdBrand]: true;
};

export type Sha256DigestContract = `sha256:${string}`;
export type CurrencyCodeContract = string;
export type SettlementFamilyContract = string;
export type PaidInvocationCanonicalizerVersionContract =
  typeof RECEIPT_CANONICALIZATION;
export type PaidInvocationPaymentStateContract =
  | "unpaid"
  | "settling"
  | "settled"
  | "refunded";
export type PaidInvocationExecutionStateContract =
  | "unstarted"
  | "queued"
  | "running"
  | "waiting_external"
  | "cancelling"
  | "succeeded"
  | "failed"
  | "cancelled";
export type PaidInvocationOutcomeGateContract =
  | "open"
  | "fulfilment_won"
  | "refund_won";
export type PaidInvocationRefusalCodeContract =
  | "offer_unavailable"
  | "quote_expired"
  | "terms_changed"
  | "replay_conflict"
  | "payment_not_authorized"
  | "capacity_unavailable"
  | "not_found"
  | "cancellation_not_available";

export type PrincipalReferenceContract = DeepReadonly<
  Omit<ReferenceContract, "type"> & { readonly type: "principal" }
>;
export type PaymentReferenceContract = ReferenceContract;
export type PaidInvocationRunReferenceContract = DeepReadonly<
  Omit<ReferenceContract, "type" | "uri"> & {
    readonly type: "act";
    readonly uri: `runx:run:${string}`;
  }
>;

export type PaymentIdempotencyBindingContract = DeepReadonly<{
  key: string;
  binding_digest: Sha256DigestContract;
}>;

export type OfferRevisionRefContract = DeepReadonly<{
  offer_id: string;
  revision: string;
  revision_digest: Sha256DigestContract;
  input_schema_digest: Sha256DigestContract;
  output_schema_digest: Sha256DigestContract;
}>;

export type PaidSkillExecutorBindingContract = DeepReadonly<{
  skill: string;
  runner: string;
  package_digest: Sha256DigestContract;
  execution_closure_digest: Sha256DigestContract;
}>;

/** A selector value: a string, a portable integer or a boolean. */
export type PaidSkillPriceChoiceContract = string | number | boolean;

/**
 * How an offer prices one call: from the listing and the inputs the runner
 * receives, never from the runner. `paidSkillPriceAmount` computes it.
 */
export type PaidSkillPriceContract = DeepReadonly<
  | { model: "fixed"; amount_minor: number }
  | {
      model: "input_choice";
      selectors: string[];
      cases: { when: PaidSkillPriceChoiceContract[]; amount_minor: number }[];
    }
  | {
      model: "artifact_bytes";
      artifact_input: string;
      base_amount_minor: number;
      unit_bytes: number;
      unit_amount_minor: number;
      max_bytes: number;
    }
>;

/**
 * Where a buyer claims what a fulfilled purchase bought but runx never holds:
 * https on the seller's fixed host, path parameters bound to JSON pointers
 * into the runner's public result. `resolvePaidSkillClaim` fills it.
 */
export type PaidSkillClaimContract = DeepReadonly<{
  url: string;
  bind?: Record<string, string>;
}>;

export type PaidSkillOfferTermsContract = DeepReadonly<{
  price: PaidSkillPriceContract;
  currency: CurrencyCodeContract;
  accepted_settlement_families: readonly SettlementFamilyContract[];
  input_schema_digest: Sha256DigestContract;
  output_schema_digest: Sha256DigestContract;
  claim?: PaidSkillClaimContract;
}>;

export type PaidSkillRunnerOfferContract = DeepReadonly<{
  offer_revision: OfferRevisionRefContract;
  price: PaidSkillPriceContract;
  currency: CurrencyCodeContract;
  accepted_settlement_families: readonly SettlementFamilyContract[];
  claim?: PaidSkillClaimContract;
}>;

export type PaidSkillListingContract = DeepReadonly<{
  skill_id: string;
  version: string;
  skill_digest: Sha256DigestContract;
  profile_digest: Sha256DigestContract;
  package_digest: Sha256DigestContract;
  vendor_ref: PrincipalReferenceContract;
  offers: Readonly<Record<string, PaidSkillRunnerOfferContract>>;
}>;

export type PaidInvocationContract = DeepReadonly<{
  invocation_id: string;
  principal: PrincipalReferenceContract;
  vendor_ref: PrincipalReferenceContract;
  counterparty: PaymentReferenceContract;
  offer_revision: OfferRevisionRefContract;
  package_digest: Sha256DigestContract;
  input_digest: Sha256DigestContract;
  canonicalizer_version: PaidInvocationCanonicalizerVersionContract;
  amount_minor: number;
  currency: CurrencyCodeContract;
  accepted_settlement_families: readonly SettlementFamilyContract[];
  settlement_family?: SettlementFamilyContract;
  idempotency: PaymentIdempotencyBindingContract;
  /** The key the purchaser claims with: `ed25519:` and 32 bytes of base64url. */
  purchaser_key?: string;
  expires_at: string;
  payment_state: PaidInvocationPaymentStateContract;
  execution_state: PaidInvocationExecutionStateContract;
  outcome_gate: PaidInvocationOutcomeGateContract;
  execution_ref?: ReferenceContract;
  external_job_ref?: ReferenceContract;
  payment_ref?: PaymentReferenceContract;
  created_at: string;
  updated_at: string;
}>;

export type PaidInvocationPaymentChallengeContract = DeepReadonly<{
  settlement_family: SettlementFamilyContract;
  protocol_version: string;
  media_type: string;
  payload: unknown;
  payload_digest: Sha256DigestContract;
  quote_ref: ReferenceContract;
  quote_expires_at: string;
}>;

export type QuotePaidInvocationRequestContract = DeepReadonly<{
  principal: PrincipalReferenceContract;
  vendor_ref: PrincipalReferenceContract;
  counterparty: PaymentReferenceContract;
  offer_revision: OfferRevisionRefContract;
  package_digest: Sha256DigestContract;
  input_digest: Sha256DigestContract;
  canonicalizer_version: PaidInvocationCanonicalizerVersionContract;
  amount_minor: number;
  currency: CurrencyCodeContract;
  accepted_settlement_families: readonly SettlementFamilyContract[];
  idempotency: PaymentIdempotencyBindingContract;
  /** The key the purchaser claims with: `ed25519:` and 32 bytes of base64url. */
  purchaser_key?: string;
}>;

export type PaidInvocationRefusalContract = DeepReadonly<{
  status: "refused";
  code: PaidInvocationRefusalCodeContract;
  reason: string;
}>;

export type QuotePaidInvocationResultContract =
  | DeepReadonly<{
      status: "admitted";
      value: {
        invocation: PaidInvocationContract;
        challenge: PaidInvocationPaymentChallengeContract;
      };
    }>
  | PaidInvocationRefusalContract;

export type ExecutePaidInvocationRequestContract = DeepReadonly<{
  invocation_id: string;
  settlement_family: SettlementFamilyContract;
  payment_ref: PaymentReferenceContract;
  idempotency: PaymentIdempotencyBindingContract;
}>;

export type PaidInvocationAdmissionResultContract =
  | DeepReadonly<{
      status: "admitted";
      value: { invocation: PaidInvocationContract };
    }>
  | PaidInvocationRefusalContract;

export type ExecutePaidInvocationResultContract = PaidInvocationAdmissionResultContract;
export type GetPaidInvocationRequestContract = DeepReadonly<{ invocation_id: string }>;
export type PaidInvocationFailureContract = DeepReadonly<{ code: string; message: string }>;
export type GetPaidInvocationResultContract =
  | DeepReadonly<{
      status: "admitted";
      value: {
        invocation: PaidInvocationContract;
        run_ref?: PaidInvocationRunReferenceContract;
        receipt_ref?: ReferenceContract;
        result_ref?: ReferenceContract;
        failure?: PaidInvocationFailureContract;
      };
    }>
  | PaidInvocationRefusalContract;
/**
 * What a purchaser reads of its purchase (K13): the paid invocation, the
 * purchase receipt once sealed, and the runner's public result once
 * fulfilment won. Never the seller's run, step outputs or grants.
 */
export type PurchaseContract = DeepReadonly<{
  invocation: PaidInvocationContract;
  /** runx's signed purchase receipt, once sealed: what a claim presents. */
  receipt?: Record<string, unknown>;
  result?: unknown;
  claim_url?: string;
  failure?: PaidInvocationFailureContract;
}>;
export type CancelPaidInvocationRequestContract = DeepReadonly<{
  invocation_id: string;
  idempotency: PaymentIdempotencyBindingContract;
}>;
export type CancelPaidInvocationResultContract = PaidInvocationAdmissionResultContract;

export const paidInvocationV1Schema =
  generatedSchema<PaidInvocationContract>("paid-invocation.schema.json");
export const paidSkillListingV1Schema =
  generatedSchema<PaidSkillListingContract>("paid-skill-listing.schema.json");
export const offerRevisionRefV1Schema =
  generatedSchema<OfferRevisionRefContract>("offer-revision-ref.schema.json");
export const quotePaidInvocationRequestV1Schema =
  generatedSchema<QuotePaidInvocationRequestContract>("quote-paid-invocation-request.schema.json");
export const quotePaidInvocationResultV1Schema =
  generatedSchema<QuotePaidInvocationResultContract>("quote-paid-invocation-result.schema.json");
export const executePaidInvocationRequestV1Schema =
  generatedSchema<ExecutePaidInvocationRequestContract>("execute-paid-invocation-request.schema.json");
export const executePaidInvocationResultV1Schema =
  generatedSchema<ExecutePaidInvocationResultContract>("execute-paid-invocation-result.schema.json");
export const getPaidInvocationRequestV1Schema =
  generatedSchema<GetPaidInvocationRequestContract>("get-paid-invocation-request.schema.json");
export const getPaidInvocationResultV1Schema =
  generatedSchema<GetPaidInvocationResultContract>("get-paid-invocation-result.schema.json");
export const purchaseV1Schema =
  generatedSchema<PurchaseContract>("purchase.schema.json");
export const cancelPaidInvocationRequestV1Schema =
  generatedSchema<CancelPaidInvocationRequestContract>("cancel-paid-invocation-request.schema.json");
export const cancelPaidInvocationResultV1Schema =
  generatedSchema<CancelPaidInvocationResultContract>("cancel-paid-invocation-result.schema.json");

export const paymentIdempotencyBindingSchema =
  generatedSchemaAt<PaymentIdempotencyBindingContract>(
    quotePaidInvocationRequestV1Schema,
    ["properties", "idempotency"],
    "quote_paid_invocation_request.idempotency",
  );
export const paidInvocationPaymentChallengeSchema =
  generatedSchemaAt<PaidInvocationPaymentChallengeContract>(
    quotePaidInvocationResultV1Schema,
    ["$defs", "PaidInvocationPaymentChallenge"],
    "quote_paid_invocation_result.$defs.PaidInvocationPaymentChallenge",
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

export function validatePaidInvocationContract(value: unknown, label = "paid_invocation") {
  const invocation = validateContractSchema(paidInvocationV1Schema, value, label);
  if (invocation.external_job_ref
    && (invocation.external_job_ref.type !== "target"
      || !invocation.external_job_ref.uri.startsWith("runx:external-job:"))) {
    throw new Error(`${label}.external_job_ref must identify a Runx external job target.`);
  }
  if (invocation.execution_state === "waiting_external" && !invocation.external_job_ref) {
    throw new Error(`${label}.waiting_external requires external_job_ref.`);
  }
  return invocation;
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
    const claimProblem = offer.claim ? paidSkillClaimProblem(offer.claim) : undefined;
    if (claimProblem) throw new Error(`${label}.offers.${runner}.claim: ${claimProblem}.`);
  }
  return listing;
}

export function validateOfferRevisionRefContract(value: unknown, label = "offer_revision") {
  return validateContractSchema(offerRevisionRefV1Schema, value, label);
}

export function validatePaymentIdempotencyBindingContract(value: unknown, label = "idempotency") {
  return validateContractSchema(paymentIdempotencyBindingSchema, value, label);
}

export function validatePaidInvocationPaymentChallengeContract(value: unknown, label = "challenge") {
  return validateContractSchema(paidInvocationPaymentChallengeSchema, value, label);
}

export function validateQuotePaidInvocationRequestContract(value: unknown, label = "request") {
  return validateContractSchema(quotePaidInvocationRequestV1Schema, value, label);
}

export function validateQuotePaidInvocationResultContract(value: unknown, label = "result") {
  return validateContractSchema(quotePaidInvocationResultV1Schema, value, label);
}

export function validateExecutePaidInvocationRequestContract(value: unknown, label = "request") {
  return validateContractSchema(executePaidInvocationRequestV1Schema, value, label);
}

export function validateExecutePaidInvocationResultContract(value: unknown, label = "result") {
  return validateContractSchema(executePaidInvocationResultV1Schema, value, label);
}

export function validateGetPaidInvocationRequestContract(value: unknown, label = "request") {
  return validateContractSchema(getPaidInvocationRequestV1Schema, value, label);
}

export function validateGetPaidInvocationResultContract(value: unknown, label = "result") {
  return validateContractSchema(getPaidInvocationResultV1Schema, value, label);
}

export function validatePurchaseContract(value: unknown, label = "purchase") {
  return validateContractSchema(purchaseV1Schema, value, label);
}

export function validateCancelPaidInvocationRequestContract(value: unknown, label = "request") {
  return validateContractSchema(cancelPaidInvocationRequestV1Schema, value, label);
}

export function validateCancelPaidInvocationResultContract(value: unknown, label = "result") {
  return validateContractSchema(cancelPaidInvocationResultV1Schema, value, label);
}
