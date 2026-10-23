import {
  generatedSchema,
  generatedSchemaAt,
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
export const receiptSubjectSchema = generatedSchemaAt<SubjectContract>(
  receiptV1Schema,
  ["properties", "subject"],
  "receipt.subject",
);
export const receiptEnforcementSchema = generatedSchemaAt<ReceiptEnforcementContract>(
  receiptV1Schema,
  ["properties", "authority", "properties", "enforcement"],
  "receipt.authority.enforcement",
);
export const receiptAuthoritySchema = generatedSchemaAt<ReceiptAuthorityContract>(
  receiptV1Schema,
  ["properties", "authority"],
  "receipt.authority",
);
export const receiptCriterionSchema = generatedSchemaAt<CriterionBindingContract>(
  receiptV1Schema,
  ["properties", "seal", "properties", "criteria", "items"],
  "receipt.seal.criteria[]",
);
export const receiptActSchema = generatedSchemaAt<ReceiptActContract>(
  receiptV1Schema,
  ["properties", "acts", "items"],
  "receipt.acts[]",
);
export const receiptSealSchema = generatedSchemaAt<SealContract>(
  receiptV1Schema,
  ["properties", "seal"],
  "receipt.seal",
);
export const receiptLineageSchema = generatedSchemaAt<LineageContract>(
  receiptV1Schema,
  ["properties", "lineage"],
  "receipt.lineage",
);

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
