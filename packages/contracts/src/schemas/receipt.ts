import {
  type DeepReadonly,
  type UnknownRecord,
  generatedSchema,
  generatedSchemaAt,
  validateContractSchema,
} from "../internal.js";
import type {
  ActFormContract,
  ClosureDispositionContract,
  CriterionBindingContract,
  DecisionContract,
  IntentContract,
  ReceiptIssuerContract,
  ReceiptSignatureContract,
  ReferenceContract,
} from "./spine.js";

/**
 * runx.receipt.v1 — the single signed governance receipt.
 *
 * The schema object below is generated from Rust. This TypeScript module keeps
 * only the public type facade and validators used by TS consumers.
 */

/** The canonicalization byte contract this receipt's digest commits under. */
export const RECEIPT_CANONICALIZATION = "runx.receipt.c14n.v1" as const;

/**
 * What ran, and the digest of its outcome: whether it succeeded, the outputs
 * it declared and its direct children's receipt ids.
 */
export type ReceiptSubjectContract = DeepReadonly<{
  kind: string;
  ref: ReferenceContract;
  outcome: string;
}>;

export type ReceiptEnforcementContract = DeepReadonly<{
  execution_boundary?: { readonly kind: string };
}>;

export type ReceiptAuthorityContract = DeepReadonly<{
  actor_ref: ReferenceContract;
  grant_refs: readonly ReferenceContract[];
  scope_refs: readonly ReferenceContract[];
  mandate_ref?: ReferenceContract;
  terms: readonly UnknownRecord[];
  derivations: readonly UnknownRecord[];
  enforcement: ReceiptEnforcementContract;
}>;

export type ReceiptCriterionContract = CriterionBindingContract;

export type ReceiptActContract = DeepReadonly<{
  id: string;
  form: ActFormContract;
  intent: IntentContract;
  /** What the caller told a reader about this call, when it said anything. */
  reason?: string;
  criterion_bindings: readonly CriterionBindingContract[];
  target_refs: readonly ReferenceContract[];
  artifact_refs: readonly ReferenceContract[];
  context_ref?: ReferenceContract;
}>;

export type ReceiptSealContract = DeepReadonly<{
  disposition: ClosureDispositionContract;
  reason_code: string;
  closed_at: string;
  criteria: readonly ReceiptCriterionContract[];
}>;

export type ReceiptLineageContract = DeepReadonly<{
  previous?: ReferenceContract;
  children: readonly ReferenceContract[];
}>;

export type ReceiptContract = DeepReadonly<{
  schema: "runx.receipt.v1";
  id: string;
  created_at: string;
  canonicalization: typeof RECEIPT_CANONICALIZATION;
  issuer: ReceiptIssuerContract;
  signature: ReceiptSignatureContract;
  digest: string;
  subject: ReceiptSubjectContract;
  authority: ReceiptAuthorityContract;
  decisions: readonly DecisionContract[];
  acts: readonly ReceiptActContract[];
  seal: ReceiptSealContract;
  lineage: ReceiptLineageContract;
  metadata?: UnknownRecord;
}>;

export const receiptV1Schema = generatedSchema<ReceiptContract>("receipt.schema.json");
export const receiptSubjectSchema = generatedSchemaAt<ReceiptSubjectContract>(
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
export const receiptCriterionSchema = generatedSchemaAt<ReceiptCriterionContract>(
  receiptV1Schema,
  ["properties", "seal", "properties", "criteria", "items"],
  "receipt.seal.criteria[]",
);
export const receiptActSchema = generatedSchemaAt<ReceiptActContract>(
  receiptV1Schema,
  ["properties", "acts", "items"],
  "receipt.acts[]",
);
export const receiptSealSchema = generatedSchemaAt<ReceiptSealContract>(
  receiptV1Schema,
  ["properties", "seal"],
  "receipt.seal",
);
export const receiptLineageSchema = generatedSchemaAt<ReceiptLineageContract>(
  receiptV1Schema,
  ["properties", "lineage"],
  "receipt.lineage",
);

export function validateReceiptContract(value: unknown, label = "receipt"): ReceiptContract {
  return validateContractSchema(receiptV1Schema, value, label) as ReceiptContract;
}
