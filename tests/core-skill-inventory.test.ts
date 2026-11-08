import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";

import { expect, it } from "vitest";

import { repositorySkills } from "../scripts/lib/skills.mjs";

it("keeps an invalid unselected package outside the core validation boundary", () => {
  const root = mkdtempSync(path.join(tmpdir(), "runx-core-inventory-"));
  const writeSkill = (name: string, profile: string) => {
    const directory = path.join(root, "skills", name);
    mkdirSync(directory, { recursive: true });
    writeFileSync(path.join(directory, "SKILL.md"), `---\nname: ${name}\ndescription: The ${name} fixture.\n---\n# ${name}\n`);
    writeFileSync(path.join(directory, "run.yaml"), profile);
  };
  try {
    writeSkill("core", "minimum_runx: 0.12.0\nmethods:\n  default:\n    steps:\n      - id: echo\n        tool: { command: node, args: [run.mjs] }\n");
    writeSkill("vendor", "methods: [not valid YAML");

    expect(repositorySkills(root, process.env, new Set(["core"])))
      .toEqual([{ name: "core", visibility: "internal" }]);
    expect(() => repositorySkills(root, process.env, new Set(["vendor"]))).toThrow();
    expect(() => repositorySkills(root)).toThrow();
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
