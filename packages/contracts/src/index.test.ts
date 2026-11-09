import { describe, expect, it } from "vitest";

import { type JsonSchema } from "./internal.js";

import {
  agentContextEnvelopeSchema,
  turnInvocationSchema,
  approvalGateSchema,
  storeResultV1Schema,
  doctorV1Schema,
  listV1Schema,
  outputSchema,
  questionSchema,
  resolutionRequestSchema,
  resolutionResponseSchema,
  receiptV1Schema,
  paidSkillListingV1Schema,
  purchaseV1Schema,
  quotePurchaseResultV1Schema,
  purchaseReadV1Schema,
  runxContractSchemas,
  runxGeneratedSchemaArtifacts,
  toolManifestV1Schema,
  validateAgentContextEnvelopeContract,
  validateOutputContract,
  validateResolutionRequestContract,
  validateStoreResultContract,
  validateDoctorReportContract,
  validateReceiptContract,
  validateRunxListReportContract,
  validateOperationalPolicyContract,
  validateReferenceContract,
  proofKindSchema,
  validateContractSchema,
} from "./index.js";

describe("@runxhq/contracts", () => {
  it("uses durable schema URI ids", () => {
    expect(runxContractSchemas.toolManifest.$id).toBe("https://schemas.runx.ai/runx/tool/manifest/v1.json");
    expect(toolManifestV1Schema).toBe(runxContractSchemas.toolManifest);
    expect(runxContractSchemas.storeResult.$id)
      .toBe("https://schemas.runx.ai/runx/store/result/v1.json");
    expect((toolManifestV1Schema.properties as Record<string, unknown>).process).toBeDefined();
    expect((toolManifestV1Schema.required as readonly string[])).not.toContain("version");
  });

  it("publishes purchase V1 through the curated contract registry", () => {
    expect(paidSkillListingV1Schema).toBe(runxContractSchemas.paidSkillListing);
    expect(purchaseV1Schema).toBe(runxContractSchemas.purchase);
    expect(quotePurchaseResultV1Schema).toBe(runxContractSchemas.quotePurchaseResult);
    expect(purchaseReadV1Schema).toBe(runxContractSchemas.purchaseRead);
  });

  it("exports Rust-generated artifacts for control schema facades", () => {
    expect(outputSchema).toBe(runxContractSchemas.output);
    expect(agentContextEnvelopeSchema).toBe(runxContractSchemas.agentContextEnvelope);
    expect(turnInvocationSchema).toBe(runxContractSchemas.turnInvocation);
    expect(questionSchema).toBe(runxContractSchemas.question);
    expect(approvalGateSchema).toBe(runxContractSchemas.approvalGate);
    expect(resolutionRequestSchema).toBe(runxContractSchemas.resolutionRequest);
    expect(resolutionResponseSchema).toBe(runxContractSchemas.resolutionResponse);
    expect(storeResultV1Schema).toBe(runxContractSchemas.storeResult);
    expect(receiptV1Schema).toBe(runxContractSchemas.receipt);
    expect(doctorV1Schema).toBe(runxContractSchemas.doctor);
    expect(listV1Schema).toBe(runxContractSchemas.list);
  });

  it("validates store results, one shape per operation", () => {
    const event = {
      id: "21:project:board/items,3:a-1,3",
      version: 3,
      type: "item.labelled",
      data: { label: "three" },
      digest: "sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
      idempotency_key: "runx:sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
      committed_at: "2026-11-09T00:00:00.000Z",
      writer: { package: "project:board", digest: "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb" },
    };
    const common = {
      schema: "runx.store.result.v1",
      stream: "project:board/items",
      backend: "postgres",
      result_digest: "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    };
    const committed = validateStoreResultContract({
      ...common,
      operation: "events.append",
      key: "a-1",
      status: "committed",
      before_version: 2,
      version: 3,
      event: { id: event.id, digest: event.digest },
      chain_digest: "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
    });
    expect(committed).toMatchObject({ status: "committed", backend: "postgres" });

    const conflict = validateStoreResultContract({
      ...common,
      operation: "events.append",
      key: "a-1",
      status: "version_conflict",
      before_version: 4,
      version: 4,
      chain_digest: "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
    });
    expect(conflict).toMatchObject({ status: "version_conflict" });
    // A conflict appends nothing, so it names no event.
    expect(() =>
      validateStoreResultContract({ ...conflict, event: { id: event.id, digest: event.digest } }),
    ).toThrow();

    expect(
      validateStoreResultContract({
        ...common,
        operation: "key.inspect",
        key: "a-1",
        version: 3,
        last_event: event,
        chain_digest: "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
      }),
    ).toMatchObject({ operation: "key.inspect" });
    expect(
      validateStoreResultContract({
        ...common,
        operation: "keys.list",
        keys: [{ key: "a-1", last_event: event }],
        next_cursor: null,
      }),
    ).toMatchObject({ next_cursor: null });
    // Today's names are refused.
    expect(() =>
      validateStoreResultContract({ ...committed, data_source_uri: "tenant://acme/board" }),
    ).toThrow();
  });

  it("accepts typed proof kinds on references", () => {
    expect(proofKindSchema).toEqual({
      title: "ProofKind",
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

  it("owns executor control protocol schemas and runtime validation", () => {
    expect(outputSchema.$id).toBe("https://schemas.runx.ai/runx/output/v1.json");
    expect(agentContextEnvelopeSchema.$id).toBe("https://schemas.runx.ai/runx/agent-context-envelope/v1.json");
    expect(runxGeneratedSchemaArtifacts["output.schema.json"].$id).toBe(outputSchema.$id);
    expect(runxGeneratedSchemaArtifacts["agent-context-envelope.schema.json"].$id).toBe(agentContextEnvelopeSchema.$id);

    expect(validateOutputContract({
      summary: { type: "string" },
      verdict: { type: "string", enum: ["pass", "fail"] },
      note: { type: "string", optional: true },
      plan: { type: "object", packet: "runx/github.sync_plan.v1" },
    })).toMatchObject({
      summary: { type: "string" },
    });
    // A field is its JSON Schema: a bare type name is not one.
    expect(() => validateOutputContract({ summary: "string" })).toThrow();

    expect(validateAgentContextEnvelopeContract({
      run_id: "rx_contract",
      skill: "demo.skill",
      instructions_digest: `sha256:${"a".repeat(64)}`,
      instructions: "Do the work.",
      inputs: {},
      requirements: {
        execution_boundary: { kind: "remote_provider" },
      },
      current_context: [],
      historical_context: [],
      provenance: [],
      output: {
        summary: { type: "string" },
      },
      trust_boundary: "test",
    })).toMatchObject({
      run_id: "rx_contract",
      output: {
        summary: { type: "string" },
      },
    });

    expect(() => validateAgentContextEnvelopeContract({
      run_id: "rx_contract",
      skill: "demo.skill",
      instructions_digest: `sha256:${"a".repeat(64)}`,
      instructions: "Do the work.",
      inputs: {},
      requirements: {
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
      instructions_digest: `sha256:${"a".repeat(64)}`,
      instructions: "Do the work.",
      inputs: {},
      requirements: {
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

    expect(validateResolutionRequestContract({
      id: "approval.demo",
      kind: "approval",
      gate: {
        id: "gate.demo",
        reason: "Needs human approval.",
      },
    })).toMatchObject({ kind: "approval" });
  });

  it("owns generated auxiliary schemas", () => {
    expect(runxGeneratedSchemaArtifacts["doctor.schema.json"]).toBe(runxContractSchemas.doctor);
    expect(runxGeneratedSchemaArtifacts["receipt.schema.json"]).toBe(runxContractSchemas.receipt);
    expect(runxGeneratedSchemaArtifacts["operational-policy.schema.json"]).toBe(runxContractSchemas.operationalPolicy);
  });

  it("owns operational policy for issue intake and source-thread routing", () => {
    expect(runxContractSchemas.operationalPolicy.$id).toBe("https://schemas.runx.ai/runx/operational-policy/v1.json");
    const policy = {
      schema: "runx.operational_policy.v1",
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
        strategy: "source_digest",
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
      permissions: {
        auto_merge: false,
      },
    });
  });

  it("validates a governed call's receipt", () => {
    const issueRef = {
      type: "thread",
      uri: "github://runxhq/example/issues/101",
      provider: "github",
      locator: "runxhq/example#101",
      observed_at: "2026-05-18T00:00:00Z",
    };
    const principalRef = { type: "principal", uri: "runx:principal:agent_1" };
    const outcome = {
      criterion_id: "step_outcome",
      status: "verified",
      evidence_refs: [],
      verification_refs: [{ type: "verification", uri: "runx:provider_ack:comment_1", proof_kind: "effect_evidence" }],
    };

    const receipt = {
      schema: "runx.receipt.v1",
      id: "rcpt_123",
      created_at: "2026-05-18T00:03:01Z",
      canonicalization: "runx.receipt.c14n.v1",
      issuer: {
        type: "local",
        kid: "key_1",
        public_key_digest: "sha256:key",
        runx_version: "0.11.0",
      },
      signature: {
        alg: "Ed25519",
        value: "sig_123",
      },
      digest: "sha256:receipt",
      subject: {
        kind: "step",
        ref: { type: "run", uri: "runx:run:run_123#reply@1" },
        outcome_digest: `sha256:${"3".repeat(64)}`,
      },
      authority: {
        actor_ref: principalRef,
        grant_refs: [{ type: "grant", uri: "runx:grant:repo_write" }],
        scope_refs: [{ type: "scope_admission", uri: "runx:scope_admission:issues:write" }],
        terms: [{
          term_id: "term_reply",
          principal_ref: principalRef,
          resource_ref: { type: "connector_instance", uri: "runx:connector_instance:github.default" },
          verbs: ["write"],
          bounds: { operations: ["github/comments.create"] },
          issued_by_ref: { type: "host", uri: "runx:host:runx-kernel" },
        }],
        derivations: [],
        enforcement: { execution_boundary: { kind: "remote_provider" } },
      },
      decisions: [{
        decision_id: "dec_reply",
        choice: "continue",
        selected_act_id: "act_reply",
      }],
      acts: [{
        id: "act_reply",
        form: "reply",
        intent: {
          purpose: "Answer the question on the thread.",
          legitimacy: "runx:authority_term:term_reply",
        },
        reason: "The reply names the release that fixes the crash.",
        criterion_bindings: [outcome],
        target_refs: [issueRef],
        artifact_refs: [{ type: "comment", uri: "github://runxhq/example/issues/comments/1", provider: "github" }],
      }],
      seal: {
        outcome: "completed",
        criteria: [outcome],
      },
      lineage: {
        children: [],
      },
    };
    expect(validateReceiptContract(receipt)).toMatchObject({
      schema: "runx.receipt.v1",
      acts: [expect.objectContaining({ id: "act_reply" })],
    });
    expect(() => validateReceiptContract({ ...receipt, signals: [] })).toThrow(/receipt\/v1\.json/);
    // The runtime writes typed facts: no summary, no criterion statement, and
    // every array present.
    expect(() => validateReceiptContract({ ...receipt, seal: { ...receipt.seal, summary: "step completed" } }))
      .toThrow(/receipt\/v1\.json/);
    expect(() => validateReceiptContract({
      ...receipt,
      seal: { ...receipt.seal, criteria: [{ ...outcome, criterion_id: "release_named" }] },
    })).toThrow(/receipt\/v1\.json/);
    const { decisions: _decisions, ...withoutDecisions } = receipt;
    expect(() => validateReceiptContract(withoutDecisions)).toThrow(/receipt\/v1\.json/);

    expect(() => validateReceiptContract({
      schema: "runx.receipt.v1",
      id: "rcpt_bad",
    })).toThrow(/receipt\/v1\.json/);
    expect(() => validateReferenceContract({
      type: `${"evidence"}_bundle`,
      uri: `runx:${"evidence"}_bundle:old`,
    })).toThrow(/reference\/v1\.json/);
  });

  it("routes public schema validation through Rust-generated artifacts", () => {
    // A schema naming a generated artifact's $id validates with the artifact,
    // never with the copy it was handed.
    const stale = { $id: runxContractSchemas.reference.$id, type: "string" } as JsonSchema;
    const reference = { type: "receipt", uri: "runx:receipt:rx_1" };
    expect(validateContractSchema(stale, reference, "reference")).toEqual(reference);
    expect(() => validateContractSchema(stale, "runx:receipt:rx_1", "reference"))
      .toThrow(/runx\/reference\/v1\.json/);
  });

  it("validates a host's own schema that refers to the published definitions", () => {
    // A host keeps a schema of its own that refers to types runx publishes
    // once, so it validates against runx's whole published set.
    const hostSchema = {
      $schema: "https://json-schema.org/draft/2020-12/schema",
      $id: "https://host.example.test/digested/v1.json",
      type: "object",
      additionalProperties: false,
      required: ["digest"],
      properties: { digest: { $ref: "https://schemas.runx.ai/runx/definitions/v1.json#/$defs/ContentDigest" } },
    } as JsonSchema;
    const digested = { digest: { algorithm: "sha256", canonicalization: "runx.c14n.v1", value: "abc", derived_from: [] } };
    expect(validateContractSchema(hostSchema, digested, "digested")).toEqual(digested);
    expect(() => validateContractSchema(hostSchema, { digest: { ...digested.digest, algorithm: "md5" } }, "digested"))
      .toThrow(/digested\.digest\.algorithm must match https:\/\/host\.example\.test\/digested\/v1\.json/);
  });

  it("validates machine report payloads from Rust-generated contract schemas", () => {
    expect(validateDoctorReportContract({
      schema: "runx.doctor.v1",
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
      schema: "runx.list.v1",
      root: "/tmp/runx",
      requested_kind: "all",
      items: [{
        kind: "tool",
        name: "demo.echo",
        source: "local",
        path: "tools/demo/echo/manifest.json",
        status: "ok",
        packets: [{ name: "result", packet: "acme/demo.result.v1" }],
        fixtures: 1,
      }],
    })).toMatchObject({
      requested_kind: "all",
      items: [expect.objectContaining({ kind: "tool", fixtures: 1 })],
    });
  });

});
