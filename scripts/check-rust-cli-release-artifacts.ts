import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, readFileSync, readdirSync, statSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import {
  isRustCliSignatureEntry,
  loadRustCliPlatforms,
  nativeRustCliPackageName,
} from "./rust-cli-topology.mjs";

type RustCliPlatformSpec = ReturnType<typeof loadRustCliPlatforms>[number];

const workspaceRoot = path.resolve(fileURLToPath(new URL("..", import.meta.url)));
const npm = process.platform === "win32" ? "npm.cmd" : "npm";

interface Finding {
  readonly rule: string;
  readonly file: string;
  readonly message: string;
}

interface Options {
  readonly artifactDir: string;
  readonly noJsDelegation: boolean;
  readonly verifySignatures: boolean;
}

const selectorPackageName = "@runxhq/cli";
const supportedPlatforms = loadRustCliPlatforms(workspaceRoot);

const options = parseArgs(process.argv.slice(2));
const artifactDir = path.resolve(workspaceRoot, options.artifactDir);
const findings: Finding[] = [];

if (!existsSync(artifactDir)) {
  findings.push(finding("artifact_dir_missing", artifactDir, "release artifact directory does not exist"));
} else {
  const packageDirs = listPackageDirs(artifactDir);
  if (packageDirs.length === 0) {
    findings.push(finding("artifact_package_missing", artifactDir, "release artifact directory contains no package.json files"));
  }
  inspectPublishTargets(packageDirs, findings);
  for (const packageDir of packageDirs) {
    inspectPackageDir(packageDir, findings);
  }
}

emit({
  status: findings.length === 0 ? "passed" : "blocked",
  artifact_dir: displayPath(artifactDir),
  findings,
});
process.exit(findings.length === 0 ? 0 : 1);

function parseArgs(argv: readonly string[]): Options {
  let artifactDir = ".runx/rust-cli-artifacts";
  let noJsDelegation = false;
  let verifySignatures = false;

  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--artifact-dir") {
      artifactDir = argv[index + 1] ?? "";
      index += 1;
      continue;
    }
    if (arg === "--no-js-delegation") {
      noJsDelegation = true;
      continue;
    }
    if (arg === "--verify-signatures") {
      verifySignatures = true;
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
  return { artifactDir, noJsDelegation, verifySignatures };
}

function listPackageDirs(root: string): readonly string[] {
  if (existsSync(path.join(root, "package.json"))) {
    return [root];
  }
  const entries = readdirSync(root, { withFileTypes: true })
    .filter((entry) => entry.isDirectory())
    .filter((entry) => existsSync(path.join(root, entry.name, "package.json")))
    .map((entry) => path.join(root, entry.name))
    .sort();
  return entries;
}

