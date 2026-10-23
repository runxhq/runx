import { mkdir, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { createHash } from "node:crypto";
import path from "node:path";
import { fileURLToPath } from "node:url";

import {
  validateRunnerManifestYamlBatch,
  validateSkillMarkdownBatch,
} from "./lib/native-parser.mjs";

const workspaceRoot = path.resolve(fileURLToPath(new URL("..", import.meta.url)));
const fixtureRoot = path.join(workspaceRoot, "fixtures", "runtime", "skills");
const check = process.argv.includes("--check");
const generatedAt = "2026-05-18T00:00:00Z";
const skillNames = ["code-intake"] as const;

interface ParsedRunnerManifest {
  readonly skill: string;
  readonly runners: Readonly<Record<string, {
    readonly name: string;
    readonly default: boolean;
  }>>;
  readonly harness?: {
    readonly cases: readonly Record<string, unknown>[];
  };
}

process.chdir(workspaceRoot);

const packages = await Promise.all(skillNames.map(async (skillName) => {
  const skillDir = path.join(workspaceRoot, "skills", skillName);
  return {
    skillName,
    skillMarkdown: await readFile(path.join(skillDir, "SKILL.md"), "utf8"),
    profileSource: await readFile(path.join(skillDir, "run.yaml"), "utf8"),
  };
}));
const skills = validateSkillMarkdownBatch(packages.map((entry) => entry.skillMarkdown));
const profiles = validateRunnerManifestYamlBatch(
  packages.map((entry) => entry.profileSource),
) as ParsedRunnerManifest[];
for (const [index, entry] of packages.entries()) {
  const profile = profiles[index];
  if (!profile) throw new Error(`native parser omitted skills/${entry.skillName}/run.yaml`);
  await generateSkillFixtures(
    entry.skillName,
    entry.skillMarkdown,
    entry.profileSource,
    profile,
    (skills[index] as { name: string }).name,
  );
}

console.log(`${check ? "checked" : "generated"} Rust product skill fixtures`);

async function generateSkillFixtures(
  skillName: typeof skillNames[number],
  skillMarkdown: string,
  profileSource: string,
  profile: ParsedRunnerManifest,
  declaredSkillName: string,
): Promise<void> {
  const profilePath = path.join(workspaceRoot, "skills", skillName, "run.yaml");
  if (declaredSkillName !== skillName || profile.skill !== skillName) {
    throw new Error(`${skillName}: product skill name drifted from SKILL.md/run.yaml`);
  }
  const runner = defaultRunner(profile, profilePath);
  const cases = harnessCases(profile, profilePath);
  const targetDir = path.join(fixtureRoot, skillName);
  if (!check) {
    await rm(targetDir, { recursive: true, force: true });
  }
  await mkdir(path.join(targetDir, "cases"), { recursive: true });

  await writeOrCheck(path.join(targetDir, "metadata.json"), `${JSON.stringify({
    schema: "runx.runtime.skill_fixture.v1",
    generated_at: generatedAt,
    source: {
      skill: path.posix.join("skills", skillName, "SKILL.md"),
      profile: path.posix.join("skills", skillName, "run.yaml"),
    },
    skill_name: skillName,
    manifest_hash: `sha256:${sha256(`${skillMarkdown}\n${profileSource}`)}`,
    harness_schema: "runx.receipt.v1",
    case_names: cases.map((entry) => String(entry.name)),
  }, null, 2)}\n`);

  for (const entry of cases) {
    const fixture = intakeFixture(entry, skillName, runner);
    await writeOrCheck(
      path.join(targetDir, "cases", `${entry.name}.yaml`),
      yaml(fixture),
    );
  }

  if (check) {
    await assertNoStaleCases(targetDir, cases);
  }
}

function intakeFixture(
  entry: Record<string, unknown>,
  skillName: string,
  runner: string,
): Record<string, unknown> {
  return {
    name: entry.name,
    kind: "skill",
    target: path.posix.join("../../../../../skills", skillName),
    runner,
    inputs: entry.inputs ?? {},
    caller: entry.caller ?? {},
    expect: canonicalExpectation(entry, {
      status: "sealed",
      state: "sealed",
      outcome: "completed",
    }),
    metadata: {
      product_skill: skillName,
      source_case: entry.name,
    },
  };
}

function defaultRunner(profile: ParsedRunnerManifest, sourcePath: string): string {
  const runners = Object.values(profile.runners);
  const defaults = runners.filter((runner) => runner.default);
  const runner = defaults.length === 1 ? defaults[0] : runners.length === 1 ? runners[0] : undefined;
  if (!runner) {
    throw new Error(`${sourcePath}: fixture generation requires one parser-selected default runner`);
  }
  return runner.name;
}

function canonicalExpectation(
  entry: Record<string, unknown>,
  receipt: {
    status: string;
    state: string;
    outcome: string;
    childReceiptCount?: number;
  },
): Record<string, unknown> {
  const status = record(entry.expect, "expect")?.status ?? receipt.status;
  const receiptExpectation: Record<string, unknown> = {
    schema: "runx.receipt.v1",
    state: receipt.state,
    outcome: receipt.outcome,
  };
  if (receipt.childReceiptCount !== undefined) {
    receiptExpectation.child_receipt_count = receipt.childReceiptCount;
  }
  return {
    status,
    receipt: receiptExpectation,
  };
}

function harnessCases(profile: ParsedRunnerManifest, sourcePath: string): Record<string, unknown>[] {
  const cases = profile.harness?.cases;
  if (!cases) {
    throw new Error(`${sourcePath}: harness.cases must be an array`);
  }
  return cases.map((entry, index) => {
    const value = record(entry, `${sourcePath}.harness.cases[${index}]`);
    if (!value || typeof value.name !== "string" || value.name.length === 0) {
      throw new Error(`${sourcePath}: harness.cases[${index}].name is required`);
    }
    return value;
  });
}

async function assertNoStaleCases(
  targetDir: string,
  cases: Record<string, unknown>[],
): Promise<void> {
  const expected = new Set(cases.map((entry) => `${entry.name}.yaml`));
  const casesDir = path.join(targetDir, "cases");
  let entries: string[];
  try {
    entries = await readdir(casesDir);
  } catch {
    entries = [];
  }
  const stale = entries.filter((entry) => !expected.has(entry));
  if (stale.length > 0) {
    throw new Error(`${casesDir}: stale generated fixture(s): ${stale.join(", ")}`);
  }
}

async function writeOrCheck(filePath: string, contents: string): Promise<void> {
  if (check) {
    const current = await readFile(filePath, "utf8").catch(() => undefined);
    if (current !== contents) {
      throw new Error(`${path.relative(workspaceRoot, filePath)} is stale; run pnpm tsx scripts/generate-rust-skill-fixtures.ts`);
    }
    return;
  }
  await mkdir(path.dirname(filePath), { recursive: true });
  await writeFile(filePath, contents);
}

function yaml(value: unknown, indent = 0): string {
  if (Array.isArray(value)) {
    if (value.length === 0) {
      return `${" ".repeat(indent)}[]\n`;
    }
    return value.map((entry) => isScalar(entry)
      ? `${" ".repeat(indent)}- ${scalar(entry)}\n`
      : `${" ".repeat(indent)}-\n${yaml(entry, indent + 2)}`).join("");
  }
  const object = record(value, "yaml") ?? {};
  if (Object.keys(object).length === 0) {
    return `${" ".repeat(indent)}{}\n`;
  }
  return Object.entries(object).map(([key, entry]) => {
    if (entry === undefined) {
      return "";
    }
    if (isScalar(entry)) {
      return `${" ".repeat(indent)}${key}: ${scalar(entry)}\n`;
    }
    return `${" ".repeat(indent)}${key}:\n${yaml(entry, indent + 2)}`;
  }).join("");
}

function scalar(value: unknown): string {
  if (value === null) {
    return "null";
  }
  if (typeof value === "number" || typeof value === "boolean") {
    return String(value);
  }
  const stringValue = String(value);
  return JSON.stringify(stringValue);
}

function isScalar(value: unknown): boolean {
  return value === null || ["string", "number", "boolean"].includes(typeof value);
}

function record(value: unknown, _field: string): Record<string, unknown> | undefined {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    return undefined;
  }
  return value as Record<string, unknown>;
}

function sha256(value: string): string {
  return createHash("sha256").update(value).digest("hex");
}
