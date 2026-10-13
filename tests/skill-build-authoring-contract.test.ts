import { readdir, readFile } from "node:fs/promises";
import path from "node:path";
import { describe, expect, it } from "vitest";

import {
  validateSkillArchitectureDecisionContract,
  validateSkillChangeDraftContract,
} from "@runxhq/contracts";
import {
  validateHarnessFixtureYamlBatch,
  validateRunnerManifestYaml,
} from "../scripts/lib/native-parser.mjs";

const root = process.cwd();
const canonicalPackets = [
  ["skill-architecture-decision.schema.json", "runx.skill.architecture.decision.v1.schema.json", "runx.skill.architecture_decision.v1"],
  ["skill-architecture-plan.schema.json", "runx.skill.architecture.plan.v1.schema.json", "runx.skill.architecture_plan.v1"],
  ["skill-change-draft.schema.json", "runx.skill.change.draft.v1.schema.json", "runx.skill.change_draft.v1"],
  ["skill-change-bundle.schema.json", "runx.skill.change.bundle.v1.schema.json", "runx.skill.change_bundle.v1"],
  ["skill-validation-result.schema.json", "runx.skill.validation.result.v1.schema.json", "runx.skill.validation_result.v1"],
  ["skill-apply-result.schema.json", "runx.skill.apply.result.v1.schema.json", "runx.skill.apply_result.v1"],
] as const;

describe("Skill Build authoring ownership", () => {
  it("projects every Rust-owned closed contract into one packet schema", async () => {
    for (const [schemaFile, packetFile, packetId] of canonicalPackets) {
      const canonical = await json(path.join(root, "schemas", schemaFile));
      assertClosedObjects(canonical, schemaFile);
      const packet = await json(path.join(root, "dist/packets", packetFile));
      expect(packet["x-runx-packet-id"]).toBe(packetId);
      expect(packet["x-runx-generated-from"]).toBe(`schemas/${schemaFile}`);
      delete packet["x-runx-packet-id"];
      delete packet["x-runx-generated-from"];
      expect(packet).toEqual(canonical);
    }
  });

  it("rejects unknown fields in nested architecture and draft values", () => {
    const architecture = architectureDecision();
    expect(() => validateSkillArchitectureDecisionContract(architecture)).not.toThrow();
    architecture.knowledge_contract.unowned = true;
    expect(() => validateSkillArchitectureDecisionContract(architecture)).toThrow(/knowledge_contract/);

    const draft = changeDraft();
    expect(() => validateSkillChangeDraftContract(draft)).not.toThrow();
    draft.writes[0]!.mode = "executable";
    expect(() => validateSkillChangeDraftContract(draft)).toThrow(/writes/);
  });

  it("keeps hashes native and uses architect-plan-author-bind-apply", async () => {
    const profile = validateRunnerManifestYaml(
      await readFile(path.join(root, "skills/skill-build/run.yaml"), "utf8"),
    ).raw.document as Record<string, any>;
    expect(stepIds(profile.runners.design)).toEqual(["inspect", "architect", "plan"]);
    for (const runnerName of ["build", "improve", "harness"]) {
      expect(stepIds(profile.runners[runnerName])).toEqual([
        "inspect",
        "architect",
        "plan",
        "author",
        "bind",
        "apply",
      ]);
      expect(step(profile.runners[runnerName], "plan").tool).toBe("runx.skill.plan");
      expect(step(profile.runners[runnerName], "bind").tool).toBe("runx.skill.bind");
      expect(step(profile.runners[runnerName], "apply").tool).toBe("runx.skill.apply");
      expect(step(profile.runners[runnerName], "author").artifacts.packets.change_draft)
        .toBe("runx.skill.change_draft.v1");
    }

    const fixtureRoot = path.join(root, "skills/skill-build/fixtures");
    const fixtureFiles = (await readdir(fixtureRoot)).filter((entry) => entry.endsWith(".yaml"));
    const fixtures = validateHarnessFixtureYamlBatch(
      await Promise.all(fixtureFiles.map((file) => readFile(path.join(fixtureRoot, file), "utf8"))),
    ) as Array<Record<string, any>>;
    for (const fixture of fixtures) {
      const answers = fixture.caller.answers;
      expect(answers["architect"].architecture_decision.schema)
        .toBe("runx.skill.architecture_decision.v1");
      const draft = answers["author"].change_draft;
      expect(draft.schema).toBe("runx.skill.change_draft.v1");
      expect(draft).not.toHaveProperty("base_digest");
      expect(draft).not.toHaveProperty("plan_digest");
      expect(draft).not.toHaveProperty("architecture");
    }
  });

  it("preserves the substantive operator manual", async () => {
    const manual = (await readFile(path.join(root, "skills/skill-build/SKILL.md"), "utf8"))
      .replace(/\s+/g, " ");
    for (const meaning of [
      "## Authoring rules",
      "## Agent task contracts",
      "required evidence",
      "stop conditions",
      "resource ceilings",
      "Never copy or calculate `base_digest`",
      "no workspace path",
    ]) {
      expect(manual).toContain(meaning);
    }
  });
});

