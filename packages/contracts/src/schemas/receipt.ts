import {
  generatedSchema,
  generatedDefinition,
  validateContractSchema,
} from "../internal.js";
import type {
  CriterionBindingContract,
  LineageContract,
  ReceiptActContract,
  ReceiptAuthorityContract,
  ReceiptContract,
  ReceiptEnforcementContract,
  SealContract,
  SubjectContract,
} from "../generated/contract-types.js";

/**
 * runx.receipt.v1 — the single signed governance receipt.
 *
 * Its schema and types are generated from Rust. This module keeps the schema
 * accessors and validators TS consumers use.
 */

export type {
  ExecutionBoundaryKindContract,
  ExecutionBoundaryObservationContract,
  LineageContract,
  OperationIntentContract,
  ReceiptActContract,
  ReceiptAuthorityContract,
  ReceiptCanonicalizationContract,
  ReceiptContract,
  ReceiptEnforcementContract,
  ReceiptIssuerContract,
  ReceiptIssuerTypeContract,
  ReceiptSignatureContract,
  SealContract,
  SealReasonContract,
  SubjectContract,
  SubjectKindContract,
} from "../generated/contract-types.js";

/** The canonicalization byte contract this receipt's digest commits under. */
export const RECEIPT_CANONICALIZATION = "runx.receipt.c14n.v1" as const;

export const receiptV1Schema = generatedSchema<ReceiptContract>("receipt.schema.json");
export const receiptSubjectSchema = generatedDefinition<SubjectContract>("Subject");
export const receiptEnforcementSchema = generatedDefinition<ReceiptEnforcementContract>("ReceiptEnforcement");
export const receiptAuthoritySchema = generatedDefinition<ReceiptAuthorityContract>("ReceiptAuthority");
export const receiptCriterionSchema = generatedDefinition<CriterionBindingContract>("CriterionBinding");
export const receiptActSchema = generatedDefinition<ReceiptActContract>("ReceiptAct");
export const receiptSealSchema = generatedDefinition<SealContract>("Seal");
export const receiptLineageSchema = generatedDefinition<LineageContract>("Lineage");

export function validateReceiptContract(value: unknown, label = "receipt"): ReceiptContract {
  return validateContractSchema(receiptV1Schema, value, label) as ReceiptContract;
}

/**
 * The one rule every verifier of a runx receipt tree applies, as
 * runx-receipts states it: a root and its descendants, each committed once by
 * its parent's signed lineage, at most this many receipts (the root included)
 * and this many levels below the root.
 */
export const RECEIPT_TREE_LIMITS = Object.freeze({ maxReceipts: 1024, maxDepth: 64 });
