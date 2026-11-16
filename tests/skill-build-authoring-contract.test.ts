import { readdir, readFile } from "node:fs/promises";
import path from "node:path";
import { describe, expect, it } from "vitest";

import { validateSkillChangeDraftContract } from "@runxhq/contracts";
import {
  validateTestFixtureYamlBatch,
  validateMethodManifestYaml,
} from "../scripts/lib/native-parser.mjs";

const root = process.cwd();
const canonicalPackets = [
  ["skill-change-draft.schema.json", "runx.skill.change_draft.v1"],
  ["skill-change-bundle.schema.json", "runx.skill.change_bundle.v1"],
  ["skill-validation-result.schema.json", "runx.skill.validation_result.v1"],
  ["skill-apply-result.schema.json", "runx.skill.apply_result.v1"],
] as const;

describe("Skill Build authoring ownership", () => {
  it("publishes every Rust-owned closed contract as one core packet", async () => {
    for (const [schemaFile, packetId] of canonicalPackets) {
      const canonical = await json(path.join(root, "schemas", schemaFile));
      assertClosedObjects(canonical, schemaFile);
      expect(canonical["x-runx-packet"]).toBe(true);
      expect(canonical["x-runx-schema"]).toBe(packetId);
    }
  });

  it("rejects unknown fields in a draft", () => {
    const draft = changeDraft();
    expect(() => validateSkillChangeDraftContract(draft)).not.toThrow();
    draft.writes[0]!.mode = "executable";
    expect(() => validateSkillChangeDraftContract(draft)).toThrow(/writes/);
  });

  it("authors in one turn, then binds and checks natively", async () => {
    const profile = validateMethodManifestYaml({
      skill: "skill-build",
      yaml: await readFile(path.join(root, "skills/skill-build/run.yaml"), "utf8"),
    }).raw.document as Record<string, any>;
    const methods = profile.methods;
    expect(stepIds(methods.design)).toEqual(["inspect", "author", "bind", "check"]);
    expect(stepIds(methods.draft)).toEqual(["inspect", "author", "bind"]);
    expect(stepIds(methods.improve)).toEqual(["draft", "apply"]);
    for (const methodName of ["build", "test"]) {
      expect(stepIds(methods[methodName])).toEqual(["inspect", "author", "bind", "apply"]);
    }
    for (const methodName of ["design", "build", "draft", "test"]) {
      const method = methods[methodName];
      expect(step(method, "bind").tool).toBe("runx.skill.bind");
      expect(step(method, "author").outputs.change_draft.packet).toBe("runx.skill.change_draft.v1");
    }
    expect(step(methods.design, "check").tool).toBe("runx.skill.apply");
    expect(step(methods.design, "check").inputs.mode).toBe("design");

    const testRoot = path.join(root, "skills/skill-build/tests");
    const testFiles = (await readdir(testRoot)).filter((entry) => entry.endsWith(".yaml"));
    const tests = validateTestFixtureYamlBatch(
      await Promise.all(testFiles.map((file) => readFile(path.join(testRoot, file), "utf8"))),
    ) as Array<Record<string, any>>;
    expect(tests.length).toBeGreaterThan(0);
    for (const fixture of tests) {
      const answers = fixture.host?.answers ?? {};
      // A test refused before any turn answers none.
      if (Object.keys(answers).length === 0) continue;
      // One turn authors: it decides and writes, and never states a digest.
      expect(Object.keys(answers)).toEqual(["author"]);
      const draft = answers["author"].change_draft;
      expect(draft.schema).toBe("runx.skill.change_draft.v1");
      expect(draft).not.toHaveProperty("base_digest");
    }
  });

  it("preserves the substantive operator manual", async () => {
    const manual = (await readFile(path.join(root, "skills/skill-build/SKILL.md"), "utf8"))
      .replace(/\s+/g, " ");
    for (const meaning of [
      "## Authoring rules",
      "## Agent task: skill-build-author",
      "cold-operator trial",
      "Never copy or calculate a digest",
      "A change to a package keeps its name",
      "Every agent turn the change writes reads the person's memory",
      "no workspace path",
    ]) {
      expect(manual).toContain(meaning);
    }
  });
});

function stepIds(method: Record<string, any>): string[] {
  return method.steps.map((value: Record<string, any>) => value.id);
}

function step(method: Record<string, any>, id: string): Record<string, any> {
  return method.steps.find((value: Record<string, any>) => value.id === id);
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

function changeDraft(): Record<string, any> {
  return {
    schema: "runx.skill.change_draft.v1",
    decision: "write",
    summary: "Create the package.",
    writes: [{ path: "skills/demo/SKILL.md", contents: "---\nname: demo\n---\n" }],
    deletes: [],
  };
}