function inspectPackageDir(packageDir: string, output: Finding[]): void {
  const manifestPath = path.join(packageDir, "package.json");
  if (!existsSync(manifestPath)) {
    output.push(finding("package_manifest_missing", packageDir, "artifact package is missing package.json"));
    return;
  }

  const manifest = readJson<{
    readonly name?: string;
    readonly version?: string;
    readonly bin?: string | { readonly runx?: string };
    readonly files?: readonly string[];
    readonly os?: readonly string[];
    readonly cpu?: readonly string[];
    readonly runx?: {
      readonly nativeSelector?: {
        readonly schema?: string;
        readonly supportedPlatforms?: readonly string[];
        readonly nativePackagePattern?: string;
      };
      readonly nativePackage?: {
        readonly schema?: string;
        readonly selectorPackage?: string;
        readonly platform?: string;
        readonly binary?: string;
        readonly worker?: string;
      };
    };
    readonly dependencies?: Record<string, string>;
    readonly optionalDependencies?: Record<string, string>;
    readonly devDependencies?: Record<string, string>;
    readonly peerDependencies?: Record<string, string>;
    readonly main?: unknown;
    readonly types?: unknown;
    readonly exports?: unknown;
    readonly scripts?: unknown;
  }>(manifestPath, output, "package_manifest_malformed");
  if (!manifest) {
    return;
  }
  const bin = typeof manifest.bin === "string" ? manifest.bin : manifest.bin?.runx;

  inspectForbiddenManifestEntrypoints(manifest, manifestPath, output);
  if (isSelectorPackage(manifest)) {
    if (!bin) {
      output.push(finding("selector_bin_missing", manifestPath, "selector package.json must declare bin.runx"));
      return;
    }
    inspectSelectorPackage(packageDir, manifestPath, manifest, bin, output);
    return;
  }

  const spec = nativeSpecForPackage(manifest.name);
  if (!spec) {
    inspectNativePackageManifest(manifestPath, manifest, output);
    return;
  }
  if (bin) {
    if (/\.(?:js|mjs|cjs)$/u.test(bin)) {
      output.push(finding("package_bin_js", manifestPath, `bin.runx points to JavaScript: ${bin}`));
    }
  }
  inspectNativePackageManifest(manifestPath, manifest, output);
  const binaryPath = path.resolve(packageDir, spec.binary);
  if (!isInside(binaryPath, packageDir)) {
    output.push(finding("package_binary_escapes", manifestPath, `native binary points outside the package: ${spec.binary}`));
    return;
  }
  if (!existsSync(binaryPath)) {
    output.push(finding("native_binary_missing", binaryPath, "native binary target is missing"));
  } else {
    const entry = statSync(binaryPath);
    if (!entry.isFile() || (process.platform !== "win32" && (entry.mode & 0o111) === 0)) {
      output.push(finding("native_binary_not_executable", binaryPath, "native binary target is not executable"));
    }
  }
  const workerPath = path.resolve(packageDir, spec.worker);
  if (!isInside(workerPath, packageDir)) {
    output.push(finding("worker_binary_escapes", manifestPath, `JavaScript worker points outside the package: ${spec.worker}`));
    return;
  }
  if (!existsSync(workerPath)) {
    output.push(finding("worker_binary_missing", workerPath, "JavaScript worker target is missing"));
  } else {
    const entry = statSync(workerPath);
    if (!entry.isFile() || (process.platform !== "win32" && (entry.mode & 0o111) === 0)) {
      output.push(finding("worker_binary_not_executable", workerPath, "JavaScript worker target is not executable"));
    }
  }

  inspectDependencySections(manifest, manifestPath, output);
  inspectChecksum(packageDir, spec.binary, spec.worker, output);
  inspectSignature(packageDir, spec.binary, spec.worker, output);
  const packedFiles = inspectPackList(packageDir, output);

  if (options.noJsDelegation) {
    // The native binary is the compiled program, not a JS launcher: its rodata
    // legitimately contains source-path strings (e.g. diagnostic examples), so
    // exclude it from the delegation-token text scan. Integrity is covered by
    // the checksum and signature manifests instead.
    inspectTextFiles(packageDir, packedFiles, output, spec.binary);
  }
}

function inspectSelectorPackage(
  packageDir: string,
  manifestPath: string,
  manifest: {
    readonly name?: string;
    readonly version?: string;
    readonly bin?: string | { readonly runx?: string };
    readonly files?: readonly string[];
    readonly runx?: {
      readonly nativeSelector?: {
        readonly schema?: string;
        readonly supportedPlatforms?: readonly string[];
        readonly nativePackagePattern?: string;
      };
    };
    readonly dependencies?: Record<string, string>;
    readonly optionalDependencies?: Record<string, string>;
    readonly devDependencies?: Record<string, string>;
    readonly peerDependencies?: Record<string, string>;
  },
  bin: string,
  output: Finding[],
): void {
  if (manifest.name !== selectorPackageName) {
    output.push(finding("selector_package_name_invalid", manifestPath, `selector package must be ${selectorPackageName}`));
  }
  if (bin !== "./bin/runx") {
    output.push(finding("selector_bin_invalid", manifestPath, `selector bin.runx must be ./bin/runx, found ${bin}`));
  }
  const binaryPath = path.resolve(packageDir, bin);
  if (!isInside(binaryPath, packageDir)) {
    output.push(finding("package_bin_escapes", manifestPath, `bin.runx points outside the package: ${bin}`));
    return;
  }
  if (!existsSync(binaryPath)) {
    output.push(finding("selector_launcher_missing", binaryPath, "selector launcher is missing"));
  } else {
    const entry = statSync(binaryPath);
    if (!entry.isFile() || (process.platform !== "win32" && (entry.mode & 0o111) === 0)) {
      output.push(finding("selector_launcher_not_executable", binaryPath, "selector launcher is not executable"));
    }
  }

  inspectDependencySections(manifest, manifestPath, output);
  inspectSelectorTopology(packageDir, manifestPath, manifest, output);
  const packedFiles = inspectPackList(packageDir, output);
  if (!packedFiles.includes("bin/runx")) {
    output.push(finding("selector_pack_launcher_missing", path.join(packageDir, "bin", "runx"), "packed selector is missing bin/runx"));
  }
  if (!packedFiles.includes("native/supported-platforms.json")) {
    output.push(finding("selector_pack_topology_missing", path.join(packageDir, "native", "supported-platforms.json"), "packed selector is missing native/supported-platforms.json"));
  }
  if (options.noJsDelegation) {
    // The selector's bin/runx is the JS launcher; scan it for delegation tokens.
    inspectTextFiles(packageDir, packedFiles, output, null);
  }
}

