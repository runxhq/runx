import {
  type JsonSchema,
  generatedSchema,
  generatedSchemaAt,
  validateContractSchema,
} from "../internal.js";
import type {
  ActAddressContract,
  ActFormContract,
  ArtifactContract,
  AuthorityApprovalContract,
  AuthorityBoundsContract,
  AuthorityResourceFamilyContract,
  AuthoritySubsetComparisonContract,
  AuthoritySubsetProofContract,
  AuthorityTermContract,
  AuthorityVerbContract,
  CriterionBindingContract,
  CriterionContract,
  CriterionStatusContract,
  DecisionChoiceContract,
  DecisionContract,
  DigestCommitmentContract,
  DuplicateCandidateContract,
  IntentContract,
  LinksContract,
  ProofKindContract,
  ReceiptActContract,
  ReceiptContract,
  ReceiptIssuerContract,
  ReceiptSignatureContract,
  ReferenceContract,
  ReferenceLinkContract,
  ReferenceTypeContract,
  RunOutcomeContract,
  SignalAuthenticityContract,
  SignalContract,
  SignalTrustLevelContract,
} from "../generated/contract-types.js";

function schemaAt<TStatic>(
  schema: JsonSchema,
  path: readonly (string | number)[],
  label: string,
): JsonSchema<TStatic> {
  return generatedSchemaAt<TStatic>(schema, path, label);
}

function enumValues(schema: JsonSchema, label: string): readonly string[] {
  const values = schema.enum;
  if (schema.type !== "string" || !Array.isArray(values)) {
    throw new Error(`generated enum fragment is not a string enum: ${label}`);
  }
  return values.map((value, index) => {
    if (typeof value !== "string") {
      throw new Error(`generated enum fragment has a non-string value: ${label}[${index}]`);
    }
    return value;
  });
}

export const referenceSchema = generatedSchema<ReferenceContract>("reference.schema.json");
export const referenceLinkSchema = generatedSchema<ReferenceLinkContract>("reference-link.schema.json");
export const signalSchema = generatedSchema<SignalContract>("signal.schema.json");
export const authoritySubsetProofSchema = generatedSchema<AuthoritySubsetProofContract>(
  "authority-subset-proof.schema.json",
);
export const authorityTermSchema = generatedSchema<AuthorityTermContract>("authority-term.schema.json");
export const decisionSchema = generatedSchema<DecisionContract>("decision.schema.json");
export const artifactSchema = generatedSchema<ArtifactContract>("artifact.schema.json");

const receiptRootSchema = generatedSchema<ReceiptContract>("receipt.schema.json");

export const referenceTypeSchema = schemaAt<ReferenceTypeContract>(
  referenceSchema,
  ["properties", "type"],
  "reference.type",
);
export const actAddressSchema = schemaAt<ActAddressContract>(
  artifactSchema,
  ["properties", "produced_by", "properties", "act"],
  "artifact.produced_by.act",
);
export const proofKindSchema = schemaAt<ProofKindContract>(
  referenceSchema,
  ["properties", "proof_kind"],
  "reference.proof_kind",
);
export const digestCommitmentSchema = schemaAt<DigestCommitmentContract>(
  artifactSchema,
  ["properties", "digest"],
  "artifact.digest",
);
export const signalTypeSchema = schemaAt<string>(
  signalSchema,
  ["properties", "signal_type"],
  "signal.signal_type",
);
export const signalAuthenticitySchema = schemaAt<SignalAuthenticityContract>(
  signalSchema,
  ["properties", "authenticity"],
  "signal.authenticity",
);
export const signalTrustLevelSchema = schemaAt<SignalTrustLevelContract>(
  signalAuthenticitySchema,
  ["properties", "trust_level"],
  "signal.authenticity.trust_level",
);
export const linksSchema = schemaAt<LinksContract>(
  signalSchema,
  ["properties", "links"],
  "signal.links",
);
export const nullableReferenceSchema = schemaAt<ReferenceContract | null>(
  linksSchema,
  ["properties", "duplicate_of"],
  "links.duplicate_of",
);
export const duplicateCandidateSchema = schemaAt<DuplicateCandidateContract>(
  linksSchema,
  ["properties", "duplicate_candidates", "items"],
  "links.duplicate_candidates[]",
);

