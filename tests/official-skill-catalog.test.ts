import { existsSync, readdirSync, readFileSync } from "node:fs";
import { readFile } from "node:fs/promises";
import path from "node:path";

import { describe, expect, it } from "vitest";

import {
  validateHarnessFixtureYamlBatch,
  validateRunnerManifestYaml as validateNativeRunnerManifestYaml,
  validateRunnerManifestYamlBatch as validateNativeRunnerManifestYamlBatch,
  validateSkillMarkdownBatch as validateNativeSkillMarkdownBatch,
} from "../scripts/lib/native-parser.mjs";

type SkillRunner = {
  readonly name: string;
  readonly default: boolean;
  readonly source: {
    readonly type: string;
    readonly graph?: { readonly steps: readonly unknown[] };
  };
};

type SkillRunnerManifest = {
  readonly catalog?: Record<string, unknown>;
  readonly runners: Readonly<Record<string, SkillRunner>>;
  readonly harness?: {
    readonly files?: readonly string[];
    readonly cases: readonly {
      readonly name: string;
      readonly runner?: string;
      readonly inputs?: unknown;
      readonly env?: unknown;
      readonly caller?: unknown;
      readonly expect?: unknown;
    }[];
  };
  readonly raw: { readonly document: Record<string, unknown> };
};