function inspectSelectorTopology(
  packageDir: string,
  manifestPath: string,
  manifest: {
    readonly version?: string;
    readonly files?: readonly string[];
    readonly runx?: {
      readonly nativeSelector?: {
        readonly schema?: string;
        readonly supportedPlatforms?: readonly string[];
        readonly nativePackagePattern?: string;
      };
    };
    readonly optionalDependencies?: Record<string, string>;
  },
  output: Finding[],
): void {
  const selector = manifest.runx?.nativeSelector;
  if (selector?.schema !== "runx.rust_cli_selector_topology.v1") {
    output.push(finding("selector_topology_schema_invalid", manifestPath, "runx.nativeSelector schema must be runx.rust_cli_selector_topology.v1"));
  }
  const expectedPlatforms = supportedPlatforms.map((entry) => entry.key);
  if (!sameStringSet(selector?.supportedPlatforms ?? [], expectedPlatforms)) {
    output.push(finding("selector_supported_platforms_invalid", manifestPath, `selector must list supported platforms: ${expectedPlatforms.join(", ")}`));
  }
  if (selector?.nativePackagePattern !== `${selectorPackageName}-\${platform}`) {
    output.push(finding("selector_native_package_pattern_invalid", manifestPath, "selector native package pattern must be @runxhq/cli-${platform}"));
  }

  const expectedOptionalDependencies = Object.fromEntries(
    supportedPlatforms.map((entry) => [nativePackageName(entry.key), manifest.version]),
  );
  for (const [name, version] of Object.entries(expectedOptionalDependencies)) {
    if (manifest.optionalDependencies?.[name] !== version) {
      output.push(finding("selector_optional_dependency_missing", manifestPath, `optionalDependencies.${name} must be ${version}`));
    }
  }
  for (const name of Object.keys(manifest.optionalDependencies ?? {})) {
    if (!Object.hasOwn(expectedOptionalDependencies, name)) {
      output.push(finding("selector_optional_dependency_unknown", manifestPath, `unexpected selector optional dependency ${name}`));
    }
  }

  const topologyPath = path.join(packageDir, "native", "supported-platforms.json");
  if (!existsSync(topologyPath)) {
    output.push(finding("selector_topology_manifest_missing", topologyPath, "native/supported-platforms.json is required"));
    return;
  }
  const topology = readJson<{
    readonly schema?: string;
    readonly selectorPackage?: string;
    readonly nativePackages?: Record<string, { readonly package?: string; readonly os?: string; readonly cpu?: string; readonly binary?: string; readonly worker?: string }>;
  }>(topologyPath, output, "selector_topology_manifest_malformed");
  if (!topology) {
    return;
  }
  if (topology.schema !== "runx.rust_cli_selector_topology.v1") {
    output.push(finding("selector_topology_manifest_schema_invalid", topologyPath, "topology manifest schema must be runx.rust_cli_selector_topology.v1"));
  }
  if (topology.selectorPackage !== selectorPackageName) {
    output.push(finding("selector_topology_manifest_selector_invalid", topologyPath, `topology selectorPackage must be ${selectorPackageName}`));
  }
  for (const spec of supportedPlatforms) {
    const entry = topology.nativePackages?.[spec.key];
    if (!entry) {
      output.push(finding("selector_topology_platform_missing", topologyPath, `topology manifest is missing ${spec.key}`));
      continue;
    }
    if (entry.package !== nativePackageName(spec.key) || entry.os !== spec.os || entry.cpu !== spec.cpu || entry.binary !== spec.binary || entry.worker !== spec.worker) {
      output.push(finding("selector_topology_platform_invalid", topologyPath, `topology manifest has invalid metadata for ${spec.key}`));
    }
  }
}

