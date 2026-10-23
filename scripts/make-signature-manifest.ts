import { createHash } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

// Emits the native/signatures.json manifest that package-rust-cli.ts requires.
// The binary digest binds the manifest to a specific binary; the signature entry
// records the build identity (GitHub Actions OIDC run by default). npm
// publish --provenance is the authoritative cryptographic attestation, this
// manifest is the in-package provenance breadcrumb the release contract checks.

const workspaceRoot = path.resolve(fileURLToPath(new URL("..", import.meta.url)));

interface Options {
  readonly binary: string;
  readonly worker: string;
  readonly platform: string;
  readonly out: string;
  readonly identity: string;
}

const options = parseArgs(process.argv.slice(2));
const manifest = JSON.parse(
  readFileSync(path.join(workspaceRoot, "packages", "cli", "package.json"), "utf8"),
) as { readonly name: string; readonly version: string };

const binaryPath = path.resolve(workspaceRoot, options.binary);
const workerPath = path.resolve(workspaceRoot, options.worker);
const binaryName = options.platform === "win32-x64" ? "runx.exe" : "runx";
const workerName = options.platform === "win32-x64" ? "runx-js-worker.exe" : "runx-js-worker";
const binaryDigest = sha256(binaryPath);
const workerDigest = sha256(workerPath);

const signatureManifest = {
  schema: "runx.rust_cli_artifact_signatures.v1",
  package: `${manifest.name}-${options.platform}`,
  version: manifest.version,
  platform: options.platform,
  binary: `bin/${binaryName}`,
  binary_digest: binaryDigest,
  worker: `bin/${workerName}`,
  worker_digest: workerDigest,
  signatures: [
    {
      kind: "github-actions-oidc",
      value: options.identity,
    },
  ],
};

writeFileSync(path.resolve(workspaceRoot, options.out), `${JSON.stringify(signatureManifest, null, 2)}\n`);
console.log(JSON.stringify({ status: "written", out: options.out, binary_digest: binaryDigest }, null, 2));

function sha256(filePath: string): string {
  return `sha256:${createHash("sha256").update(readFileSync(filePath)).digest("hex")}`;
}

function parseArgs(argv: readonly string[]): Options {
  let binary = "";
  let worker = "";
  let platform = "";
  let out = "";
  let identity = process.env.RUNX_SIGNATURE_IDENTITY ?? "local-unattested";

  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
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
    if (arg === "--platform") {
      platform = argv[index + 1] ?? "";
      index += 1;
      continue;
    }
    if (arg === "--out") {
      out = argv[index + 1] ?? "";
      index += 1;
      continue;
    }
    if (arg === "--identity") {
      identity = argv[index + 1] ?? "";
      index += 1;
      continue;
    }
    throw new Error(`unknown argument: ${arg}`);
  }

  if (!binary) throw new Error("--binary requires a path");
  if (!worker) throw new Error("--worker requires a path");
  if (!platform) throw new Error("--platform requires a value");
  if (!out) throw new Error("--out requires a path");
  return { binary, worker, platform, out, identity };
}
