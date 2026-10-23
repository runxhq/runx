import { readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

// Single source of truth for the runx CLI release version across every CLI
// channel. A cli-vX.Y.Z tag is the CLI distribution version only. It stamps the
// npm selector/native packages, the `runx-cli` crate, and the runtime's
// execution-closure release identity so both displayed and bound versions stay
// truthful. Internal library crate versions remain independent and unpublished.

const workspaceRoot = path.resolve(fileURLToPath(new URL("..", import.meta.url)));
const SEMVER = /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/u;

interface Options {
  readonly version: string;
  readonly check: boolean;
}

interface Finding {
  readonly file: string;
  readonly message: string;
}

const packageJsonPath = path.join(workspaceRoot, "packages", "cli", "package.json");
const cargoLockPath = path.join(workspaceRoot, "crates", "Cargo.lock");
const cargoCliPackagePath = path.join(workspaceRoot, "crates", "runx-cli", "Cargo.toml");
const runtimeSourcePath = path.join(workspaceRoot, "crates", "runx-runtime", "src", "lib.rs");
const topologyPath = path.join(workspaceRoot, "packages", "cli", "native", "supported-platforms.json");
const options = parseArgs(process.argv.slice(2));
const findings: Finding[] = [];

stampPackageJson(packageJsonPath, options, findings);
stampCargoPackage(cargoCliPackagePath, options, findings);
stampRustConstant(runtimeSourcePath, "RUNX_VERSION", options, findings);
stampCargoLock(cargoLockPath, ["runx-cli"], options, findings);

if (findings.length > 0) {
  emit({ status: options.check ? "drift" : "failed", version: options.version, findings });
  process.exit(1);
}
emit({
  status: options.check ? "matched" : "stamped",
  version: options.version,
  files: [
    relative(packageJsonPath),
    relative(cargoCliPackagePath),
    relative(runtimeSourcePath),
    relative(cargoLockPath),
  ],
});

function stampPackageJson(filePath: string, opts: Options, output: Finding[]): void {
  const raw = readFileSync(filePath, "utf8");
  const manifest = JSON.parse(raw) as {
    name?: string;
    version?: string;
    optionalDependencies?: Record<string, string>;
    runx?: { nativeSelector?: { supportedPlatforms?: string[]; nativePackagePattern?: string } };
  };
  const topology = JSON.parse(readFileSync(topologyPath, "utf8")) as {
    nativePackages?: Record<string, { package?: string }>;
  };
  const platforms = Object.keys(topology.nativePackages ?? {});
  const selector = manifest.runx?.nativeSelector;
  if (!manifest.name || !selector || platforms.length === 0) {
    output.push({ file: relative(filePath), message: "CLI selector or platform topology is incomplete" });
    return;
  }
  for (const platform of platforms) {
    const expected = selector.nativePackagePattern?.replace("${platform}", platform);
    if (topology.nativePackages?.[platform]?.package !== expected) {
      output.push({ file: relative(filePath), message: `selector package mapping disagrees with topology for ${platform}` });
    }
  }
  if (opts.check) {
    if (manifest.version !== opts.version) {
      output.push({ file: relative(filePath), message: `version is ${manifest.version}, expected ${opts.version}` });
    }
    for (const [name, spec] of Object.entries(manifest.optionalDependencies ?? {})) {
      if (spec !== opts.version) {
        output.push({ file: relative(filePath), message: `optionalDependencies.${name} is ${spec}, expected ${opts.version}` });
      }
    }
    if (JSON.stringify(selector.supportedPlatforms) !== JSON.stringify(platforms)) {
      output.push({ file: relative(filePath), message: "supportedPlatforms disagrees with platform topology" });
    }
    return;
  }
  manifest.version = opts.version;
  selector.supportedPlatforms = platforms;
  for (const name of Object.keys(manifest.optionalDependencies ?? {})) {
    manifest.optionalDependencies![name] = opts.version;
  }
  writeFileSync(filePath, `${JSON.stringify(manifest, null, 2)}\n`);
}

function stampCargoPackage(filePath: string, opts: Options, output: Finding[]): void {
  const raw = readFileSync(filePath, "utf8");
  // Match the first `version = "..."` in the [package] section.
  const match = raw.match(/^version = "([^"]*)"/mu);
  if (!match) {
    output.push({ file: relative(filePath), message: "could not find a package version line" });
    return;
  }
  if (opts.check) {
    if (match[1] !== opts.version) {
      output.push({ file: relative(filePath), message: `version is ${match[1]}, expected ${opts.version}` });
    }
    return;
  }
  writeFileSync(filePath, raw.replace(/^version = "[^"]*"/mu, `version = "${opts.version}"`));
}

function stampRustConstant(
  filePath: string,
  name: string,
  opts: Options,
  output: Finding[],
): void {
  const raw = readFileSync(filePath, "utf8");
  const pattern = new RegExp(`(pub const ${escapeRegExp(name)}: &str = ")([^"]+)(";)$`, "mu");
  const match = raw.match(pattern);
  if (!match) {
    output.push({ file: relative(filePath), message: `could not find Rust constant ${name}` });
    return;
  }
  if (opts.check) {
    if (match[2] !== opts.version) {
      output.push({ file: relative(filePath), message: `${name} is ${match[2]}, expected ${opts.version}` });
    }
    return;
  }
  writeFileSync(filePath, raw.replace(pattern, `$1${opts.version}$3`));
}

function stampCargoLock(filePath: string, packageNames: readonly string[], opts: Options, output: Finding[]): void {
  let raw = readFileSync(filePath, "utf8");
  for (const packageName of packageNames) {
    const escapedName = escapeRegExp(packageName);
    const block = new RegExp(`(name = "${escapedName}"\\r?\\nversion = ")([^"]+)(")`, "u");
    const match = raw.match(block);
    if (!match) {
      output.push({ file: relative(filePath), message: `could not find the ${packageName} lock entry` });
      continue;
    }
    if (opts.check) {
      if (match[2] !== opts.version) {
        output.push({
          file: relative(filePath),
          message: `${packageName} lock version is ${match[2]}, expected ${opts.version}`,
        });
      }
      continue;
    }
    raw = raw.replace(block, `$1${opts.version}$3`);
  }
  if (!opts.check) {
    writeFileSync(filePath, raw);
  }
}

function parseArgs(argv: readonly string[]): Options {
  let version = "";
  let check = false;
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--check") {
      check = true;
      continue;
    }
    if (arg === "--version") {
      version = argv[index + 1] ?? "";
      index += 1;
      continue;
    }
    if (arg === "--help" || arg === "-h") {
      console.log("Usage: tsx scripts/set-release-version.ts [--version X.Y.Z] [--check]");
      process.exit(0);
    }
    if (!version && !arg.startsWith("--")) {
      // Allow a bare positional version for convenience (e.g. from a tag).
      version = arg;
      continue;
    }
    throw new Error(`unknown argument: ${arg}`);
  }
  // Tolerate a leading cli-v / v prefix so the raw tag can be passed through.
  version = version.replace(/^(?:cli-)?v/u, "");
  if (version && !SEMVER.test(version)) {
    throw new Error(`--version must be semver (got "${version}")`);
  }
  if (!version) {
    version = currentPackageVersion(packageJsonPath);
  }
  return { version, check };
}

function currentPackageVersion(filePath: string): string {
  const manifest = JSON.parse(readFileSync(filePath, "utf8")) as { version?: string };
  const version = manifest.version ?? "";
  if (!SEMVER.test(version)) {
    throw new Error(`packages/cli/package.json has invalid version "${version}"`);
  }
  return version;
}

function relative(filePath: string): string {
  return path.relative(workspaceRoot, filePath).split(path.sep).join("/");
}

function escapeRegExp(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/gu, "\\$&");
}

function emit(payload: unknown): void {
  console.log(JSON.stringify(payload, null, 2));
}
