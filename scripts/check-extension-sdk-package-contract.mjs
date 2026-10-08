import { execFile } from "node:child_process";
import { copyFile, mkdir, mkdtemp, readFile, rm, stat, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { promisify } from "node:util";
import { fileURLToPath } from "node:url";

import { sourceCandidatesForCompiled } from "./lib/compiled-package-files.mjs";

const execFileAsync = promisify(execFile);
const workspaceRoot = path.resolve(fileURLToPath(new URL("..", import.meta.url)));
const extensionRoot = path.join(workspaceRoot, "packages", "extension-sdk");
const contractsRoot = path.join(workspaceRoot, "packages", "contracts");
const npm = process.platform === "win32" ? "npm.cmd" : "npm";
const tar = process.platform === "win32" ? "tar.exe" : "tar";
const exec = {
  timeout: 120_000,
  maxBuffer: 2 * 1024 * 1024,
  env: {
    ...process.env,
    npm_config_cache: path.join(workspaceRoot, ".runx", "cache", "npm"),
  },
};

await execFileAsync(process.execPath, ["scripts/build-workspace.mjs", "--pack"], {
  cwd: workspaceRoot,
  ...exec,
});

const tempRoot = await mkdtemp(path.join(os.tmpdir(), "runx-extension-pack-"));
const tarballs = [];

try {
  const contracts = await pack(contractsRoot);
  const extension = await pack(extensionRoot);
  tarballs.push(contracts.tarball, extension.tarball);
  requireFiles(contracts, "contracts", [
    "dist/index.js",
    "dist/index.d.ts",
    "dist/src/index.js",
    "dist/src/index.d.ts",
    "package.json",
  ]);
  requireFiles(extension, "extension-sdk", [
    "dist/index.js",
    "dist/index.d.ts",
    "dist/src/index.js",
    "dist/src/index.d.ts",
    "dist/src/tool.js",
    "package.json",
  ]);
  await assertCompiledSourcesHaveOwners(extension, extensionRoot);

  const contractsCopy = path.join(tempRoot, path.basename(contracts.tarball));
  await copyFile(contracts.tarball, contractsCopy);
  const rewrittenExtension = await rewriteWorkspaceDependency(
    extension.tarball,
    "@runxhq/contracts",
    contractsCopy,
    tempRoot,
  );
  tarballs.push(rewrittenExtension);

  const consumer = path.join(tempRoot, "consumer");
  await mkdir(consumer);
  await execFileAsync(npm, ["init", "-y"], { cwd: consumer, ...exec });
  await execFileAsync(npm, ["install", rewrittenExtension], { cwd: consumer, ...exec });
  const smoke = await execFileAsync(
    process.execPath,
    [
      "--input-type=module",
      "-e",
      [
        'import { defineTool } from "@runxhq/extension-sdk";',
        'import { Type, definePacket } from "@runxhq/contracts";',
        'const packet = definePacket({ id: "demo.echo.v1", schema: Type.Object({ value: Type.String() }, { additionalProperties: false }) });',
        'const tool = defineTool({ name: "demo.echo", run: ({ inputs }) => ({ value: inputs.value }) });',
        'const output = await tool.runWith({ value: "ok" });',
        'process.stdout.write(JSON.stringify({ packet: packet.id, output }));',
      ].join(""),
    ],
    { cwd: consumer, ...exec },
  );
  if (smoke.stdout.trim() !== '{"packet":"demo.echo.v1","output":{"value":"ok"}}') {
    throw new Error(`extension SDK consumer smoke returned: ${smoke.stdout.trim()}`);
  }
} finally {
  await Promise.all(tarballs.map((tarball) => rm(tarball, { force: true })));
  await rm(tempRoot, { recursive: true, force: true });
}

async function pack(packageRoot) {
  const report = await execFileAsync(npm, ["pack", "--json"], { cwd: packageRoot, ...exec });
  const [entry] = JSON.parse(report.stdout);
  if (!entry?.filename) throw new Error(`npm pack did not report a tarball for ${packageRoot}`);
  return {
    tarball: path.join(packageRoot, entry.filename),
    files: entry.files ?? [],
  };
}

function requireFiles(report, label, required) {
  const files = new Set(report.files.map((file) => file.path));
  for (const file of required) {
    if (!files.has(file)) throw new Error(`${label} package is missing ${file}`);
  }
}

async function assertCompiledSourcesHaveOwners(report, packageRoot) {
  for (const file of report.files.map((entry) => entry.path)) {
    if (!file.startsWith("dist/src/")) continue;
    const candidates = sourceCandidatesForCompiled(file.slice("dist/".length));
    if (!candidates) continue;
    const owned = (
      await Promise.all(candidates.map((candidate) => exists(path.join(packageRoot, candidate))))
    ).some(Boolean);
    if (!owned) {
      throw new Error(`extension-sdk package contains stale compiler output ${file}`);
    }
  }
}

async function exists(filePath) {
  try {
    await stat(filePath);
    return true;
  } catch (error) {
    if (error && typeof error === "object" && "code" in error && error.code === "ENOENT") {
      return false;
    }
    throw error;
  }
}

async function rewriteWorkspaceDependency(tarball, dependency, dependencyTarball, tempRoot) {
  const unpacked = path.join(tempRoot, "extension-unpacked");
  await mkdir(unpacked);
  await execFileAsync(tar, ["-xzf", tarball, "-C", unpacked, "--strip-components=1"], exec);
  const manifestPath = path.join(unpacked, "package.json");
  const manifest = JSON.parse(await readFile(manifestPath, "utf8"));
  if (typeof manifest.dependencies?.[dependency] !== "string") {
    throw new Error(`extension SDK must depend on ${dependency}`);
  }
  manifest.dependencies[dependency] = `file:${dependencyTarball}`;
  await writeFile(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`);
  return (await pack(unpacked)).tarball;
}
