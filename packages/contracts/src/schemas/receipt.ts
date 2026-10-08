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
  ClosureRecordContract,
  CriterionBindingContract,
  DecisionContract,
  IntentContract,
  ReceiptIssuerContract,
  ReceiptSignatureContract,
  ReferenceContract,
  RevisionDetailsContract,
  VerificationDetailsContract,
} from "./spine.js";

/**
 * runx.receipt.v1 — the single signed governance receipt.
 *
 * The schema object below is generated from Rust. This TypeScript module keeps
 * only the public type facade and validators used by TS consumers.
 */

/** The canonicalization byte contract this receipt's digest commits under. */
export const RECEIPT_CANONICALIZATION = "runx.receipt.c14n.v1" as const;

export type ReceiptCommitmentContract = DeepReadonly<{
  scope: string;
  algorithm: string;
  value: string;
  canonicalization: string;
}>;

export type ReceiptInputContextContract = DeepReadonly<{
  source: string;
  preview: string;
  value_hash: string;
}>;

export type ReceiptClassContract = "executed" | "mediated";

export type ReceiptOfferRevisionContract = DeepReadonly<{
  offer_id: string;
  revision: string;
  revision_digest: string;
  input_schema_digest: string;
  output_schema_digest: string;
}>;

export type ReceiptParentInvocationBindingContract = DeepReadonly<{
  invocation_id: string;
  execution_digest: string;
}>;

export type ReceiptPaidInvocationMediationContract = DeepReadonly<{
  listing_ref: string;
  endpoint_url: string;
  vendor_offer_revision: DeepReadonly<{
    offer_id: string;
    revision: string;
    revision_digest: `sha256:${string}`;
    input_schema_digest: `sha256:${string}`;
    output_schema_digest: `sha256:${string}`;
  }>;
  vendor_package_digest: `sha256:${string}`;
  vendor_amount_minor: number;
  platform_fee_minor: number;
  currency: string;
  settlement_family: string;
  expected_receipt_class: "executed";
  prepared_price?: {
    readonly source_invocation_id: string;
    readonly input_digest: `sha256:${string}`;
  };
}>;

export type ReceiptPaidInvocationBindingContract = DeepReadonly<{
  invocation_id: string;
  vendor_ref: ReferenceContract;
  offer_revision: ReceiptOfferRevisionContract;
  package_digest: string;
  input_digest: string;
  mediation?: ReceiptPaidInvocationMediationContract;
  parent_binding?: ReceiptParentInvocationBindingContract;
}>;

export type ReceiptEvidenceContract = DeepReadonly<{
  type: "inner_receipt";
  receipt_ref: ReferenceContract;
  expected: ReceiptPaidInvocationBindingContract;
}>;

export type ReceiptSubjectContract = DeepReadonly<{
  kind: string;
  ref: ReferenceContract;
  input_context?: ReceiptInputContextContract;
  commitments: readonly ReceiptCommitmentContract[];
  paid_invocation?: ReceiptPaidInvocationBindingContract;
}>;

export type ReceiptEnforcementContract = DeepReadonly<{
  profile_hash: string;
  redaction_refs: readonly ReferenceContract[];
  setup_refs: readonly ReferenceContract[];
  teardown_refs: readonly ReferenceContract[];
}>;

export type ReceiptAuthorityContract = DeepReadonly<{
  actor_ref: ReferenceContract;
  grant_refs: readonly ReferenceContract[];
  scope_refs: readonly ReferenceContract[];
  authority_proof_refs: readonly ReferenceContract[];
  attenuation: UnknownRecord;
  mandate_ref?: ReferenceContract;
  terms: readonly UnknownRecord[];
  enforcement: ReceiptEnforcementContract;
}>;

export type ReceiptIdempotencyContract = DeepReadonly<{
  intent_key: string;
  trigger_fingerprint: string;
  content_hash: string;
}>;

export type ReceiptRunnerProvenanceContract = DeepReadonly<{
  provider: string | null;
  model: string | null;
  prompt_version: string | null;
}>;

export type ReceiptCriterionContract = CriterionBindingContract;

export type ReceiptActContract = DeepReadonly<{
  id: string;
  form: ActFormContract;
  intent: IntentContract;
  summary: string;
  criterion_bindings: readonly CriterionBindingContract[];
  by?: ReceiptRunnerProvenanceContract;
  source_refs: readonly ReferenceContract[];
  target_refs: readonly ReferenceContract[];
  artifact_refs: readonly ReferenceContract[];
  context_ref?: ReferenceContract;
  closure: ClosureRecordContract;
  revision?: RevisionDetailsContract;
  verification?: VerificationDetailsContract;
}>;

export type ReceiptSealContract = DeepReadonly<{
  disposition: ClosureDispositionContract;
  reason_code: string;
  summary: string;
  closed_at: string;
  last_observed_at: string;
  criteria: readonly ReceiptCriterionContract[];
}>;

export type ReceiptLineageContract = DeepReadonly<{
  parent?: ReferenceContract;
  previous?: ReferenceContract;
  children: readonly ReferenceContract[];
  resume_ref?: ReferenceContract;
}>;

export type ReceiptContract = DeepReadonly<{
  schema: string;
  id: string;
  created_at: string;
  canonicalization: string;
  issuer: ReceiptIssuerContract;
  signature: ReceiptSignatureContract;
  digest: string;
  idempotency: ReceiptIdempotencyContract;
  class: ReceiptClassContract;
  subject: ReceiptSubjectContract;
  authority: ReceiptAuthorityContract;
  signals: readonly ReferenceContract[];
  decisions: readonly DecisionContract[];
  acts: readonly ReceiptActContract[];
  evidence?: readonly ReceiptEvidenceContract[];
  seal: ReceiptSealContract;
  lineage?: ReceiptLineageContract;
  metadata?: UnknownRecord;
}>;

export const receiptV1Schema = generatedSchema<ReceiptContract>("receipt.schema.json");
export const receiptClassSchema = generatedSchemaAt<ReceiptClassContract>(
  receiptV1Schema,
  ["properties", "class"],
  "receipt.class",
);
export const receiptCommitmentSchema = generatedSchemaAt<ReceiptCommitmentContract>(
  receiptV1Schema,
  ["properties", "subject", "properties", "commitments", "items"],
  "receipt.subject.commitments[]",
);
export const receiptInputContextSchema = generatedSchemaAt<ReceiptInputContextContract>(
  receiptV1Schema,
  ["properties", "subject", "properties", "input_context"],
  "receipt.subject.input_context",
);
export const receiptSubjectSchema = generatedSchemaAt<ReceiptSubjectContract>(
  receiptV1Schema,
  ["properties", "subject"],
  "receipt.subject",
);
export const receiptPaidInvocationBindingSchema =
  generatedSchemaAt<ReceiptPaidInvocationBindingContract>(
    receiptV1Schema,
    ["properties", "subject", "properties", "paid_invocation"],
    "receipt.subject.paid_invocation",
  );
export const receiptEvidenceSchema = generatedSchemaAt<ReceiptEvidenceContract>(
  receiptV1Schema,
  ["properties", "evidence", "items"],
  "receipt.evidence[]",
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
export const receiptIdempotencySchema = generatedSchemaAt<ReceiptIdempotencyContract>(
  receiptV1Schema,
  ["properties", "idempotency"],
  "receipt.idempotency",
);
export const receiptRunnerProvenanceSchema = generatedSchemaAt<ReceiptRunnerProvenanceContract>(
  receiptV1Schema,
  ["properties", "acts", "items", "properties", "by"],
  "receipt.acts[].by",
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
