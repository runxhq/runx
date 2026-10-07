import { spawnSync } from "node:child_process";
import { cp, mkdir, mkdtemp, readFile, readdir, rm, stat, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const workspaceRoot = path.resolve(fileURLToPath(new URL("..", import.meta.url)));
const fixtureRoot = path.join(workspaceRoot, "fixtures", "tool-catalogs");
const oracleRoot = path.join(fixtureRoot, "oracles");
const check = process.argv.includes("--check");
const runx = process.env.RUNX_RUST_CLI_BIN
  ?? path.join(workspaceRoot, "crates", "target", "debug", process.platform === "win32" ? "runx.exe" : "runx");

process.chdir(workspaceRoot);

interface SearchCase {
  readonly name: string;
  readonly query: string;
  readonly source: string;
  readonly expectedStatus: number;
}

interface InspectCase {
  readonly name: string;
  readonly ref: string;
  readonly source?: string;
  readonly toolRoot?: string;
  readonly expectedStatus: number;
}

interface OracleCase {
  readonly name: string;
  readonly argv: readonly string[];
  readonly cwd: string;
  readonly env?: Readonly<Record<string, string>>;
  readonly expectedStatus: number;
}

const tempRoot = await mkdtemp(path.join(os.tmpdir(), "runx-tool-catalog-oracles-"));
const expectedFiles = new Set<string>();

try {
  const cases = [
    ...(await buildCases(tempRoot)),
    ...(await searchCases()),
    ...(await inspectCases()),
  ];

  for (const oracleCase of cases) {
    await runOracleCase(oracleCase);
  }

  if (check) {
    await checkNoStaleFiles();
  }

  console.log(`${check ? "checked" : "generated"} ${cases.length} tool catalog oracle cases`);
} finally {
  await rm(tempRoot, { recursive: true, force: true });
}

async function buildCases(root: string): Promise<readonly OracleCase[]> {
  const cases = [
    ["build-minimal", "minimal", "tools/fixture/minimal", 0],
    ["build-multi-command", "multi-command", "tools/fixture/multi_command", 0],
    ["build-metadata-heavy", "metadata-heavy", "tools/fixture/metadata_heavy", 0],
    ["build-invalid", "invalid", "tools/fixture/invalid", 1],
  ] as const;

  const generated: OracleCase[] = [];
  for (const [name, fixtureName, toolPath, expectedStatus] of cases) {
    const sourceWorkspace = path.join(fixtureRoot, "build", fixtureName, "workspace");
    const workspace = path.join(root, name, "workspace");
    await cp(sourceWorkspace, workspace, { recursive: true });
    generated.push({
      name,
      argv: ["tool", "build", toolPath, "--json"],
      cwd: workspace,
      expectedStatus,
    });
  }
  return generated;
}

async function searchCases(): Promise<readonly OracleCase[]> {
  const cases = await readJson<readonly SearchCase[]>(path.join(fixtureRoot, "search", "cases.json"));
  return cases.map((fixtureCase) => ({
    name: fixtureCase.name,
    argv: ["tool", "search", fixtureCase.query, "--source", fixtureCase.source, "--json"],
    cwd: workspaceRoot,
    expectedStatus: fixtureCase.expectedStatus,
  }));
}

async function inspectCases(): Promise<readonly OracleCase[]> {
  const cases = await readJson<readonly InspectCase[]>(path.join(fixtureRoot, "inspect", "cases.json"));
  return cases.map((fixtureCase) => {
    const argv = ["tool", "inspect", fixtureCase.ref, "--json"];
    if (fixtureCase.source) {
      argv.push("--source", fixtureCase.source);
    }
    const env: Record<string, string> = {};
    if (fixtureCase.toolRoot) {
      env.RUNX_TOOL_ROOTS = path.join(fixtureRoot, "inspect", fixtureCase.toolRoot);
    }
    return {
      name: fixtureCase.name,
      argv,
      cwd: workspaceRoot,
      env,
      expectedStatus: fixtureCase.expectedStatus,
    };
  });
}

async function runOracleCase(oracleCase: OracleCase): Promise<void> {
  const env = deterministicEnv(oracleCase.cwd, path.join(tempRoot, oracleCase.name), oracleCase.env);
  const result = spawnSync(runx, oracleCase.argv, {
    cwd: oracleCase.cwd,
    env,
    encoding: "utf8",
    maxBuffer: 16 * 1024 * 1024,
  });
  if (result.error) throw result.error;
  if (result.status !== oracleCase.expectedStatus) {
    throw new Error(`${oracleCase.name}: expected status ${oracleCase.expectedStatus}, got ${result.status}`);
  }

  const normalizedStdout = normalizeOutput(result.stdout);
  const normalizedStderr = normalizeOutput(result.stderr);
  await writeOrCheck(oraclePath(oracleCase.name, "stdout"), normalizedStdout);
  await writeOrCheck(oraclePath(oracleCase.name, "stderr"), normalizedStderr);
  await writeOrCheck(oraclePath(oracleCase.name, "status"), `${result.status}\n`);

  const parsed = parseJson(normalizedStdout);
  if (parsed !== undefined) {
    await writeOrCheck(oraclePath(oracleCase.name, "json"), `${JSON.stringify(parsed, null, 2)}\n`);
  }

}

function deterministicEnv(
  cwd: string,
  caseTempRoot: string,
  overrides: Readonly<Record<string, string>> | undefined,
): NodeJS.ProcessEnv {
  const env: NodeJS.ProcessEnv = {
    ...process.env,
    CI: "1",
    FORCE_COLOR: "0",
    LANG: "C",
    LC_ALL: "C",
    NO_COLOR: "1",
    RUNX_CWD: cwd,
    RUNX_HOME: path.join(caseTempRoot, "home"),
    RUNX_KNOWLEDGE_DIR: path.join(caseTempRoot, "knowledge"),
    RUNX_OFFICIAL_SKILLS_DIR: path.join(caseTempRoot, "official-skills"),
    RUNX_PROJECT_DIR: path.join(caseTempRoot, "project"),
    RUNX_REGISTRY_DIR: path.join(caseTempRoot, "registry"),
    RUNX_REGISTRY_URL: "",
    TZ: "UTC",
    ...overrides,
  };
  return env;
}

function normalizeOutput(value: string): string {
  return value
    .split(workspaceRoot).join("<repo>")
    .split(tempRoot).join("<temp>")
    .replaceAll("\\", "/");
}

function oraclePath(name: string, extension: string): string {
  return path.join(oracleRoot, `${name}.${extension}`);
}

async function writeOrCheck(filePath: string, contents: string): Promise<void> {
  expectedFiles.add(filePath);
  if (check) {
    const existing = await readFile(filePath, "utf8");
    if (existing !== contents) {
      throw new Error(`stale oracle file: ${path.relative(workspaceRoot, filePath)}`);
    }
    return;
  }
  await mkdir(path.dirname(filePath), { recursive: true });
  await writeFile(filePath, contents);
}

async function checkNoStaleFiles(): Promise<void> {
  for (const filePath of await collectFiles(oracleRoot)) {
    if (!expectedFiles.has(filePath)) {
      throw new Error(`stale oracle file: ${path.relative(workspaceRoot, filePath)}`);
    }
  }
}

async function collectFiles(directory: string): Promise<readonly string[]> {
  try {
    const directoryStat = await stat(directory);
    if (!directoryStat.isDirectory()) {
      return [];
    }
  } catch {
    return [];
  }

  const files: string[] = [];
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const entryPath = path.join(directory, entry.name);
    if (entry.isDirectory()) {
      files.push(...await collectFiles(entryPath));
    } else if (entry.isFile()) {
      files.push(entryPath);
    }
  }
  return files.sort();
}

async function readJson<T>(filePath: string): Promise<T> {
  return JSON.parse(await readFile(filePath, "utf8")) as T;
}

function parseJson(value: string): unknown | undefined {
  if (value.trim().length === 0) {
    return undefined;
  }
  try {
    return JSON.parse(value);
  } catch {
    return undefined;
  }
}