function inspectNativePackageManifest(
  manifestPath: string,
  manifest: {
    readonly name?: string;
    readonly bin?: string | { readonly runx?: string };
    readonly os?: readonly string[];
    readonly cpu?: readonly string[];
    readonly runx?: {
      readonly nativePackage?: {
        readonly schema?: string;
        readonly selectorPackage?: string;
        readonly platform?: string;
        readonly binary?: string;
        readonly worker?: string;
      };
    };
  },
  output: Finding[],
): void {
  const spec = nativeSpecForPackage(manifest.name);
  if (!spec) {
    output.push(finding("native_package_name_invalid", manifestPath, `native package name must be one of ${supportedPlatforms.map((entry) => nativePackageName(entry.key)).join(", ")}`));
    return;
  }
  if (Object.hasOwn(manifest, "bin")) {
    output.push(finding("native_package_bin_unexpected", manifestPath, "native package must not declare package.json bin; use runx.nativePackage.binary"));
  }
  if (!sameStringSet(manifest.os ?? [], [spec.os])) {
    output.push(finding("native_package_os_invalid", manifestPath, `native package os must be ${spec.os}`));
  }
  if (!sameStringSet(manifest.cpu ?? [], [spec.cpu])) {
    output.push(finding("native_package_cpu_invalid", manifestPath, `native package cpu must be ${spec.cpu}`));
  }
  const native = manifest.runx?.nativePackage;
  if (native?.schema !== "runx.rust_cli_native_package.v1") {
    output.push(finding("native_package_schema_invalid", manifestPath, "runx.nativePackage schema must be runx.rust_cli_native_package.v1"));
  }
  if (native?.selectorPackage !== selectorPackageName) {
    output.push(finding("native_package_selector_invalid", manifestPath, `native package selectorPackage must be ${selectorPackageName}`));
  }
  if (native?.platform !== spec.key) {
    output.push(finding("native_package_platform_invalid", manifestPath, `native package platform must be ${spec.key}`));
  }
  if (native?.binary !== spec.binary) {
    output.push(finding("native_package_binary_invalid", manifestPath, `runx.nativePackage.binary must be ${spec.binary}`));
  }
  if (native?.worker !== spec.worker) {
    output.push(finding("native_package_worker_invalid", manifestPath, `runx.nativePackage.worker must be ${spec.worker}`));
  }
}

function inspectDependencySections(
  manifest: {
    readonly dependencies?: Record<string, string>;
    readonly optionalDependencies?: Record<string, string>;
    readonly devDependencies?: Record<string, string>;
    readonly peerDependencies?: Record<string, string>;
  },
  manifestPath: string,
  output: Finding[],
): void {
  for (const sectionName of ["dependencies", "optionalDependencies", "devDependencies", "peerDependencies"] as const) {
    const section = manifest[sectionName];
    if (!section) continue;
    for (const [name, spec] of Object.entries(section)) {
      if (["@runxhq/adapters", "@runxhq/contracts", "@runxhq/extension-sdk", "@runxhq/runtime-local"].includes(name)) {
        output.push(finding("ts_runtime_dependency", manifestPath, `${sectionName}.${name} is not allowed in the Rust CLI artifact`));
      }
      if (spec.startsWith("workspace:")) {
        output.push(finding("workspace_dependency", manifestPath, `${sectionName}.${name} still uses ${spec}`));
      }
    }
  }
}

