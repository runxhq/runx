import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { readFileSync, statSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { loadRustCliPlatforms } from "./rust-cli-topology.mjs";

// Collects the actual per-target archive hashes into the single input that
// gen-channel-manifests.ts consumes. The sidecars must agree with the archive
// bytes; they are not an independent source of release facts.

const workspaceRoot = path.resolve(fileURLToPath(new URL("..", import.meta.url)));
const options = parseArgs(process.argv.slice(2));
const platforms = loadRustCliPlatforms(workspaceRoot);
const cliPackage = JSON.parse(readFileSync(path.join(workspaceRoot, "packages/cli/package.json"), "utf8"));
if (options.tag !== `cli-v${options.version}`) {
  throw new Error(`tag ${options.tag} does not match CLI version ${options.version}`);
}
if (!/^[0-9a-f]{40}$/u.test(options.commit)) {
  throw new Error("--commit must name the exact source commit");
}
if (cliPackage.version !== options.version || cliPackage.homepage !== "https://runx.ai") {
  throw new Error("release version and homepage must match the CLI package manifest");
}
if (typeof cliPackage.description !== "string" || !cliPackage.description || cliPackage.license !== "Apache-2.0") {
  throw new Error("CLI description and license must be present in the package manifest");
}

const artifacts = {};
for (const platform of platforms) {
  const file = `runx-${options.version}-${platform.rustTarget}.${platform.archiveExtension}`;
  const archivePath = path.join(options.archives, file);
  const sidecar = readFileSync(`${archivePath}.sha256`, "utf8").trim();
  const match = sidecar.match(/^([0-9a-f]{64})\s+([^\s]+)$/u);
  if (!match || match[2] !== file) {
    throw new Error(`invalid checksum sidecar for ${file}`);
  }
  const sha256 = createHash("sha256").update(readFileSync(archivePath)).digest("hex");
  if (match[1] !== sha256) {
    throw new Error(`checksum sidecar does not match archive bytes for ${file}`);
  }
  const stem = `runx-${options.version}-${platform.rustTarget}`;
  artifacts[platform.rustTarget] = {
    file,
    sha256,
    size: statSync(archivePath).size,
    binary: archivedExecutable(archivePath, `${stem}/${platform.binaryName}`),
    worker: archivedExecutable(archivePath, `${stem}/${platform.workerName}`),
  };
}

const manifest = {
  version: options.version,
  repo: options.repo,
  tag: options.tag,
  commit_ref: options.commit,
  homepage: cliPackage.homepage,
  description: cliPackage.description,
  license: cliPackage.license,
  artifacts,
};

writeFileSync(options.out, `${JSON.stringify(manifest, null, 2)}\n`);
console.log(JSON.stringify({ status: "built", out: options.out, targets: Object.keys(artifacts) }, null, 2));

function archivedExecutable(archivePath, member) {
  const bytes = archivePath.endsWith(".zip")
    ? execFileSync("python3", [
      "-c",
      "import sys,zipfile; sys.stdout.buffer.write(zipfile.ZipFile(sys.argv[1]).read(sys.argv[2]))",
      archivePath,
      member,
    ], { maxBuffer: 256 * 1024 * 1024 })
    : execFileSync("tar", ["-xOzf", archivePath, member], { maxBuffer: 256 * 1024 * 1024 });
  if (bytes.length === 0) throw new Error(`${archivePath} contains empty ${member}`);
  return { file: member, sha256: createHash("sha256").update(bytes).digest("hex"), size: bytes.length };
}

function parseArgs(argv) {
  const opts = { version: "", repo: "", tag: "", commit: "", archives: "", out: "" };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === "--version") { opts.version = argv[++i] ?? ""; continue; }
    if (arg === "--repo") { opts.repo = argv[++i] ?? ""; continue; }
    if (arg === "--tag") { opts.tag = argv[++i] ?? ""; continue; }
    if (arg === "--commit") { opts.commit = argv[++i] ?? ""; continue; }
    if (arg === "--archives") { opts.archives = argv[++i] ?? ""; continue; }
    if (arg === "--out") { opts.out = argv[++i] ?? ""; continue; }
    throw new Error(`unknown argument: ${arg}`);
  }
  for (const [k, v] of Object.entries(opts)) {
    if (!v) throw new Error(`--${k} is required`);
  }
  return opts;
}