export const authorityBoundsSchema = schemaAt<AuthorityBoundsContract>(
  authorityTermSchema,
  ["properties", "bounds"],
  "authority.terms[].bounds",
);
export const authorityResourceFamilySchema = schemaAt<AuthorityResourceFamilyContract>(
  authorityTermSchema,
  ["properties", "resource_family"],
  "authority.terms[].resource_family",
);
export const authorityVerbSchema = schemaAt<AuthorityVerbContract>(
  authorityTermSchema,
  ["properties", "verbs", "items"],
  "authority.terms[].verbs[]",
);
export const authorityApprovalSchema = schemaAt<AuthorityApprovalContract>(
  authorityTermSchema,
  ["properties", "approvals", "items"],
  "authority.terms[].approvals[]",
);
export const authoritySubsetComparisonSchema = schemaAt<AuthoritySubsetComparisonContract>(
  authoritySubsetProofSchema,
  ["properties", "compared_terms", "items"],
  "authority_subset_proof.compared_terms[]",
);
const receiptActSchema = schemaAt<ReceiptActContract>(
  receiptRootSchema,
  ["properties", "acts", "items"],
  "receipt.acts[]",
);
export const intentSchema = schemaAt<IntentContract>(
  receiptActSchema,
  ["properties", "intent"],
  "receipt.acts[].intent",
);
export const criterionBindingSchema = schemaAt<CriterionBindingContract>(
  receiptActSchema,
  ["properties", "criterion_bindings", "items"],
  "receipt.acts[].criterion_bindings[]",
);
export const criterionSchema = schemaAt<CriterionContract>(
  criterionBindingSchema,
  ["properties", "criterion_id"],
  "receipt.acts[].criterion_bindings[].criterion_id",
);
export const criterionStatusSchema = schemaAt<CriterionStatusContract>(
  criterionBindingSchema,
  ["properties", "status"],
  "receipt.acts[].criterion_bindings[].status",
);
export const runOutcomeSchema = schemaAt<RunOutcomeContract>(
  receiptRootSchema,
  ["properties", "seal", "properties", "outcome"],
  "receipt.seal.outcome",
);
export const actFormSchema = schemaAt<ActFormContract>(
  receiptActSchema,
  ["properties", "form"],
  "receipt.acts[].form",
);
export const decisionChoiceSchema = schemaAt<DecisionChoiceContract>(
  decisionSchema,
  ["properties", "choice"],
  "decision.choice",
);

export const receiptIssuerSchema = schemaAt<ReceiptIssuerContract>(
  receiptRootSchema,
  ["properties", "issuer"],
  "receipt.issuer",
);
export const receiptSignatureSchema = schemaAt<ReceiptSignatureContract>(
  receiptRootSchema,
  ["properties", "signature"],
  "receipt.signature",
);

export const referenceTypes = enumValues(referenceTypeSchema, "reference.type");
/**
 * Canonical signal type identifiers. The wire schema accepts any non-empty
 * string so adapters can publish their own identifier without a schema edit;
 * this list mirrors the Rust `signal_type` canonical module.
 */
export const signalTypes = [
  "issue_opened",
  "issue_comment",
  "pull_request_event",
  "review_event",
  "chat_message",
  "alert",
  "deployment_event",
  "effect_required",
  "schedule_tick",
  "operator_note",
  "system_event",
  "support_ticket",
] as const;
export const signalTrustLevels = enumValues(signalTrustLevelSchema, "signal.authenticity.trust_level");
export const runOutcomes = enumValues(runOutcomeSchema, "receipt.seal.outcome");
export const decisionChoices = enumValues(decisionChoiceSchema, "decision.choice");
export const actForms = enumValues(actFormSchema, "receipt.acts[].form");
export const criterionStatuses = enumValues(criterionStatusSchema, "receipt.acts[].criterion_bindings[].status");
export const criteria = enumValues(criterionSchema, "receipt.acts[].criterion_bindings[].criterion_id");
export const authorityResourceFamilies = enumValues(authorityResourceFamilySchema, "authority.terms[].resource_family");
export const authorityVerbs = enumValues(authorityVerbSchema, "authority.terms[].verbs[]");
export const proofKinds = enumValues(proofKindSchema, "reference.proof_kind");

export function validateReferenceContract(value: unknown, label = "reference"): ReferenceContract {
  return validateContractSchema(referenceSchema, value, label) as ReferenceContract;
}

export function validateSignalContract(value: unknown, label = "signal"): SignalContract {
  return validateContractSchema(signalSchema, value, label) as SignalContract;
}

export function validateAuthoritySubsetProofContract(
  value: unknown,
  label = "authority_subset_proof",
): AuthoritySubsetProofContract {
  return validateContractSchema(authoritySubsetProofSchema, value, label) as AuthoritySubsetProofContract;
}

export function validateDecisionContract(value: unknown, label = "decision"): DecisionContract {
  return validateContractSchema(decisionSchema, value, label) as DecisionContract;
}

export function validateSpineArtifactContract(value: unknown, label = "artifact"): ArtifactContract {
  return validateContractSchema(artifactSchema, value, label) as ArtifactContract;
}
