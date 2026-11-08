import { existsSync, readdirSync, readFileSync } from "node:fs";
import { readFile } from "node:fs/promises";
import path from "node:path";

import { describe, expect, it } from "vitest";

import {
  validateTestFixtureYamlBatch,
  validateMethodManifestYaml as validateNativeMethodManifestYaml,
  validateMethodManifestYamlBatch as validateNativeMethodManifestYamlBatch,
  validateSkillMarkdownBatch as validateNativeSkillMarkdownBatch,
} from "../../scripts/lib/native-parser.mjs";

type SkillMethod = {
  readonly name: string;
  readonly graph: { readonly steps: readonly unknown[] };
};

type SkillMethodManifest = {
  readonly catalog?: Record<string, unknown>;
  readonly methods: Readonly<Record<string, SkillMethod>>;
  readonly raw: { readonly document: Record<string, unknown> };
};

describe("runx-maintained skill catalog", () => {
  it("ships runx-maintained skills as portable packages plus checked-in execution profiles", async () => {
    const packages = await Promise.all(runxMaintainedSkillPackages().map(async (skillName) => {
      const skillDir = path.resolve("skills", skillName);
      const skillMarkdownPath = path.join(skillDir, "SKILL.md");
      const manifestPath = path.join(skillDir, "run.yaml");

      expect(existsSync(skillDir)).toBe(true);
      expect(existsSync(skillMarkdownPath)).toBe(true);
      expect(existsSync(manifestPath)).toBe(true);
      return {
        skillName,
        markdown: await readFile(skillMarkdownPath, "utf8"),
        profile: await readFile(manifestPath, "utf8"),
      };
    }));
    const skills = validateNativeSkillMarkdownBatch(packages.map((entry) => entry.markdown)) as Array<{
      readonly name: string;
    }>;
    const manifests = validateNativeMethodManifestYamlBatch(
      packages.map((entry, index) => ({ skill: skills[index]!.name, yaml: entry.profile })),
    ) as SkillMethodManifest[];
    for (const [index, { skillName }] of packages.entries()) {
      const skill = skills[index];
      const manifest = manifests[index];
      expect(skill.name).toBe(skillName);
      expect(manifest.catalog).toBeDefined();
      expect(Object.keys(manifest.methods).length).toBeGreaterThan(0);
    }
  });

  it("keeps static agent instructions exclusively in SKILL.md", () => {
    for (const manifestPath of skillManifestPaths(path.resolve("skills"))) {
      const skillMarkdownPath = path.join(path.dirname(manifestPath), "SKILL.md");
      expect(existsSync(skillMarkdownPath), manifestPath).toBe(true);
      const manifest = validateMethodManifestYaml(path.dirname(manifestPath)).raw.document;
      expect(findObjectKeyPaths(manifest, "instructions"), manifestPath).toEqual([]);
    }
  });

  it("keeps public packages covered by executable proof", () => {
    for (const skillName of runxMaintainedSkillPackages()) {
      if (catalogVisibility(skillName) !== "public") {
        continue;
      }
      const tests = publicSkillTests(skillName);

      expect(tests.length, `${skillName} needs executable proof`).toBeGreaterThan(0);
      expect(
        tests
          .filter((entry) => entry.package !== undefined && entry.package !== null)
          .every((entry) => {
            // A test that composes skills runs a small package under fixtures/.
            const target = entry.package!;
            return !path.posix.isAbsolute(target)
              && target.startsWith("fixtures/")
              && !target.split("/").some((part) => part === "." || part === ".." || part === "")
              && existsSync(path.resolve("skills", skillName, target, "SKILL.md"))
              && existsSync(path.resolve("skills", skillName, target, "run.yaml"));
          }),
        `${skillName} tests must run this package or a fixture package inside it`,
      ).toBe(true);
    }
  });
});

function runxMaintainedSkillPackages(): readonly string[] {
  return readdirSync(path.resolve("skills"), { withFileTypes: true })
    .filter((entry) => entry.isDirectory())
    .filter((entry) => !entry.name.startsWith("."))
    .filter((entry) => existsSync(path.resolve("skills", entry.name, "SKILL.md")))
    .filter((entry) => existsSync(path.resolve("skills", entry.name, "run.yaml")))
    .map((entry) => entry.name)
    .sort();
}

function skillManifestPaths(root: string): readonly string[] {
  const paths: string[] = [];
  for (const entry of readdirSync(root, { withFileTypes: true })) {
    const entryPath = path.join(root, entry.name);
    if (entry.isDirectory()) {
      paths.push(...skillManifestPaths(entryPath));
    } else if (entry.name === "run.yaml") {
      paths.push(entryPath);
    }
  }
  return paths.sort();
}

function findObjectKeyPaths(value: unknown, key: string, prefix = "$"): readonly string[] {
  if (Array.isArray(value)) {
    return value.flatMap((entry, index) => findObjectKeyPaths(entry, key, `${prefix}[${index}]`));
  }
  if (!value || typeof value !== "object") {
    return [];
  }
  return Object.entries(value as Record<string, unknown>).flatMap(([name, entry]) => [
    ...(name === key ? [`${prefix}.${name}`] : []),
    ...findObjectKeyPaths(entry, key, `${prefix}.${name}`),
  ]);
}

function catalogVisibility(skillName: string): "public" | "internal" {
  const manifest = validateMethodManifestYaml(path.resolve("skills", skillName));
  const catalog = manifest.catalog as { readonly visibility?: "public" | "internal" } | undefined;
  return catalog?.visibility ?? "public";
}

type PublicSkillTest = {
  readonly name: string;
  readonly package?: string | null;
};

function publicSkillTests(skillName: string): readonly PublicSkillTest[] {
  const testsDir = path.resolve("skills", skillName, "tests");
  if (!existsSync(testsDir)) {
    return [];
  }
  const documents = readdirSync(testsDir)
    .filter((entry) => entry.endsWith(".yaml"))
    .sort()
    .map((entry) => readFileSync(path.join(testsDir, entry), "utf8"));
  return validateTestFixtureYamlBatch(documents) as PublicSkillTest[];
}

/** A package directory's `run.yaml`, validated as the package its `SKILL.md` names. */
function validateMethodManifestYaml(packageDir: string): SkillMethodManifest {
  const [skill] = validateNativeSkillMarkdownBatch([readFileSync(path.join(packageDir, "SKILL.md"), "utf8")]) as Array<{
    readonly name: string;
  }>;
  return validateNativeMethodManifestYaml({
    skill: skill!.name,
    yaml: readFileSync(path.join(packageDir, "run.yaml"), "utf8"),
  }) as SkillMethodManifest;
}
