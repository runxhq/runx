import { describe, expect, it } from "vitest";

import {
  RUNX_CONTRACT_IDS,
  RUNX_AUXILIARY_SCHEMA_IDS,
  RUNX_CONTROL_SCHEMA_REFS,
  RUNX_LOGICAL_SCHEMAS,
  agentContextEnvelopeSchema,
  agentActInvocationSchema,
  approvalGateSchema,
  actResultEnvelopeSchema,
  actAssignmentV1Schema,
  authorityProofSchema,
  credentialDeliveryResponseV1Schema,
  credentialDeliveryObservationV1Schema,
  credentialDeliveryProfileV1Schema,
  credentialDeliveryRequestV1Schema,
  dataOperationResultV1Schema,
  devV1Schema,
  doctorV1Schema,
  effectFinalityReceiptV1Schema,
  externalAdapterCancellationFrameV1Schema,
  externalAdapterCredentialRequestV1Schema,
  externalAdapterHostResolutionFrameV1Schema,
  externalAdapterInvocationV1Schema,
  externalAdapterManifestV1Schema,
  externalAdapterResponseV1Schema,
  fixtureV1Schema,
  handoffSignalV1Schema,
  handoffStateV1Schema,
  ledgerRecordSchema,
  listV1Schema,
  outputSchema,
  packetIndexV1Schema,
  credentialEnvelopeSchema,
  questionSchema,
  registryBindingSchema,
  reviewReceiptOutputSchema,
  resolutionRequestSchema,
  resolutionResponseSchema,
  receiptV1Schema,
  receiptClassSchema,
  receiptPaidInvocationBindingSchema,
  receiptEvidenceSchema,
  paidSkillListingV1Schema,
  paidInvocationV1Schema,
  quotePaidInvocationRequestV1Schema,
  cancelPaidInvocationResultV1Schema,
  runSummaryV1Schema,
  runxContractSchemas,
  runxAuxiliarySchemas,
  runxGeneratedSchemaArtifacts,
  scopeAdmissionSchema,
  suppressionRecordV1Schema,
  threadOutboxProviderFetchV1Schema,
  threadOutboxProviderManifestV1Schema,
  threadOutboxProviderObservationV1Schema,
  threadOutboxProviderPushV1Schema,
  toolManifestV1Schema,
  contractSchemaMatches,
  validateActResultEnvelopeContract,
  validateActContract,
  validateAgentContextEnvelopeContract,
  validateAuthorityProofContract,
  validateOutputContract,
  validateResolutionRequestContract,
  validateCredentialEnvelopeContract,
  validateDataOperationResultContract,
  validateActAssignmentContract,
  validateDevReportContract,
  validateDoctorReportContract,
  validateHandoffSignalContract,
  validateHandoffStateContract,
  validateEffectFinalityReceiptContract,
  validateReceiptContract,
  validateRegistryBindingContract,
  validateRunxListReportContract,
  validateReviewReceiptOutputContract,
  validateScopeAdmissionContract,
  validateSuppressionRecordContract,
  validateOperationalPolicyContract,
  validateOperationalPolicySemantics,
  validateOperationalProposalContract,
  validateSignalContract,
  validateReferenceContract,
  proofKinds,
  proofKindSchema,
} from "./index.js";

