import { existsSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { mkdir, readFile, readdir, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const workspaceRoot = path.resolve(fileURLToPath(new URL("..", import.meta.url)));
const fixtureRoot = path.join(workspaceRoot, "fixtures", "doctor");
const check = process.argv.includes("--check");
const runx = process.env.RUNX_RUST_CLI_BIN
  ?? path.join(workspaceRoot, "crates", "target", "debug", process.platform === "win32" ? "runx.exe" : "runx");
const managedAgentEnvNames = [
  "RUNX_AGENT_PROVIDER",
  "RUNX_AGENT_MODEL",
  "RUNX_AGENT_API_KEY",
  "ANTHROPIC_API_KEY",
  "OPENAI_API_KEY",
] as const;

interface DoctorFixtureCase {
  readonly name: string;
  readonly expectedExitCode: number;
  readonly files: readonly FixtureFile[];
}

interface FixtureFile {
  readonly path: string;
  readonly contents: string;
}

const cases: readonly DoctorFixtureCase[] = [
  {
    name: "empty-success",
    expectedExitCode: 0,
    files: [],
  },
  {
    name: "tool-fixture-missing",
    expectedExitCode: 1,
    files: [
      file("tools/demo/echo/manifest.json", `${JSON.stringify({
        schema: "runx.tool.manifest.v1",
        name: "demo.echo",
        description: "Echo fixture.",
        source: {
          command: "node",
          args: ["./run.mjs"],
        },
        inputs: {},
        scopes: [],
      }, null, 2)}\n`),
    ],
  },
  {
    name: "skill-fixture-missing",
    expectedExitCode: 1,
    files: [
      file("skills/uncovered/SKILL.md", `---
name: uncovered
description: Doctor fixture with an execution profile and no harness proof.
---

# Uncovered

Exercise the missing-harness diagnostic for a valid skill package.
`),
      file("skills/uncovered/run.yaml", `skill: uncovered
runners:
  default:
    default: true
    steps:
      - id: default
        tool: { command: node, args: [-e, "process.stdout.write(JSON.stringify({ ok: true }))"] }
        outputs:
          ok: boolean
`),
    ],
  },
  {
    name: "skill-frontmatter-invalid",
    expectedExitCode: 1,
    files: [
      file("skills/catalog-in-manual/SKILL.md", `---
name: catalog-in-manual
description: Doctor fixture whose frontmatter carries catalog metadata.
runx:
  category: content
---

# Catalog in manual

Catalog metadata belongs in run.yaml, so Agent Skills hosts reject this manual.
`),
      file("skills/renamed-directory/SKILL.md", `---
name: original-name
description: Doctor fixture whose name no longer matches its directory.
---

# Original name

Hosts discover this skill by its directory, so the name must match it.
`),
    ],
  },
  {
    name: "cross-package-reach-in",
    expectedExitCode: 1,
    files: [
      file("packages/sample/src/index.ts", `import "../../core/src/index.js";\n`),
      file("packages/core/src/index.ts", "export const core = true;\n"),
    ],
  },
];

const expectedFiles = new Set<string>();

for (const fixtureCase of cases) {
  await writeWorkspace(fixtureCase);
  const report = await runDoctorFixture(fixtureCase);
  await writeOrCheck(
    path.join(fixtureRoot, fixtureCase.name, "expected.json"),
    `${JSON.stringify(report, null, 2)}\n`,
  );
}

if (check) {
  await checkNoStaleFiles();
}

console.log(`${check ? "checked" : "generated"} ${cases.length} doctor fixtures`);

function file(filePath: string, contents: string): FixtureFile {
  return { path: filePath, contents };
}

async function writeWorkspace(fixtureCase: DoctorFixtureCase): Promise<void> {
  for (const fixtureFile of fixtureCase.files) {
    await writeOrCheck(
      path.join(fixtureRoot, fixtureCase.name, "workspace", fixtureFile.path),
      fixtureFile.contents,
    );
  }
}

async function runDoctorFixture(fixtureCase: DoctorFixtureCase): Promise<unknown> {
  const workspacePath = path.join(fixtureRoot, fixtureCase.name, "workspace");
  if (!existsSync(workspacePath)) {
    await mkdir(workspacePath, { recursive: true });
  }
  const env = {
    ...process.env,
    RUNX_CWD: workspacePath,
    RUNX_HOME: path.join(workspaceRoot, "tmp", "doctor-fixtures", fixtureCase.name, "runx-home"),
  };
  for (const envName of managedAgentEnvNames) {
    delete env[envName];
  }
  const result = spawnSync(runx, ["doctor", "--json"], {
    cwd: workspacePath,
    env,
    encoding: "utf8",
    maxBuffer: 16 * 1024 * 1024,
  });
  if (result.error) throw result.error;
  if (result.status !== fixtureCase.expectedExitCode) {
    throw new Error(
      `${fixtureCase.name}: expected exit ${fixtureCase.expectedExitCode}, got ${result.status}`,
    );
  }
  if (result.stderr !== "") {
    throw new Error(`${fixtureCase.name}: expected empty stderr, got ${JSON.stringify(result.stderr)}`);
  }
  return JSON.parse(result.stdout);
}

async function writeOrCheck(filePath: string, contents: string): Promise<void> {
  expectedFiles.add(filePath);
  if (check) {
    const existing = await readFile(filePath, "utf8");
    if (existing !== contents) {
      throw new Error(`fixture is stale: ${path.relative(workspaceRoot, filePath)}`);
    }
    return;
  }
  await mkdir(path.dirname(filePath), { recursive: true });
  await writeFile(filePath, contents);
}

async function checkNoStaleFiles(): Promise<void> {
  if (!existsSync(fixtureRoot)) {
    throw new Error("doctor fixture root is missing");
  }
  for (const filePath of await collectFiles(fixtureRoot)) {
    if (!expectedFiles.has(filePath)) {
      throw new Error(`stale fixture file: ${path.relative(workspaceRoot, filePath)}`);
    }
  }
}

async function collectFiles(directory: string): Promise<readonly string[]> {
  const entries = await readdir(directory, { withFileTypes: true });
  const files: string[] = [];
  for (const entry of entries) {
    const entryPath = path.join(directory, entry.name);
    if (entry.isDirectory()) {
      files.push(...await collectFiles(entryPath));
    } else if (entry.isFile()) {
      files.push(entryPath);
    }
  }
  return files.sort();
}
