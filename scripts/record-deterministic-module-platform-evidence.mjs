import { createHash } from "node:crypto";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { loadRustCliPlatforms, rustCliPlatformForKey } from "./rust-cli-topology.mjs";

const workspaceRoot = path.resolve(fileURLToPath(new URL("..", import.meta.url)));
const commands = [
  "cargo test --locked -p runx-js-worker",
  "cargo test --locked -p runx-runtime --test integration 'javascript_worker::' -- --nocapture",
  "cargo test --locked -p runx-runtime --test integration 'javascript_worker_hostile::' -- --nocapture",
];

const options = parseArgs(process.argv.slice(2));
const release = rustCliPlatformForKey(loadRustCliPlatforms(workspaceRoot), options.platform);
if (options.target !== release.rustTarget) {
  throw new Error(`release target mismatch for ${options.platform}: expected ${release.rustTarget}`);
}
if (options.runner !== release.runner) {
  throw new Error(`release runner mismatch for ${options.platform}: expected ${release.runner}`);
}
if (path.basename(options.worker) !== path.posix.basename(release.worker)) {
  throw new Error(`worker filename mismatch for ${options.platform}: expected ${release.worker}`);
}

const decision = JSON.parse(readFileSync(resolve(options.decision), "utf8"));
if (decision.schema !== "runx.deterministic_module_engine_decision.v1") {
  throw new Error("engine decision has an unsupported schema");
}
const workerPath = resolve(options.worker);
const outputPath = resolve(options.out);
const runUrl = process.env.GITHUB_RUN_ID && process.env.GITHUB_REPOSITORY
  ? `${process.env.GITHUB_SERVER_URL ?? "https://github.com"}/${process.env.GITHUB_REPOSITORY}/actions/runs/${process.env.GITHUB_RUN_ID}`
  : null;

const evidence = {
  schema: "runx.deterministic_module_platform_run.v1",
  status: "passed",
  platform: options.platform,
  rust_target: options.target,
  runner: options.runner,
  commit: process.env.GITHUB_SHA ?? null,
  run_url: runUrl,
  decision_digest: decision.decision_digest,
  worker: {
    path: path.basename(workerPath),
    digest: digest(readFileSync(workerPath)),
  },
  commands,
};

mkdirSync(path.dirname(outputPath), { recursive: true });
writeFileSync(outputPath, `${JSON.stringify(evidence, null, 2)}\n`);
console.log(JSON.stringify({ status: "written", platform: options.platform, out: options.out }, null, 2));

function parseArgs(argv) {
  const values = { decision: "", platform: "", target: "", runner: "", worker: "", out: "" };
  for (let index = 0; index < argv.length; index += 1) {
    const name = argv[index];
    if (!name.startsWith("--")) {
      throw new Error(`unexpected argument: ${name}`);
    }
    const key = name.slice(2);
    if (!Object.hasOwn(values, key)) {
      throw new Error(`unknown argument: ${name}`);
    }
    values[key] = argv[index + 1] ?? "";
    index += 1;
  }
  for (const [name, value] of Object.entries(values)) {
    if (!value) {
      throw new Error(`--${name} is required`);
    }
  }
  return values;
}

function resolve(filePath) {
  return path.resolve(workspaceRoot, filePath);
}

function digest(bytes) {
  return `sha256:${createHash("sha256").update(bytes).digest("hex")}`;
}