function stepIds(runner: Record<string, any>): string[] {
  return runner.steps.map((value: Record<string, any>) => value.id);
}

function step(runner: Record<string, any>, id: string): Record<string, any> {
  return runner.steps.find((value: Record<string, any>) => value.id === id);
}

async function json(file: string): Promise<Record<string, any>> {
  return JSON.parse(await readFile(file, "utf8")) as Record<string, any>;
}

function assertClosedObjects(value: unknown, location: string): void {
  if (Array.isArray(value)) {
    value.forEach((child, index) => assertClosedObjects(child, `${location}[${index}]`));
    return;
  }
  if (!value || typeof value !== "object") return;
  const record = value as Record<string, unknown>;
  if (record.type === "object") {
    expect(record.additionalProperties, location).toBe(false);
  }
  Object.entries(record).forEach(([key, child]) => assertClosedObjects(child, `${location}.${key}`));
}

function architectureDecision(): Record<string, any> {
  return {
    schema: "runx.skill.architecture_decision.v1",
    disposition: "build",
    identity: {
      proposed_name: "demo",
      action: "create",
      visibility: "public",
      rationale: "Demo is the natural operation name.",
    },
    direct_use: {
      trigger_requests: ["Make a bounded demo decision."],
      non_trigger_requests: ["Publish this decision."],
      default_outcome: "Return one bounded decision.",
      routine_host_work: ["Inspect the supplied objective."],
      runx_boundary: "Bind result evidence in a receipt.",
      terminal_result: "A reviewable demo decision.",
      blocker_behavior: "Block once and name missing evidence.",
      native_escape: "Return gathered evidence for native continuation.",
    },
    chain_use: {
      accepted_inputs: ["A supplied objective or prior evidence packet."],
      result: "A reusable demo decision.",
      reused_evidence: ["Prior objective evidence."],
      reused_effects: [],
      must_not_repeat: ["Do not rediscover supplied evidence."],
    },
    objective: "Create one bounded skill.",
    operator_value: "Give the operator one reviewable outcome.",
    knowledge_contract: {
      purpose: "Explain the bounded operation.",
      evidence_required: ["A supplied objective."],
      decision_logic: ["Preserve supplied evidence."],
      stop_conditions: ["Stop when evidence is missing."],
      recovery: ["Resume with the missing evidence."],
    },
    required_behaviors: [{ id: "guide", outcome: "Explain the operation.", lane: "manual" }],
    native_reuse: { inspected_capabilities: [], selected_capabilities: [], missing_capabilities: [] },
    effects: [{ effect: "none", authority_scopes: [], approval: "none", provider_boundary: false }],
    skill_chain: { context_skills: [], routes: [] },
    resource_budget: {
      max_files: 2,
      max_executable_lines: 0,
      max_process_spawns: 0,
      network_allowed: false,
    },
    preservation_obligations: ["Keep the manual substantive."],
    deletions: [],
    proof_plan: [
      {
        name: "cold-selection",
        kind: "selection_trial",
        expected: "The natural request selects demo and a publish request does not.",
      },
      {
        name: "standalone-result",
        kind: "standalone_operator_journey",
        expected: "The direct request returns a decision.",
      },
      {
        name: "composed-reuse",
        kind: "composed_operator_journey",
        expected: "Prior evidence is reused without rediscovery.",
      },
    ],
  };
}

function changeDraft(): Record<string, any> {
  return {
    schema: "runx.skill.change_draft.v1",
    decision: "write",
    summary: "Create the package.",
    non_goals: ["Do not publish."],
    writes: [{ path: "SKILL.md", contents: "---\nname: demo\n---\n" }],
    deletes: [],
    expected_outputs: [],
  };
}