describe("official skill catalog", () => {
  it("ships official skills as portable packages plus checked-in execution profiles", async () => {
    const packages = await Promise.all(officialSkillPackages().map(async (skillName) => {
      const skillDir = path.resolve("skills", skillName);
      const skillMarkdownPath = path.join(skillDir, "SKILL.md");
      const manifestPath = path.join(skillDir, "X.yaml");

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
    const manifests = validateNativeRunnerManifestYamlBatch(
      packages.map((entry) => entry.profile),
    ) as SkillRunnerManifest[];
    for (const [index, { skillName }] of packages.entries()) {
      const skill = skills[index];
      const manifest = manifests[index];
      expect(skill.name).toBe(skillName);
      expect(manifest.catalog).toBeDefined();
      expect(Object.keys(manifest.runners).length).toBeGreaterThan(0);
    }
  });

  it("keeps the public official catalog limited to implemented catalog skills", async () => {
    const publicSkills = officialSkillPackages().filter((skillName) => catalogVisibility(skillName) === "public");
    const entries = JSON.parse(
      await readFile(path.resolve("skills", "official.lock.json"), "utf8"),
    ) as ReadonlyArray<{ readonly skill_id: string; readonly catalog_visibility?: string }>;
    const publicLockSkills = entries
      .filter((entry) => entry.catalog_visibility === "public")
      .map((entry) => entry.skill_id.split("/").at(-1)!)
      .sort();

    expect(publicLockSkills).toEqual(publicSkills);
  });

  it("keeps static agent instructions exclusively in SKILL.md", () => {
    for (const manifestPath of skillManifestPaths(path.resolve("skills"))) {
      const manifest = validateRunnerManifestYaml(readFileSync(manifestPath, "utf8")).raw.document;
      expect(findObjectKeyPaths(manifest, "instructions"), manifestPath).toEqual([]);

      if (findAgentTaskNames(manifest).length === 0) {
        continue;
      }
      const skillMarkdownPath = path.join(path.dirname(manifestPath), "SKILL.md");
      expect(existsSync(skillMarkdownPath), manifestPath).toBe(true);
    }
  });

  it("keeps public packages covered by executable proof", () => {
    for (const skillName of officialSkillPackages()) {
      if (catalogVisibility(skillName) !== "public") {
        continue;
      }
      if (catalogRole(skillName) === "context") {
        continue;
      }
      const manifest = validateRunnerManifestYaml(readFileSync(path.resolve("skills", skillName, "X.yaml"), "utf8"));
      const fixtures = publicSkillFixtureCases(skillName);
      const inlineCases = manifest.harness?.cases ?? [];

      expect(fixtures.length + inlineCases.length, `${skillName} needs executable proof`).toBeGreaterThan(0);
      expect(
        fixtures.every((entry) => entry.kind === "skill" || entry.kind === "graph"),
        `${skillName} fixtures must target a skill or operator journey`,
      ).toBe(true);
      expect(
        fixtures
          .filter((entry) => entry.kind === "skill")
          .every((entry) => entry.target === ".." || entry.target?.startsWith("../graph/") === true),
        `${skillName} skill fixtures must stay within their package`,
      ).toBe(true);
      expect(
        fixtures
          .filter((entry) => entry.kind === "graph")
          .every((entry) => {
            if (entry.target?.startsWith("../harness/") === true) return true;
            // Native package admission requires explicit harness support files
            // under fixtures/. Match that declared edge as well as legacy harness/.
            const target = entry.target;
            return typeof target === "string"
              && !path.posix.isAbsolute(target)
              && !target.split("/").some((part) => part === "." || part === ".." || part === "")
              && (manifest.harness?.files ?? []).includes(`fixtures/${target}`);
          }),
        `${skillName} journey fixtures must target their package harness or declared fixture support`,
      ).toBe(true);
    }
  });
});

function officialSkillPackages(): readonly string[] {
  return readdirSync(path.resolve("skills"), { withFileTypes: true })
    .filter((entry) => entry.isDirectory())
    .filter((entry) => !entry.name.startsWith("."))
    .filter((entry) => existsSync(path.resolve("skills", entry.name, "SKILL.md")))
    .filter((entry) => existsSync(path.resolve("skills", entry.name, "X.yaml")))
    .map((entry) => entry.name)
    .sort();
}

function skillManifestPaths(root: string): readonly string[] {
  const paths: string[] = [];
  for (const entry of readdirSync(root, { withFileTypes: true })) {
    const entryPath = path.join(root, entry.name);
    if (entry.isDirectory()) {
      paths.push(...skillManifestPaths(entryPath));
    } else if (entry.name === "X.yaml") {
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

function findAgentTaskNames(value: unknown): readonly string[] {
  if (Array.isArray(value)) {
    return value.flatMap(findAgentTaskNames);
  }
  if (!value || typeof value !== "object") {
    return [];
  }
  const record = value as Record<string, unknown>;
  const current = record.type === "agent-task" && typeof record.task === "string"
    ? [record.task]
    : [];
  return [...current, ...Object.values(record).flatMap(findAgentTaskNames)];
}

function catalogVisibility(skillName: string): "public" | "internal" {
  const manifest = validateRunnerManifestYaml(readFileSync(path.resolve("skills", skillName, "X.yaml"), "utf8"));
  const catalog = manifest.catalog as { readonly visibility?: "public" | "internal" } | undefined;
  return catalog?.visibility ?? "public";
}

function catalogRole(skillName: string): string | undefined {
  const manifest = validateRunnerManifestYaml(readFileSync(path.resolve("skills", skillName, "X.yaml"), "utf8"));
  const catalog = manifest.catalog as { readonly role?: string } | undefined;
  return catalog?.role;
}

type PublicSkillFixtureCase = {
  readonly kind?: string;
  readonly target?: string;
  readonly runner?: string;
};

function publicSkillFixtureCases(skillName: string): readonly PublicSkillFixtureCase[] {
  const fixturesDir = path.resolve("skills", skillName, "fixtures");
  if (!existsSync(fixturesDir)) {
    return [];
  }
  const documents = readdirSync(fixturesDir)
    .filter((entry) => entry.endsWith(".yaml") || entry.endsWith(".yml"))
    .sort()
    .map((entry) => readFileSync(path.join(fixturesDir, entry), "utf8"));
  return validateHarnessFixtureYamlBatch(documents) as PublicSkillFixtureCase[];
}

function validateRunnerManifestYaml(profileDocument: string): SkillRunnerManifest {
  return validateNativeRunnerManifestYaml(profileDocument);
}