function inspectForbiddenManifestEntrypoints(
  manifest: {
    readonly main?: unknown;
    readonly types?: unknown;
    readonly exports?: unknown;
    readonly scripts?: unknown;
  },
  manifestPath: string,
  output: Finding[],
): void {
  for (const field of ["main", "types", "exports", "scripts"] as const) {
    if (Object.hasOwn(manifest, field)) {
      output.push(finding("js_package_entrypoint", manifestPath, `Rust CLI artifact must not declare package.json ${field}`));
    }
  }
}

function inspectChecksum(packageDir: string, bin: string, worker: string, output: Finding[]): void {
  const checksumPath = path.join(packageDir, "native", "checksums.json");
  if (!existsSync(checksumPath)) {
    output.push(finding("checksum_manifest_missing", checksumPath, "native/checksums.json is required"));
    return;
  }
  const checksum = readJson<{
    readonly schema?: string;
    readonly package?: string;
    readonly platform?: string;
    readonly binary?: string;
    readonly binary_digest?: string;
    readonly worker?: string;
    readonly worker_digest?: string;
  }>(checksumPath, output, "checksum_manifest_malformed");
  if (!checksum) {
    return;
  }
  if (checksum.schema !== "runx.rust_cli_artifact_checksums.v1") {
    output.push(finding("checksum_schema_invalid", checksumPath, "checksum schema must be runx.rust_cli_artifact_checksums.v1"));
  }
  if (checksum.binary !== stripDotSlash(bin)) {
    output.push(finding("checksum_binary_mismatch", checksumPath, `checksum binary ${checksum.binary ?? "<missing>"} does not match ${bin}`));
    return;
  }
  const manifest = readJson<{ readonly name?: string; readonly runx?: { readonly nativePackage?: { readonly platform?: string } } }>(
    path.join(packageDir, "package.json"),
    [],
    "package_manifest_malformed",
  );
  if (manifest?.name && checksum.package !== manifest.name) {
    output.push(finding("checksum_package_mismatch", checksumPath, `checksum package ${checksum.package ?? "<missing>"} does not match ${manifest.name}`));
  }
  const expectedPlatform = manifest?.runx?.nativePackage?.platform;
  if (expectedPlatform && checksum.platform !== expectedPlatform) {
    output.push(finding("checksum_platform_mismatch", checksumPath, `checksum platform ${checksum.platform ?? "<missing>"} does not match ${expectedPlatform}`));
  }
  if (!isDigest(checksum.binary_digest)) {
    output.push(finding("checksum_binary_digest_invalid", checksumPath, "checksum binary_digest must be sha256: and a 64-character lowercase hex digest"));
    return;
  }
  const binaryPath = path.resolve(packageDir, bin);
  if (existsSync(binaryPath) && checksum.binary_digest !== sha256(readFileSync(binaryPath))) {
    output.push(finding("checksum_mismatch", checksumPath, "binary digest does not match native/checksums.json"));
  }
  if (checksum.worker !== worker) {
    output.push(finding("checksum_worker_mismatch", checksumPath, `checksum worker ${checksum.worker ?? "<missing>"} does not match ${worker}`));
    return;
  }
  if (!isDigest(checksum.worker_digest)) {
    output.push(finding("checksum_worker_digest_invalid", checksumPath, "worker_digest must be sha256: and a 64-character lowercase hex digest"));
    return;
  }
  const workerPath = path.resolve(packageDir, worker);
  if (existsSync(workerPath) && checksum.worker_digest !== sha256(readFileSync(workerPath))) {
    output.push(finding("checksum_worker_mismatch", checksumPath, "worker digest does not match native/checksums.json"));
  }
}

