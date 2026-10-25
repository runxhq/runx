import { spawnSync } from "node:child_process";
import { existsSync, readFileSync, readdirSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const workspaceRoot = path.resolve(fileURLToPath(new URL("..", import.meta.url)));
const npm = process.platform === "win32" ? "npm.cmd" : "npm";
const pnpm = process.platform === "win32" ? "pnpm.cmd" : "pnpm";

interface Options {
  readonly artifactDir: string;
  readonly binary: string;
  readonly worker: string;
  readonly dryRun: boolean;
  readonly platform: string | null;
  readonly publish: boolean;
  readonly signatureManifest: string | null;
  readonly tag: string;
}

const options = parseArgs(process.argv.slice(2));

if (!options.signatureManifest) {
  throw new Error("--signature-manifest is required so release artifacts can pass signature verification");
}

run(pnpm, [
  "exec",
  "tsx",
  "scripts/package-rust-cli.ts",
  "--binary",
  options.binary,
  "--worker",
  options.worker,
  "--out-dir",
  options.artifactDir,
  ...(options.platform ? ["--platform", options.platform] : []),
  "--signature-manifest",
  options.signatureManifest,
]);
run(pnpm, [
  "exec",
  "tsx",
  "scripts/check-rust-cli-release-artifacts.ts",
  "--artifact-dir",
  options.artifactDir,
  "--no-js-delegation",
  "--verify-signatures",
]);

if (!options.publish) {
  console.log(JSON.stringify({
    status: "prepared",
    artifact_dir: options.artifactDir,
    dry_run: options.dryRun,
    publish: false,
  }, null, 2));
  process.exit(0);
}

const publishTargets = packageDirs(path.resolve(workspaceRoot, options.artifactDir));
assertPublishTargets(publishTargets);
for (const packageDir of publishTargets) {
  if (options.dryRun) {
    run(npm, ["pack", "--dry-run", "--json"], { cwd: packageDir });
  } else {
    run(npm, ["publish", "--access", "public", "--tag", options.tag], { cwd: packageDir });
  }
}

console.log(JSON.stringify({
  status: options.dryRun ? "dry_run_validated" : "published",
  artifact_dir: options.artifactDir,
  tag: options.tag,
}, null, 2));

function parseArgs(argv: readonly string[]): Options {
  let artifactDir = ".runx/rust-cli-artifacts";
  let binary = "target/debug/runx";
  let worker = "target/debug/runx-js-worker";
  let dryRun = true;
  let platform: string | null = null;
  let publish = false;
  let signatureManifest: string | null = null;
  let tag = "next";

  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--artifact-dir") {
      artifactDir = argv[index + 1] ?? "";
      index += 1;
      continue;
    }
    if (arg === "--publish") {
      publish = true;
      continue;
    }
    if (arg === "--binary") {
      binary = argv[index + 1] ?? "";
      index += 1;
      continue;
    }
    if (arg === "--worker") {
      worker = argv[index + 1] ?? "";
      index += 1;
      continue;
    }
    if (arg === "--signature-manifest") {
      signatureManifest = argv[index + 1] ?? "";
      index += 1;
      continue;
    }
    if (arg === "--platform") {
      platform = argv[index + 1] ?? "";
      index += 1;
      continue;
    }
    if (arg === "--no-dry-run") {
      dryRun = false;
      continue;
    }
    if (arg === "--tag") {
      tag = argv[index + 1] ?? "";
      index += 1;
      continue;
    }
    if (arg === "--help" || arg === "-h") {
      printUsage();
      process.exit(0);
    }
    throw new Error(`unknown argument: ${arg}`);
  }

  if (!artifactDir) {
    throw new Error("--artifact-dir requires a path");
  }
  if (!binary) {
    throw new Error("--binary requires a path");
  }
  if (!worker) {
    throw new Error("--worker requires a path");
  }
  if (signatureManifest === "") {
    throw new Error("--signature-manifest requires a path");
  }
  if (platform === "") {
    throw new Error("--platform requires a value");
  }
  if (!publish && !dryRun) {
    throw new Error("--no-dry-run requires --publish");
  }
  if (!tag) {
    throw new Error("--tag requires a value");
  }
  return { artifactDir, binary, worker, dryRun, platform, publish, signatureManifest, tag };
}

function packageDirs(root: string): readonly string[] {
  const rootManifest = path.join(root, "package.json");
  if (existsSync(rootManifest)) {
    return [root];
  }
  const dirs = readdirSync(root, { withFileTypes: true })
    .filter((entry) => entry.isDirectory() && existsSync(path.join(root, entry.name, "package.json")))
    .map((entry) => path.join(root, entry.name))
    .sort();
  if (dirs.length === 0) {
    throw new Error(`release artifact directory contains no package.json files: ${root}`);
  }
  return dirs;
}

function assertPublishTargets(packageDirs: readonly string[]): void {
  const seen = new Set<string>();
  for (const packageDir of packageDirs) {
    const manifest = JSON.parse(readFileSync(path.join(packageDir, "package.json"), "utf8")) as {
      readonly name?: string;
      readonly version?: string;
    };
    const key = `${manifest.name ?? "<missing-name>"}@${manifest.version ?? "<missing-version>"}`;
    if (seen.has(key)) {
      throw new Error(`duplicate npm publish target: ${key}`);
    }
    seen.add(key);
  }
}

function run(command: string, args: readonly string[], options: { readonly cwd?: string } = {}): void {
  const result = spawnSync(command, args, {
    cwd: options.cwd ?? workspaceRoot,
    stdio: "inherit",
    env: process.env,
    // Windows package-manager shims are .cmd files; spawnSync needs a shell to
    // execute them reliably. Arguments here are fixed release-script literals.
    shell: process.platform === "win32",
  });
  if (result.error) {
    throw result.error;
  }
  if (result.status !== 0) {
    throw new Error(`${command} ${args.join(" ")} exited with ${result.status}`);
  }
}

function printUsage(): void {
  console.log("Usage: pnpm exec tsx scripts/release-rust-cli.ts [--artifact-dir .runx/rust-cli-artifacts] [--binary target/debug/runx] [--worker target/debug/runx-js-worker] [--platform darwin-arm64|darwin-x64|linux-arm64|linux-x64|win32-x64] --signature-manifest native/signatures.json [--publish] [--no-dry-run] [--tag next]");
}
