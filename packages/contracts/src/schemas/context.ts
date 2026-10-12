import { Type, type Static } from "../internal.js";
import {
  JSON_SCHEMA_DRAFT_2020_12,
  RUNX_CONTRACT_IDS,
  type DeepReadonly,
  generatedSchema,
  unknownRecordSchema,
  validateContractSchema,
} from "../internal.js";
import { artifactEnvelopeSchema } from "./artifact.js";
import { executionBoundaryObservationSchema } from "./execution-boundary.js";
import { outputSchema } from "./output.js";

export const agentContextProvenanceSchema = Type.Object(
  {
    input: Type.String({ minLength: 1 }),
    output: Type.String({ minLength: 1 }),
    from_step: Type.Optional(Type.String()),
    artifact_id: Type.Optional(Type.String()),
    receipt_id: Type.Optional(Type.String()),
  },
  { additionalProperties: false },
);

export type AgentContextProvenanceContract = DeepReadonly<Static<typeof agentContextProvenanceSchema>>;

export const contextDocumentSchema = Type.Object(
  {
    root_path: Type.String({ minLength: 1 }),
    path: Type.String({ minLength: 1 }),
    sha256: Type.String({ minLength: 1 }),
    content: Type.String(),
  },
  { additionalProperties: false },
);

export type ContextDocumentContract = DeepReadonly<Static<typeof contextDocumentSchema>>;

export const contextSchema = Type.Object(
  {
    memory: Type.Optional(contextDocumentSchema),
    conventions: Type.Optional(contextDocumentSchema),
  },
  { additionalProperties: false },
);

export type ContextContract = DeepReadonly<Static<typeof contextSchema>>;

export const executionLocationSchema = Type.Object(
  {
    skill_directory: Type.String({ minLength: 1 }),
    tool_roots: Type.Optional(Type.Array(Type.String({ minLength: 1 }))),
  },
  { additionalProperties: false },
);

export type ExecutionLocationContract = DeepReadonly<Static<typeof executionLocationSchema>>;

export const environmentRequirementsSchema = Type.Object(
  {
    required: Type.Optional(Type.Array(Type.String())),
    optional: Type.Optional(Type.Array(Type.String())),
  },
  { additionalProperties: false },
);

export const executionRequirementsSchema = Type.Object(
  {
    auth: Type.Optional(Type.Unknown()),
    scopes: Type.Optional(Type.Array(Type.String())),
    environment: Type.Optional(environmentRequirementsSchema),
    runtime: Type.Optional(Type.Unknown()),
  },
  { additionalProperties: false },
);

export const agentExecutionRequirementsSchema = Type.Object(
  {
    declaration: executionRequirementsSchema,
    environment: Type.Optional(Type.Array(Type.Object(
      {
        name: Type.String(),
        required: Type.Boolean(),
        available: Type.Boolean(),
      },
      { additionalProperties: false },
    ))),
    execution_boundary: executionBoundaryObservationSchema,
  },
  { additionalProperties: false },
);

export type AgentExecutionRequirementsContract = DeepReadonly<Static<typeof agentExecutionRequirementsSchema>>;

const agentContextEnvelopeTypeSchema = Type.Object(
  {
    run_id: Type.String({ minLength: 1 }),
    step_id: Type.Optional(Type.String({ minLength: 1 })),
    skill: Type.String({ minLength: 1 }),
    instructions_sha256: Type.String({ minLength: 1 }),
    instructions: Type.String({ minLength: 1 }),
    inputs: unknownRecordSchema(),
    requirements: agentExecutionRequirementsSchema,
    current_context: Type.Array(artifactEnvelopeSchema),
    historical_context: Type.Array(artifactEnvelopeSchema),
    provenance: Type.Array(agentContextProvenanceSchema),
    context: Type.Optional(contextSchema),
    voice_profile: Type.Optional(contextDocumentSchema),
    execution_location: Type.Optional(executionLocationSchema),
    output: Type.Optional(outputSchema),
    trust_boundary: Type.String({ minLength: 1 }),
  },
  {
    $schema: JSON_SCHEMA_DRAFT_2020_12,
    $id: RUNX_CONTRACT_IDS.agentContextEnvelope,
    additionalProperties: false,
  },
);

export type AgentContextEnvelopeContract = DeepReadonly<Static<typeof agentContextEnvelopeTypeSchema>>;

export const agentContextEnvelopeSchema = generatedSchema<AgentContextEnvelopeContract>(
  "agent-context-envelope.schema.json",
);

export function validateAgentContextEnvelopeContract(
  value: unknown,
  label = "agent_context_envelope",
): AgentContextEnvelopeContract {
  return validateContractSchema(agentContextEnvelopeSchema, value, label);
}