function inspectSignature(packageDir: string, bin: string, worker: string, output: Finding[]): void {
  const signaturePath = path.join(packageDir, "native", "signatures.json");
  if (!existsSync(signaturePath)) {
    if (options.verifySignatures) {
      output.push(finding("signature_manifest_missing", signaturePath, "native/signatures.json is required by --verify-signatures"));
    }
    return;
  }

  const signature = readJson<{
    readonly schema?: string;
    readonly package?: string;
    readonly platform?: string;
    readonly binary?: string;
    readonly binary_digest?: string;
    readonly worker?: string;
    readonly worker_digest?: string;
    readonly signatures?: readonly unknown[];
  }>(signaturePath, output, "signature_manifest_malformed");
  if (!signature) {
    return;
  }
  if (signature.schema !== "runx.rust_cli_artifact_signatures.v1") {
    output.push(finding("signature_schema_invalid", signaturePath, "signature manifest schema must be runx.rust_cli_artifact_signatures.v1"));
  }
  if (signature.binary !== stripDotSlash(bin)) {
    output.push(finding("signature_binary_mismatch", signaturePath, `signature binary ${signature.binary ?? "<missing>"} does not match ${bin}`));
  }
  const manifest = readJson<{ readonly name?: string; readonly runx?: { readonly nativePackage?: { readonly platform?: string } } }>(
    path.join(packageDir, "package.json"),
    [],
    "package_manifest_malformed",
  );
  if (manifest?.name && signature.package !== manifest.name) {
    output.push(finding("signature_package_mismatch", signaturePath, `signature package ${signature.package ?? "<missing>"} does not match ${manifest.name}`));
  }
  const expectedPlatform = manifest?.runx?.nativePackage?.platform;
  if (expectedPlatform && signature.platform !== expectedPlatform) {
    output.push(finding("signature_platform_mismatch", signaturePath, `signature platform ${signature.platform ?? "<missing>"} does not match ${expectedPlatform}`));
  }
  if (!isDigest(signature.binary_digest)) {
    output.push(finding("signature_binary_digest_invalid", signaturePath, "signature binary_digest must be sha256: and a 64-character lowercase hex digest"));
  }
  if (signature.worker !== worker) {
    output.push(finding("signature_worker_mismatch", signaturePath, `signature worker ${signature.worker ?? "<missing>"} does not match ${worker}`));
  }
  if (!isDigest(signature.worker_digest)) {
    output.push(finding("signature_worker_digest_invalid", signaturePath, "signature worker_digest must be sha256: and a 64-character lowercase hex digest"));
  }
  const checksum = readJson<{ readonly binary_digest?: string; readonly worker_digest?: string }>(path.join(packageDir, "native", "checksums.json"), [], "checksum_manifest_malformed");
  if (checksum?.binary_digest && signature.binary_digest && checksum.binary_digest !== signature.binary_digest) {
    output.push(finding("signature_checksum_mismatch", signaturePath, "signature binary_digest does not match native/checksums.json"));
  }
  if (checksum?.worker_digest && signature.worker_digest && checksum.worker_digest !== signature.worker_digest) {
    output.push(finding("signature_worker_checksum_mismatch", signaturePath, "signature worker_digest does not match native/checksums.json"));
  }
  if (!Array.isArray(signature.signatures) || signature.signatures.length === 0) {
    output.push(finding("signature_entries_missing", signaturePath, "signature manifest must include at least one signature entry"));
  } else {
    for (const [index, entry] of signature.signatures.entries()) {
      if (!isRustCliSignatureEntry(entry)) {
        output.push(finding("signature_entry_invalid", signaturePath, `signature entry ${index} must include non-empty kind and value strings`));
      }
    }
  }
}

function inspectPackList(packageDir: string, output: Finding[]): readonly string[] {
  try {
    const pack = execFileSync(npm, ["pack", "--dry-run", "--json"], {
      cwd: packageDir,
      encoding: "utf8",
      maxBuffer: 1024 * 1024,
      // Windows package-manager shims are .cmd files; execFileSync needs a
      // shell to execute them reliably. Arguments here are fixed literals.
      shell: process.platform === "win32",
    });
    const [report] = JSON.parse(pack) as [{ readonly files?: readonly { readonly path: string }[] }];
    const files = (report.files ?? []).map((entry) => entry.path).sort();
    for (const entryPath of files) {
      if (/^(dist|src|tools|node_modules)\//u.test(entryPath) || /^bin\/runx\.(?:js|mjs|cjs)$/u.test(entryPath)) {
        output.push(finding("pack_contains_js_runtime", path.join(packageDir, entryPath), "packed Rust CLI artifact contains a JS/TS runtime path"));
      }
    }
    return files;
  } catch (error) {
    output.push(finding("pack_dry_run_failed", packageDir, errorMessage(error)));
    return [];
  }
}