describe("@runxhq/contracts", () => {
  it("exports stable runx logical schema identifiers", () => {
    expect(RUNX_LOGICAL_SCHEMAS.doctor).toBe("runx.doctor.v1");
    expect(RUNX_LOGICAL_SCHEMAS.receipt).toBe("runx.receipt.v1");
    expect(RUNX_LOGICAL_SCHEMAS.effectFinalityReceipt).toBe("runx.effect_finality_receipt.v1");
    expect(RUNX_LOGICAL_SCHEMAS.operationalProposal).toBe("runx.operational_proposal.v1");
    expect(RUNX_LOGICAL_SCHEMAS.dataOperationResult).toBe("runx.data.operation_result.v1");
  });

  it("uses durable schema URI ids", () => {
    expect(RUNX_CONTRACT_IDS.toolManifest).toBe("https://schemas.runx.ai/runx/tool/manifest/v1.json");
    expect(runxContractSchemas.toolManifest.$id).toBe(RUNX_CONTRACT_IDS.toolManifest);
    expect(toolManifestV1Schema).toBe(runxContractSchemas.toolManifest);
    expect(RUNX_CONTRACT_IDS.effectFinalityReceipt)
      .toBe("https://schemas.runx.ai/runx/effect-finality-receipt/v1.json");
    expect(runxContractSchemas.effectFinalityReceipt.$id).toBe(RUNX_CONTRACT_IDS.effectFinalityReceipt);
    expect(RUNX_CONTRACT_IDS.dataOperationResult)
      .toBe("https://schemas.runx.ai/runx/data/operation-result/v1.json");
    expect(runxContractSchemas.dataOperationResult.$id).toBe(RUNX_CONTRACT_IDS.dataOperationResult);
    expect((toolManifestV1Schema.properties as Record<string, unknown>).source).toBeDefined();
    expect((toolManifestV1Schema.required as readonly string[])).not.toContain("version");
    const devProperties = runxContractSchemas.dev.properties as Record<string, unknown> | undefined;
    expect(devProperties?.doctor).toMatchObject({ $id: RUNX_CONTRACT_IDS.doctor });
  });

  it("publishes paid-invocation V1 through the curated contract registry", () => {
    expect(RUNX_LOGICAL_SCHEMAS.paidSkillListing)
      .toBe("runx.marketplace.paid_skill_listing.v1");
    expect(RUNX_CONTRACT_IDS.paidSkillListing)
      .toBe("https://schemas.runx.ai/runx/marketplace/paid-skill-listing/v1.json");
    expect(paidSkillListingV1Schema).toBe(runxContractSchemas.paidSkillListing);
    expect(RUNX_LOGICAL_SCHEMAS.paidInvocation).toBe("runx.payment.paid_invocation.v1");
    expect(RUNX_CONTRACT_IDS.paidInvocation)
      .toBe("https://schemas.runx.ai/runx/payment/paid-invocation/v1.json");
    expect(paidInvocationV1Schema).toBe(runxContractSchemas.paidInvocation);
    expect(quotePaidInvocationRequestV1Schema).toBe(runxContractSchemas.quotePaidInvocationRequest);
    expect(cancelPaidInvocationResultV1Schema).toBe(runxContractSchemas.cancelPaidInvocationResult);
  });

  it("exports Rust-generated artifacts for control schema facades", () => {
    expect(outputSchema).toBe(runxContractSchemas.output);
    expect(agentContextEnvelopeSchema).toBe(runxContractSchemas.agentContextEnvelope);
    expect(agentActInvocationSchema).toBe(runxContractSchemas.agentActInvocation);
    expect(questionSchema).toBe(runxContractSchemas.question);
    expect(approvalGateSchema).toBe(runxContractSchemas.approvalGate);
    expect(resolutionRequestSchema).toBe(runxContractSchemas.resolutionRequest);
    expect(resolutionResponseSchema).toBe(runxContractSchemas.resolutionResponse);
    expect(actResultEnvelopeSchema).toBe(runxContractSchemas.actResultEnvelope);
    expect(credentialEnvelopeSchema).toBe(runxContractSchemas.credentialEnvelope);
    expect(scopeAdmissionSchema).toBe(runxContractSchemas.scopeAdmission);
    expect(authorityProofSchema).toBe(runxContractSchemas.authorityProof);
    expect(credentialDeliveryProfileV1Schema).toBe(runxContractSchemas.credentialDeliveryProfile);
    expect(credentialDeliveryRequestV1Schema).toBe(runxContractSchemas.credentialDeliveryRequest);
    expect(credentialDeliveryResponseV1Schema).toBe(runxContractSchemas.credentialDeliveryResponse);
    expect(credentialDeliveryObservationV1Schema).toBe(runxContractSchemas.credentialDeliveryObservation);
    expect(threadOutboxProviderManifestV1Schema).toBe(runxContractSchemas.threadOutboxProviderManifest);
    expect(threadOutboxProviderPushV1Schema).toBe(runxContractSchemas.threadOutboxProviderPush);
    expect(threadOutboxProviderFetchV1Schema).toBe(runxContractSchemas.threadOutboxProviderFetch);
    expect(threadOutboxProviderObservationV1Schema).toBe(runxContractSchemas.threadOutboxProviderObservation);
    expect(dataOperationResultV1Schema).toBe(runxContractSchemas.dataOperationResult);
    expect(externalAdapterManifestV1Schema).toBe(runxContractSchemas.externalAdapterManifest);
    expect(externalAdapterCredentialRequestV1Schema).toBe(runxContractSchemas.externalAdapterCredentialRequest);
    expect(externalAdapterInvocationV1Schema).toBe(runxContractSchemas.externalAdapterInvocation);
    expect(externalAdapterResponseV1Schema).toBe(runxContractSchemas.externalAdapterResponse);
    expect(externalAdapterHostResolutionFrameV1Schema).toBe(runxContractSchemas.externalAdapterHostResolution);
    expect(externalAdapterCancellationFrameV1Schema).toBe(runxContractSchemas.externalAdapterCancellation);
    expect(receiptV1Schema).toBe(runxContractSchemas.receipt);
    expect(receiptClassSchema).toBeDefined();
    expect(receiptPaidInvocationBindingSchema).toBeDefined();
    expect(receiptEvidenceSchema).toBeDefined();
    expect(effectFinalityReceiptV1Schema).toBe(runxContractSchemas.effectFinalityReceipt);
    expect(doctorV1Schema).toBe(runxContractSchemas.doctor);
    expect(devV1Schema).toBe(runxContractSchemas.dev);
    expect(listV1Schema).toBe(runxContractSchemas.list);
    expect(runSummaryV1Schema).toBe(runxContractSchemas.runSummary);
    expect(fixtureV1Schema).toBe(runxContractSchemas.fixture);
    expect(packetIndexV1Schema).toBe(runxContractSchemas.packetIndex);
    expect(actAssignmentV1Schema).toBe(runxContractSchemas.actAssignment);
    expect(ledgerRecordSchema).toBe(runxContractSchemas.ledgerEntry);
    expect(handoffSignalV1Schema).toBe(runxContractSchemas.handoffSignal);
    expect(handoffStateV1Schema).toBe(runxContractSchemas.handoffState);
    expect(suppressionRecordV1Schema).toBe(runxContractSchemas.suppressionRecord);
  });

  it("keeps fixture lanes aligned with authoring plan", () => {
    const fixtureProperties = runxContractSchemas.fixture.properties as Record<string, unknown> | undefined;
    expect(fixtureProperties?.lane).toEqual({
      type: "string",
      enum: ["deterministic", "agent", "repo-integration"],
    });
  });

  it("validates governed data operation result packets", () => {
    const committed = validateDataOperationResultContract({
      schema: RUNX_LOGICAL_SCHEMAS.dataOperationResult,
      data_source_ref: "tenant://acme/board",
      provider: "postgres",
      operation: "append_event",
      resource: "board_events",
      aggregate_id: "posting-123",
      status: "committed",
      before_version: 2,
      after_version: 3,
      idempotency_key: "posting-123:claim:agent-9",
      event_ref: "board_events:posting-123:3",
      event_digest: "sha256:event",
      result_digest: "sha256:result",
      projection_digest: "sha256:projection",
      projection: null,
      events: [],
      rows: [],
      redactions: [],
      stop_conditions: [],
      provider_evidence: {
        adapter: "data.postgres",
        profile: "prod-board",
      },
    });

    expect(committed.status).toBe("committed");
    expect(committed.provider_evidence?.adapter).toBe("data.postgres");

    const conflict = validateDataOperationResultContract({
      schema: RUNX_LOGICAL_SCHEMAS.dataOperationResult,
      data_source_ref: "tenant://acme/board",
      provider: "postgres",
      operation: "append_event",
      resource: "board_events",
      aggregate_id: "posting-123",
      status: "conflict",
      before_version: 4,
      after_version: 4,
      idempotency_key: "posting-123:claim:agent-9",
      event_ref: null,
      event_digest: "sha256:event",
      result_digest: "sha256:conflict",
      projection_digest: "sha256:projection",
      projection: null,
      events: [],
      rows: [],
      redactions: [],
      stop_conditions: [
        {
          code: "conflict",
          message: "expected version 2, got 4",
        },
      ],
      provider_evidence: {
        adapter: "data.postgres",
        profile: "prod-board",
      },
    });

    expect(conflict.stop_conditions[0]?.code).toBe("conflict");
  });

  it("accepts typed proof kinds on references", () => {
    expect(proofKinds).toEqual(["effect_evidence", "effect_finality", "credential_resolution"]);
    expect(proofKindSchema).toEqual({
      type: "string",
      enum: ["effect_evidence", "effect_finality", "credential_resolution"],
    });
    expect(validateReferenceContract({
      type: "verification",
      uri: "receipt-proof:mock:payment-execution-001",
      proof_kind: "effect_evidence",
      label: "display-only text",
    })).toMatchObject({
      type: "verification",
      proof_kind: "effect_evidence",
    });
    expect(validateReferenceContract({
      type: "verification",
      uri: "receipt-proof:mock:effect-finality-001",
      proof_kind: "effect_finality",
    })).toMatchObject({
      type: "verification",
      proof_kind: "effect_finality",
    });
    expect(validateReferenceContract({
      type: "credential",
      uri: "runx:credential:local:grant_1",
      provider: "github",
      proof_kind: "credential_resolution",
    })).toMatchObject({
      type: "credential",
      provider: "github",
      proof_kind: "credential_resolution",
    });
  });

  it("exports and validates effect finality receipts", () => {
    expect(validateEffectFinalityReceiptContract({
      schema: "runx.effect_finality_receipt.v1",
      id: "effect-finality-001",
      created_at: "2026-01-01T00:00:00Z",
      family: "payment",
      phase: "sealed",
      original_receipt_ref: {
        type: "receipt",
        uri: "runx:receipt:original-001",
      },
      criterion_id: "payment.finality",
      evidence_refs: [{
        type: "verification",
        uri: "receipt-proof:mock:effect-finality-001",
        proof_kind: "effect_finality",
      }],
      norm_refs: ["acme:norm:reply-before-escalation"],
      proof_ref: {
        type: "verification",
        uri: "receipt-proof:mock:effect-finality-001",
        proof_kind: "effect_finality",
      },
      confirmation_depth: 1,
      payload: {
        provider: "stripe",
      },
    })).toMatchObject({
      schema: "runx.effect_finality_receipt.v1",
      family: "payment",
      phase: "sealed",
      criterion_id: "payment.finality",
      norm_refs: ["acme:norm:reply-before-escalation"],
    });
    expect(() => validateEffectFinalityReceiptContract({
      schema: "runx.effect_finality_receipt.v1",
      id: "effect-finality-002",
      created_at: "2026-01-01T00:00:00Z",
      family: "payment",
      phase: "sealed",
    })).toThrow(/effect finality receipt/);
  });

  it("owns credential envelope schema and runtime validation", () => {
    expect(credentialEnvelopeSchema.$id).toBe(RUNX_CONTROL_SCHEMA_REFS.credential_envelope);
    expect(validateCredentialEnvelopeContract({
      kind: "runx.credential-envelope.v1",
      grant_id: "grant_1",
      provider: "github",
      auth_mode: "api_key",
      material_kind: "api_key",
      provider_reference: "local_per_run",
      scopes: ["repo:read"],
      material_ref: "local:github:grant_1",
    })).toMatchObject({
      provider: "github",
      scopes: ["repo:read"],
    });
  });

  it("owns scope admission schema and runtime validation", () => {
    expect(RUNX_CONTROL_SCHEMA_REFS.scope_admission).toBe("https://runx.ai/spec/scope-admission.schema.json");
    expect(validateScopeAdmissionContract({
      status: "allow",
      requested_scopes: ["repo:status"],
      granted_scopes: ["repo:*"],
      decision_summary: "",
    })).toEqual({
      status: "allow",
      requested_scopes: ["repo:status"],
      granted_scopes: ["repo:*"],
      decision_summary: "",
    });
    expect(() => validateScopeAdmissionContract({
      status: "pending",
      requested_scopes: ["repo:status"],
      granted_scopes: ["repo:*"],
    })).toThrow(/scope-admission\.schema\.json/);
  });

  it("owns authority proof schema and generated artifact", () => {
    expect(authorityProofSchema.$id).toBe(RUNX_CONTROL_SCHEMA_REFS.authority_proof);
    expect(runxContractSchemas.authorityProof.$id).toBe(authorityProofSchema.$id);
    expect(runxGeneratedSchemaArtifacts["authority-proof.schema.json"].$id).toBe(authorityProofSchema.$id);
    expect(validateAuthorityProofContract({
      schema_version: "runx.authority-proof.v1",
      skill_name: "connected-review",
      source_type: "agent-task",
      requested: {
        connected_auth: true,
        scopes: ["repo:read"],
        scope_family: "github_repo",
        authority_kind: "read_only",
        target_repo: "runxhq/runx",
      },
      scope_admission: {
        status: "allow",
        requested_scopes: ["repo:read"],
        granted_scopes: ["repo:*"],
      },
      credential_material: {
        status: "not_resolved",
        provider: "github",
        scopes: ["repo:read"],
        scope_family: "github_repo",
        authority_kind: "read_only",
        target_repo: "runxhq/runx",
      },
      execution_boundary: {
        kind: "remote_provider",
      },
      redaction: {
        status: "applied",
        secret_material: "omitted",
        stdout: "hashed",
        stderr: "hashed",
        metadata_secret_keys: ["token-like metadata keys"],
      },
    })).toMatchObject({
      schema_version: "runx.authority-proof.v1",
    });
  });

  it("owns executor control protocol schemas and runtime validation", () => {
    expect(outputSchema.$id).toBe(RUNX_CONTROL_SCHEMA_REFS.output);
    expect(agentContextEnvelopeSchema.$id).toBe(RUNX_CONTROL_SCHEMA_REFS.agent_context_envelope);
    expect(actResultEnvelopeSchema.$id).toBe(RUNX_CONTROL_SCHEMA_REFS.act_result);
    expect(runxGeneratedSchemaArtifacts["output.schema.json"].$id).toBe(outputSchema.$id);
    expect(runxGeneratedSchemaArtifacts["agent-context-envelope.schema.json"].$id).toBe(agentContextEnvelopeSchema.$id);
    expect(runxGeneratedSchemaArtifacts["act-result.schema.json"].$id).toBe(actResultEnvelopeSchema.$id);

    expect(validateOutputContract({
      summary: "string",
      verdict: {
        type: "string",
        enum: ["pass", "fail"],
        required: true,
      },
    })).toMatchObject({
      summary: "string",
    });

    expect(validateAgentContextEnvelopeContract({
      run_id: "rx_contract",
      skill: "demo.skill",
      instructions_sha256: `sha256:${"a".repeat(64)}`,
      instructions: "Do the work.",
      inputs: {},
      allowed_tools: ["fs.read"],
      requirements: {
        declaration: {},
        execution_boundary: { kind: "remote_provider" },
      },
      current_context: [],
      historical_context: [],
      provenance: [],
      output: {
        summary: "string",
      },
      trust_boundary: "test",
    })).toMatchObject({
      run_id: "rx_contract",
      output: {
        summary: "string",
      },
    });

    expect(() => validateAgentContextEnvelopeContract({
      run_id: "rx_contract",
      skill: "demo.skill",
      instructions_sha256: `sha256:${"a".repeat(64)}`,
      instructions: "Do the work.",
      inputs: {},
      allowed_tools: [],
      requirements: {
        declaration: {},
        execution_boundary: { kind: "remote_provider" },
      },
      current_context: [],
      historical_context: [],
      provenance: [],
      context: {
        voice_grammar: {},
      },
      trust_boundary: "test",
    })).toThrow("agent_context_envelope.context must match");

    expect(validateAgentContextEnvelopeContract({
      run_id: "rx_contract",
      step_id: "plan",
      skill: "demo.plan",
      instructions_sha256: `sha256:${"a".repeat(64)}`,
      instructions: "Do the work.",
      inputs: {},
      allowed_tools: ["fs.read"],
      requirements: {
        declaration: {},
        execution_boundary: { kind: "remote_provider" },
      },
      current_context: [],
      historical_context: [],
      provenance: [],
      execution_location: {
        skill_directory: "/tmp/demo-skill",
        tool_roots: ["/tmp/extra-tools"],
      },
      trust_boundary: "test",
    })).toMatchObject({
      execution_location: {
        skill_directory: "/tmp/demo-skill",
        tool_roots: ["/tmp/extra-tools"],
      },
    });

    const resolutionRequest = validateResolutionRequestContract({
      id: "approval.demo",
      kind: "approval",
      gate: {
        id: "gate.demo",
        reason: "Needs human approval.",
      },
    });

    expect(validateActResultEnvelopeContract({
      status: "needs_agent",
      stdout: "",
      stderr: "",
      exitCode: null,
      signal: null,
      durationMs: 0,
      request: resolutionRequest,
    })).toMatchObject({
      status: "needs_agent",
      request: {
        kind: "approval",
      },
    });
  });

  it("owns generated auxiliary schemas", () => {
    expect(registryBindingSchema.$id).toBe(RUNX_AUXILIARY_SCHEMA_IDS.registryBinding);
    expect(reviewReceiptOutputSchema.$id).toBe(RUNX_AUXILIARY_SCHEMA_IDS.reviewReceiptOutput);
    expect(runxAuxiliarySchemas.registryBinding).toBe(runxGeneratedSchemaArtifacts["registry-binding.schema.json"]);
    expect(runxAuxiliarySchemas.reviewReceiptOutput).toBe(runxGeneratedSchemaArtifacts["review-receipt-output.schema.json"]);
    expect(runxAuxiliarySchemas.registryBinding.$id).toBe(registryBindingSchema.$id);
    expect(runxAuxiliarySchemas.reviewReceiptOutput.$id).toBe(reviewReceiptOutputSchema.$id);
    expect(runxGeneratedSchemaArtifacts["doctor.schema.json"]).toBe(runxContractSchemas.doctor);
    expect(runxGeneratedSchemaArtifacts["act-assignment.schema.json"]).toBe(runxContractSchemas.actAssignment);
    expect(runxGeneratedSchemaArtifacts["receipt.schema.json"]).toBe(runxContractSchemas.receipt);
    expect(runxGeneratedSchemaArtifacts["effect-finality-receipt.schema.json"])
      .toBe(runxContractSchemas.effectFinalityReceipt);
    expect(runxGeneratedSchemaArtifacts["run-summary.schema.json"]).toBe(runxContractSchemas.runSummary);
    const retiredReceiptArtifact = `${"harness"}-receipt.schema.json` as keyof typeof runxGeneratedSchemaArtifacts;
    expect(runxGeneratedSchemaArtifacts[retiredReceiptArtifact]).toBeUndefined();
    const retiredCentralArtifact = `${"engage"}ment.schema.json` as keyof typeof runxGeneratedSchemaArtifacts;
    const retiredEvidenceArtifact = `${"evidence"}-bundle.schema.json` as keyof typeof runxGeneratedSchemaArtifacts;
    expect(runxGeneratedSchemaArtifacts[retiredCentralArtifact]).toBeUndefined();
    expect(runxGeneratedSchemaArtifacts[retiredEvidenceArtifact])
      .toBeUndefined();
    expect(runxGeneratedSchemaArtifacts["handoff-signal.schema.json"]).toBe(runxContractSchemas.handoffSignal);
    expect(runxGeneratedSchemaArtifacts["handoff-state.schema.json"]).toBe(runxContractSchemas.handoffState);
    expect(runxGeneratedSchemaArtifacts["suppression-record.schema.json"]).toBe(runxContractSchemas.suppressionRecord);
    expect(runxGeneratedSchemaArtifacts["operational-policy.schema.json"]).toBe(runxContractSchemas.operationalPolicy);
    expect(runxGeneratedSchemaArtifacts["operational-proposal.schema.json"]).toBe(runxContractSchemas.operationalProposal);
    expect(runxGeneratedSchemaArtifacts["thread-outbox-provider-manifest.schema.json"])
      .toBe(runxContractSchemas.threadOutboxProviderManifest);
    expect(runxGeneratedSchemaArtifacts["thread-outbox-provider-push.schema.json"])
      .toBe(runxContractSchemas.threadOutboxProviderPush);
    expect(runxGeneratedSchemaArtifacts["thread-outbox-provider-fetch.schema.json"])
      .toBe(runxContractSchemas.threadOutboxProviderFetch);
    expect(runxGeneratedSchemaArtifacts["thread-outbox-provider-observation.schema.json"])
      .toBe(runxContractSchemas.threadOutboxProviderObservation);
    const retiredIssueArtifact = `${"issue"}-to-pr-${"out"}come.schema.json` as keyof typeof runxGeneratedSchemaArtifacts;
    expect(runxGeneratedSchemaArtifacts[retiredIssueArtifact]).toBeUndefined();
    expect(runxGeneratedSchemaArtifacts["review-receipt-output.schema.json"].$id).toBe(reviewReceiptOutputSchema.$id);
  });

  it("owns operational policy for issue intake and source-thread routing", () => {
    expect(RUNX_LOGICAL_SCHEMAS.operationalPolicy).toBe("runx.operational_policy.v1");
    expect(RUNX_CONTRACT_IDS.operationalPolicy).toBe("https://schemas.runx.ai/runx/operational-policy/v1.json");
    expect(runxContractSchemas.operationalPolicy.$id).toBe(RUNX_CONTRACT_IDS.operationalPolicy);
    const policy = {
      schema: RUNX_LOGICAL_SCHEMAS.operationalPolicy,
      schema_version: "runx.operational_policy.v1",
      policy_id: "example-dev-flow",
      sources: [{
        source_id: "bugs",
        provider: "slack",
        allowed_locators: ["slack://team/T123/channel/CBUGS"],
        allowed_actions: ["issue-intake", "issue-to-pr", "manual-review"],
        source_thread: {
          required: true,
          publish_mode: "reply",
          missing_behavior: "fail_closed",
        },
      }],
      runners: [{
        runner_id: "local-review",
        kind: "local",
        state: "available",
        allowed_actions: ["issue-to-pr", "merge-assist"],
        target_repos: ["example/api"],
        scafld_required: true,
      }],
      owner_routes: [{
        route_id: "api-owner",
        owners: ["Ops"],
        target_repos: ["example/api"],
      }],
      targets: [{
        repo: "example/api",
        runner_ids: ["local-review"],
        allowed_actions: ["issue-to-pr", "merge-assist"],
        default_owner_route: "api-owner",
        scafld_required: true,
      }],
      dedupe: {
        strategy: "source_fingerprint",
        key_fields: ["source_locator", "target_repo"],
        on_duplicate: "reuse",
      },
      outcomes: {
        observe_provider: true,
        verification_required: true,
        close_source_issue: "when_verified",
        publish_final_source_thread_update: true,
      },
      permissions: {
        auto_merge: false,
        mutate_target_repo: true,
        require_human_merge_gate: true,
      },
    };
    expect(validateOperationalPolicyContract(policy)).toMatchObject({
      policy_id: "example-dev-flow",
    });
    expect(validateOperationalPolicySemantics(policy)).toMatchObject({
      policy_id: "example-dev-flow",
      permissions: {
        auto_merge: false,
      },
    });
  });

  it("owns the generic operational proposal contract", () => {
    expect(RUNX_CONTRACT_IDS.operationalProposal)
      .toBe("https://schemas.runx.ai/runx/operational-proposal/v1.json");
    expect(runxContractSchemas.operationalProposal.$id)
      .toBe(RUNX_CONTRACT_IDS.operationalProposal);

    const proposal = validateOperationalProposalContract({
      schema: RUNX_LOGICAL_SCHEMAS.operationalProposal,
      proposal_id: "proposal_123",
      proposal_kind: "escalation",
      source_event_id: "slack_event_123",
      idempotency: {
        key: "operational-proposal:slack_event_123:tracking-to-change:api-owner",
        fingerprint: "sha256:proposal-123-source-action-target",
      },
      source_ref: {
        type: "provider_thread",
        uri: "slack://team/T123/channel/CBUGS/thread/1710000000.000100",
      },
      source_thread_ref: {
        type: "provider_thread",
        uri: "slack://team/T123/channel/CBUGS/thread/1710000000.000100",
      },
      hydrated_context_ref: {
        type: "artifact",
        uri: "runx:artifact:hydrated_context_123",
      },
      redaction_status: "redacted",
      decision_summary: "The issue needs a governed fix.",
      rationale: "The source thread contains reproducible failure evidence and a target repo route.",
      recommended_actions: [{
        action_intent: "tracking-to-change",
        summary: "Build a guarded fix in the owning repository.",
        mutating: true,
        target_refs: [{
          type: "repository",
          uri: "github://example/api",
        }],
      }],
      evidence_refs: [{
        type: "artifact",
        uri: "runx:artifact:public_evidence_123",
      }],
      artifact_refs: [{
        type: "artifact",
        uri: "runx:artifact:plan_123",
      }],
      receipt_refs: [{
        type: "receipt",
        uri: "runx:receipt:receipt_123",
      }],
      story_refs: [{
        type: "surface",
        uri: "runx:story:story_123",
      }],
      result_refs: [{
        role: "tracking_item",
        ref: {
          type: "tracking_item",
          uri: "github://example/api/issues/123",
          provider: "github",
          locator: "example/api#123",
        },
      }, {
        role: "change_request",
        ref: {
          type: "change_request",
          uri: "github://example/api/pulls/124",
          provider: "github",
          locator: "example/api#124",
        },
      }],
      publication_refs: [{
        role: "source_thread_update",
        ref: {
          type: "provider_thread",
          uri: "slack://team/T123/channel/CBUGS/thread/1710000000.000100",
          provider: "slack",
          locator: "T123/CBUGS/1710000000.000100",
        },
      }],
      owner_route_id: "api-owner",
      confidence: 0.86,
      risks: ["Target repo tests may reveal a broader contract issue."],
      caveats: ["Customer send is not authorized by this proposal."],
      missing_context: [],
      authority: {
        proposal_only: true,
        mutation_authority_granted: false,
        publication_authority_granted: false,
        final_decision_authority_granted: false,
        notes: ["A human must approve merge and any customer-facing send."],
      },
      human_gates: [{
        gate_id: "gate_merge_review",
        gate_kind: "final_change_approval",
        required: true,
        decision: "Review and approve the final change if the fix is correct.",
        reason: "Mutating target repo work requires a human final-change gate.",
      }],
      allowed_next_actions: ["tracking-to-change", "manual-review"],
      final_outcome: {
        observed: true,
        status: "merged",
        summary: "The governed change request was merged and verified.",
        observed_at: "2026-05-28T00:00:00Z",
        refs: [{
          type: "change_request",
          uri: "github://example/api/pulls/124",
        }],
      },
      public_summary: "Escalation proposal prepared with tracking item and change request links.",
    });

    expect(proposal).toMatchObject({
      proposal_kind: "escalation",
      owner_route_id: "api-owner",
      authority: {
        proposal_only: true,
        mutation_authority_granted: false,
      },
    });

    expect(contractSchemaMatches(runxContractSchemas.operationalProposal, {
      ...proposal,
      authority: {
        ...proposal.authority,
        mutation_authority_granted: true,
      },
    })).toBe(false);
  });

  it("validates a source signal and its complete execution receipt", () => {
    const issueRef = {
      type: "github_issue",
      uri: "github://runxhq/example/issues/101",
      provider: "github",
      locator: "runxhq/example#101",
      observed_at: "2026-05-18T00:00:00Z",
    };
    const principalRef = { type: "principal", uri: "runx:principal:agent_1" };
    const criterion = {
      criterion_id: "crit_revision_reviewable",
      statement: "Revision is available for review.",
      required: true,
    };
    const intent = {
      purpose: "Prepare a bounded revision for checkout retry behavior.",
      legitimacy: "The authenticated issue requests a fix in the target repository.",
      success_criteria: [criterion],
      constraints: ["Stay inside the checkout surface."],
      derived_from: [issueRef],
    };
    const verification = {
      status: "passed",
      checks: [{
        check_id: "check_pr_open",
        criterion_ids: ["crit_revision_reviewable"],
        status: "passed",
        evidence_refs: [{ type: "github_pull_request", uri: "github://runxhq/example/pulls/102" }],
      }],
      verified_at: "2026-05-18T00:02:00Z",
      evidence_refs: [{ type: "github_pull_request", uri: "github://runxhq/example/pulls/102" }],
    };
    const seal = {
      disposition: "closed",
      reason_code: "revision_ready",
      summary: "Revision act completed and reviewable PR was observed.",
      closed_at: "2026-05-18T00:03:00Z",
      last_observed_at: "2026-05-18T00:03:00Z",
      criteria: [{
        criterion_id: "crit_revision_reviewable",
        status: "verified",
        verification_refs: [{ type: "verification", uri: "runx:verification:check_pr_open" }],
        evidence_refs: [{ type: "github_pull_request", uri: "github://runxhq/example/pulls/102" }],
      }],
    };

    expect(validateSignalContract({
      schema: "runx.signal.v1",
      signal_id: "sig_101",
      source_ref: issueRef,
      authenticity: {
        host_ref: { type: "webhook_delivery", uri: "github://delivery/abc" },
        principal_ref: { type: "principal", uri: "github:user:octocat" },
        verified_by_ref: principalRef,
        trust_level: "verified_signature",
        verified_at: "2026-05-18T00:00:01Z",
      },
      signal_type: "issue_opened",
      title: "Checkout retry failure",
      body_preview: "Retry fails when the discount service flakes.",
      observed_at: "2026-05-18T00:00:00Z",
      evidence_refs: [issueRef],
    })).toMatchObject({ schema: "runx.signal.v1", signal_type: "issue_opened" });

    expect(validateReceiptContract({
      schema: "runx.receipt.v1",
      id: "hrn_rcpt_123",
      created_at: "2026-05-18T00:03:01Z",
      canonicalization: "runx.receipt.c14n.v1",
      issuer: {
        type: "local",
        kid: "key_1",
        public_key_sha256: "sha256:key",
      },
      signature: {
        alg: "Ed25519",
        value: "sig_123",
      },
      digest: "sha256:receipt",
      idempotency: {
        intent_key: "sha256:checkout-retry",
        trigger_fingerprint: "sha256:trigger",
        content_hash: "sha256:content",
      },
      class: "executed",
      subject: {
        kind: "skill",
        ref: { type: "harness", uri: "runx:harness:local-cli" },
        commitments: [{
          scope: "output",
          algorithm: "sha256",
          value: "sha256:private-transcript",
          canonicalization: "runx.artifact-hash.v1",
        }],
      },
      authority: {
        actor_ref: principalRef,
        grant_refs: [{ type: "grant", uri: "runx:grant:repo_write" }],
        scope_refs: [{ type: "scope_admission", uri: "runx:scope_admission:repo_write" }],
        authority_proof_refs: [{ type: "authority_proof", uri: "runx:authority_proof:proof_1" }],
        attenuation: { parent_authority_ref: null, subset_proof: null },
        terms: [{
          term_id: "term_repo_write",
          principal_ref: principalRef,
          resource_ref: { type: "github_repo", uri: "github://runxhq/example" },
          resource_family: "github_repo",
          verbs: ["read", "write", "create"],
          bounds: {
            repo_path_globs: ["app/checkout/**"],
            branch_patterns: ["runx/**"],
            max_child_depth: 1,
          },
          conditions: [{
            condition_id: "cond_signal_verified",
            predicate: "signal_verified",
            refs: [{ type: "signal", uri: "runx:signal:sig_101" }],
          }],
          approvals: [],
          capabilities: ["filesystem_read", "filesystem_write", "provider_mutation"],
          issued_by_ref: principalRef,
          credential_ref: { type: "credential", uri: "runx:credential:github_installation" },
        }],
        enforcement: {
          profile_hash: "sha256:profile",
          redaction_refs: [{ type: "redaction_policy", uri: "runx:redaction_policy:public_safe" }],
          setup_refs: [],
          teardown_refs: [],
        },
      },
      signals: [{ type: "signal", uri: "runx:signal:sig_101" }],
      decisions: [{
        decision_id: "dec_revision",
        choice: "open",
        inputs: {
          signal_refs: [{ type: "signal", uri: "runx:signal:sig_101" }],
          target_ref: null,
          opportunity_refs: [],
          selection_ref: null,
        },
        proposed_intent: intent,
        selected_act_id: "act_revision",
        selected_harness_ref: null,
        justification: {
          summary: "The authenticated issue authorizes a bounded checkout revision.",
          evidence_refs: [issueRef],
        },
        closure: null,
        artifact_refs: [],
      }],
      acts: [{
        id: "act_revision",
        form: "revision",
        intent,
        summary: "Prepared a reviewable checkout retry revision.",
        criterion_bindings: [{
          criterion_id: "crit_revision_reviewable",
          status: "verified",
          evidence_refs: [{ type: "github_pull_request", uri: "github://runxhq/example/pulls/102" }],
          verification_refs: [{ type: "verification", uri: "runx:verification:check_pr_open" }],
          summary: "Reviewable PR observed.",
        }],
        source_refs: [issueRef],
        target_refs: [{ type: "github_repo", uri: "github://runxhq/example" }],
        artifact_refs: [{ type: "artifact", uri: "runx:artifact:summary_1" }],
        context_ref: { type: "act", uri: "runx:act:act_revision_context" },
        closure: {
          disposition: "closed",
          reason_code: "revision_ready",
          summary: "Revision act completed.",
          closed_at: "2026-05-18T00:03:00Z",
        },
        revision: {
          change_request: {
            request_id: "cr_checkout_retry",
            summary: "Fix checkout retry behavior.",
            target_surfaces: [],
            success_criteria: [criterion],
          },
          change_plan: {
            plan_id: "cp_checkout_retry",
            summary: "Adjust retry guard and open a PR.",
            steps: ["Edit retry guard", "Open PR"],
          },
          target_surfaces: [],
          invariants: [],
          verification,
          handoff_refs: [],
        },
      }],
      seal,
      lineage: {
        children: [],
        sync: [],
      },
    })).toMatchObject({
      schema: "runx.receipt.v1",
      acts: [expect.objectContaining({ id: "act_revision" })],
    });

    expect(() => validateReceiptContract({
      schema: "runx.receipt.v1",
      id: "hrn_rcpt_bad",
    })).toThrow(/receipt\/v1\.json/);
    expect(() => validateSignalContract({
      schema: "runx.signal.v1",
      signal_id: "sig_bad",
      source_ref: { type: `${"evidence"}_bundle`, uri: `runx:${"evidence"}_bundle:old` },
      signal_type: "issue_opened",
      title: "Old evidence bundle ref",
      observed_at: "2026-05-18T00:00:00Z",
    })).toThrow(/signal\/v1\.json/);
  });

  it("validates auxiliary schema payloads", () => {
    expect(validateReviewReceiptOutputContract({
      verdict: "pass",
      failure_summary: "No harness failure.",
      improvement_proposals: [],
      next_harness_checks: ["runx harness"],
    })).toMatchObject({ verdict: "pass" });

    expect(validateRegistryBindingContract({
      schema: "runx.registry_binding.v1",
      state: "registry_bound",
      skill: {
        id: "runx/sourcey",
        name: "sourcey",
        description: "Docs skill.",
      },
      upstream: {
        host: "github.com",
        owner: "runxhq",
        repo: "runx",
        path: "skills/sourcey",
        commit: "abc123",
        blob_sha: "def456",
        source_of_truth: true,
      },
      registry: {
        owner: "runx",
        trust_tier: "first_party",
        version: "1.0.0",
        profile_path: "X.yaml",
        materialized_package_is_registry_artifact: true,
      },
      harness: {
        status: "harness_verified",
        case_count: 1,
      },
    })).toMatchObject({ schema: "runx.registry_binding.v1" });

    expect(() => validateRegistryBindingContract({
      schema: "runx.registry_binding.v1",
      state: "registry_bound",
      skill: {
        id: "runx/sourcey",
        name: "sourcey",
        description: "Docs skill.",
      },
      upstream: {
        host: "github.com",
        owner: "runxhq",
        repo: "runx",
        path: "skills/sourcey",
        commit: "abc123",
        blob_sha: "def456",
        source_of_truth: false,
      },
      registry: {
        owner: "runx",
        trust_tier: "first_party",
        version: "1.0.0",
        profile_path: "X.yaml",
        materialized_package_is_registry_artifact: true,
      },
      harness: {
        status: "harness_verified",
        case_count: 1,
      },
    })).toThrow(/registry-binding\.schema\.json/);
  });

  it("owns generic post-handoff contracts for reusable outreach state", () => {
    expect(RUNX_CONTRACT_IDS.handoffSignal).toBe("https://schemas.runx.ai/runx/handoff-signal/v1.json");
    expect(runxContractSchemas.handoffSignal.$id).toBe(RUNX_CONTRACT_IDS.handoffSignal);
    expect(validateHandoffSignalContract({
      schema: "runx.handoff_signal.v1",
      signal_id: "sig_1",
      handoff_id: "docs-pr:example/repo:001",
      boundary_kind: "external_maintainer",
      target_repo: "example/repo",
      target_locator: "github://example/repo/pulls/42",
      thread_locator: "github://example/repo/pulls/42",
      outbox_entry_id: "pull_request:docs-refresh-example-repo",
      source: "pull_request_comment",
      disposition: "requested_changes",
      recorded_at: "2026-04-24T02:30:00Z",
      actor: {
        actor_id: "maintainer",
        role: "maintainer",
      },
      source_ref: {
        type: "provider_comment",
        uri: "https://github.com/example/repo/pull/42#issuecomment-1",
      },
    })).toMatchObject({
      handoff_id: "docs-pr:example/repo:001",
      disposition: "requested_changes",
    });

    expect(validateHandoffStateContract({
      schema: "runx.handoff_state.v1",
      handoff_id: "docs-pr:example/repo:001",
      target_repo: "example/repo",
      status: "needs_revision",
      signal_count: 2,
      last_signal_id: "sig_1",
      last_signal_at: "2026-04-24T02:30:00Z",
      last_signal_disposition: "requested_changes",
    })).toMatchObject({
      status: "needs_revision",
      signal_count: 2,
    });

    expect(validateSuppressionRecordContract({
      schema: "runx.suppression_record.v1",
      record_id: "sup_1",
      scope: "contact",
      key: "mailto:maintainer@example.org",
      reason: "requested_no_contact",
      recorded_at: "2026-04-24T02:31:00Z",
      source_signal_id: "sig_2",
    })).toMatchObject({
      scope: "contact",
      reason: "requested_no_contact",
    });
  });

  it("owns a generic act assignment envelope contract for host-neutral invocation", () => {
    expect(RUNX_CONTRACT_IDS.actAssignment).toBe("https://schemas.runx.ai/runx/act-assignment/v1.json");
    expect(runxContractSchemas.actAssignment.$id).toBe(RUNX_CONTRACT_IDS.actAssignment);

    expect(validateActAssignmentContract({
      schema: "runx.act_assignment.v1",
      skill_ref: "outreach",
      runner: "rerun",
      source_ref: "github://sourcey/sourcey.com/issues/3",
      requested_at: "2026-04-25T13:45:00Z",
      host: {
        kind: "github_issue_comment",
        trigger_ref: "https://github.com/sourcey/sourcey.com/issues/3#issuecomment-1",
        scope_set: ["docs.write", "thread:push"],
        actor: {
          actor_id: "auscaster",
          display_name: "auscaster",
          provider_identity: "github:auscaster",
        },
      },
      input_overrides: {
        objective: "Refresh the MCP-first docs preview.",
        bind_current: true,
      },
      idempotency: {
        algorithm: "sha256",
        intent_key: "sha256:intent",
        trigger_key: "sha256:trigger",
        content_hash: "sha256:content",
      },
    })).toMatchObject({
      skill_ref: "outreach",
      runner: "rerun",
      host: {
        kind: "github_issue_comment",
      },
      idempotency: {
        algorithm: "sha256",
      },
    });
  });

  it("routes public schema validation through Rust-generated artifacts", () => {
    const rustAuthoritativeAssignment = {
      schema: "runx.act_assignment.v1",
      skill_ref: "outreach",
      runner: "rerun",
      requested_at: "2026-04-25T13:45:00Z",
      host: {
        kind: "api",
        trigger_ref: "",
        scope_set: [""],
        actor: {
          actor_id: "",
        },
      },
      idempotency: {
        algorithm: "sha256",
        intent_key: "sha256:intent",
        content_hash: "sha256:content",
      },
    };

    expect(contractSchemaMatches(runxContractSchemas.actAssignment, rustAuthoritativeAssignment)).toBe(true);
    expect(contractSchemaMatches(actAssignmentV1Schema, rustAuthoritativeAssignment)).toBe(true);
    expect(validateActAssignmentContract(rustAuthoritativeAssignment)).toMatchObject({
      host: {
        kind: "api",
      },
    });
  });

  it("validates machine report payloads from Rust-generated contract schemas", () => {
    expect(validateDoctorReportContract({
      schema: RUNX_LOGICAL_SCHEMAS.doctor,
      status: "success",
      summary: {
        errors: 0,
        warnings: 1,
        infos: 0,
      },
      diagnostics: [{
        id: "runx.tool.fixture.missing",
        instance_id: "sha256:fixture",
        severity: "warning",
        title: "Missing fixture",
        message: "Tool has no deterministic fixture.",
        target: {
          kind: "tool",
          ref: "demo.echo",
        },
        location: {
          path: "tools/demo/echo/manifest.json",
        },
        repairs: [],
      }],
    })).toMatchObject({
      status: "success",
      diagnostics: [expect.objectContaining({ id: "runx.tool.fixture.missing" })],
    });

    expect(validateRunxListReportContract({
      schema: RUNX_LOGICAL_SCHEMAS.list,
      root: "/tmp/runx",
      requested_kind: "all",
      items: [{
        kind: "tool",
        name: "demo.echo",
        source: "local",
        path: "tools/demo/echo/manifest.json",
        status: "ok",
        scopes: ["repo:status"],
        emits: [{ name: "result", packet: "demo.result" }],
        fixtures: 1,
      }],
    })).toMatchObject({
      requested_kind: "all",
      items: [expect.objectContaining({ kind: "tool", fixtures: 1 })],
    });

    expect(validateDevReportContract({
      schema: RUNX_LOGICAL_SCHEMAS.dev,
      status: "success",
      doctor: {
        schema: RUNX_LOGICAL_SCHEMAS.doctor,
        status: "success",
        summary: {
          errors: 0,
          warnings: 0,
          infos: 0,
        },
        diagnostics: [],
      },
      fixtures: [{
        name: "demo-fixture",
        lane: "deterministic",
        target: {
          kind: "tool",
        },
        status: "success",
        duration_ms: 12,
        assertions: [],
      }],
      receipt_id: "rx_123",
    })).toMatchObject({
      status: "success",
      receipt_id: "rx_123",
    });
  });

});
