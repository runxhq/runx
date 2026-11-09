import { generatedDefinition, generatedSchema, validateContractSchema } from "../internal.js";
import type {
  ActAddressContract,
  ActFormContract,
  ArtifactContract,
  AuthorityApprovalContract,
  AuthorityBoundsContract,
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
  IntentContract,
  ProofKindContract,
  ReceiptActContract,
  ReceiptIssuerContract,
  ReceiptSignatureContract,
  ReferenceContract,
  ReferenceTypeContract,
  RunOutcomeContract,
} from "../generated/contract-types.js";

export const referenceSchema = generatedSchema<ReferenceContract>("reference.schema.json");
export const authoritySubsetProofSchema = generatedSchema<AuthoritySubsetProofContract>(
  "authority-subset-proof.schema.json",
);
export const authorityTermSchema = generatedSchema<AuthorityTermContract>("authority-term.schema.json");
export const decisionSchema = generatedSchema<DecisionContract>("decision.schema.json");
export const artifactSchema = generatedSchema<ArtifactContract>("artifact.schema.json");


export const referenceTypeSchema = generatedDefinition<ReferenceTypeContract>("ReferenceType");
export const actAddressSchema = generatedDefinition<ActAddressContract>("ActAddress");
export const proofKindSchema = generatedDefinition<ProofKindContract>("ProofKind");
export const digestCommitmentSchema = generatedDefinition<DigestCommitmentContract>("DigestCommitment");
export const authorityBoundsSchema = generatedDefinition<AuthorityBoundsContract>("AuthorityBounds");
export const authorityVerbSchema = generatedDefinition<AuthorityVerbContract>("AuthorityVerb");
export const authorityApprovalSchema = generatedDefinition<AuthorityApprovalContract>("AuthorityApproval");
export const authoritySubsetComparisonSchema = generatedDefinition<AuthoritySubsetComparisonContract>("AuthoritySubsetComparison");
const receiptActSchema = generatedDefinition<ReceiptActContract>("ReceiptAct");
export const intentSchema = generatedDefinition<IntentContract>("Intent");
export const criterionBindingSchema = generatedDefinition<CriterionBindingContract>("CriterionBinding");
export const criterionSchema = generatedDefinition<CriterionContract>("Criterion");
export const criterionStatusSchema = generatedDefinition<CriterionStatusContract>("CriterionStatus");
export const runOutcomeSchema = generatedDefinition<RunOutcomeContract>("RunOutcome");
export const actFormSchema = generatedDefinition<ActFormContract>("ActForm");
export const decisionChoiceSchema = generatedDefinition<DecisionChoiceContract>("DecisionChoice");

export const receiptIssuerSchema = generatedDefinition<ReceiptIssuerContract>("ReceiptIssuer");
export const receiptSignatureSchema = generatedDefinition<ReceiptSignatureContract>("ReceiptSignature");

export function validateReferenceContract(value: unknown, label = "reference"): ReferenceContract {
  return validateContractSchema(referenceSchema, value, label) as ReferenceContract;
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