function inspectTextFiles(packageDir: string, packedFiles: readonly string[], output: Finding[], excludeRelative: string | null): void {
  for (const relative of packedFiles) {
    if (relative === excludeRelative) {
      continue;
    }
    if (relative !== "bin/runx" && !/\.(?:json|md|txt|js|mjs|cjs|ts|tsx)$/u.test(relative)) {
      continue;
    }
    const filePath = path.join(packageDir, relative);
    const text = readFileSync(filePath, "utf8");
    for (const token of [
      "RUNX_JS_BIN",
      "RUNX_NPM_PACKAGE",
      "RUNX_RUST_CLI",
      "RUNX_RUST_HARNESS",
      "npm exec",
      "packages/cli/src",
      "packages/cli/dist",
      "process.execPath",
      "dist/index.js",
      "dist/src",
    ]) {
      if (text.includes(token)) {
        output.push(finding("js_delegation_token", filePath, `artifact contains forbidden delegation token ${token}`));
      }
    }
  }
}

function inspectPublishTargets(packageDirs: readonly string[], output: Finding[]): void {
  const seen = new Map<string, string>();
  for (const packageDir of packageDirs) {
    const manifestPath = path.join(packageDir, "package.json");
    const manifest = readJson<{ readonly name?: string; readonly version?: string }>(
      manifestPath,
      [],
      "package_manifest_malformed",
    );
    if (!manifest) continue;
    const key = `${manifest.name ?? "<missing-name>"}@${manifest.version ?? "<missing-version>"}`;
    const previous = seen.get(key);
    if (previous) {
      output.push(finding("duplicate_publish_target", manifestPath, `duplicate npm publish target ${key}; first seen at ${displayPath(previous)}`));
    } else {
      seen.set(key, manifestPath);
    }
  }
}

function isSelectorPackage(manifest: { readonly name?: string; readonly runx?: { readonly nativeSelector?: unknown } }): boolean {
  return manifest.name === selectorPackageName && Boolean(manifest.runx?.nativeSelector);
}

function nativePackageName(platform: string): string {
  return nativeRustCliPackageName(selectorPackageName, platform);
}

function nativeSpecForPackage(packageName: string | undefined): RustCliPlatformSpec | undefined {
  return supportedPlatforms.find((entry) => nativePackageName(entry.key) === packageName);
}

function sameStringSet(actual: readonly string[], expected: readonly string[]): boolean {
  if (actual.length !== expected.length) return false;
  const actualSet = new Set(actual);
  return expected.every((value) => actualSet.has(value));
}

function stripDotSlash(value: string): string {
  return value.replace(/^\.\//u, "");
}

function readJson<T>(filePath: string, output: Finding[], rule: string): T | null {
  try {
    return JSON.parse(readFileSync(filePath, "utf8")) as T;
  } catch (error) {
    output.push(finding(rule, filePath, errorMessage(error)));
    return null;
  }
}

function isInside(candidatePath: string, rootPath: string): boolean {
  const relative = path.relative(rootPath, candidatePath);
  return relative === "" || (!relative.startsWith("..") && !path.isAbsolute(relative));
}

function sha256(bytes: Buffer): string {
  return `sha256:${createHash("sha256").update(bytes).digest("hex")}`;
}

function isDigest(value: string | undefined): value is string {
  return value !== undefined && /^sha256:[0-9a-f]{64}$/u.test(value);
}

function finding(rule: string, filePath: string, message: string): Finding {
  return { rule, file: displayPath(filePath), message };
}

function displayPath(filePath: string): string {
  const relative = path.relative(workspaceRoot, filePath);
  return relative && !relative.startsWith("..") ? relative.split(path.sep).join("/") : filePath;
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function emit(payload: unknown): void {
  console.log(JSON.stringify(payload, null, 2));
}

function printUsage(): void {
  console.log("Usage: pnpm exec tsx scripts/check-rust-cli-release-artifacts.ts [--artifact-dir .runx/rust-cli-artifacts] [--no-js-delegation] [--verify-signatures]");
}
