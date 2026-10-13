// The one Rust build the checks and test wrappers share: every shipped binary
// and build-only example in one cargo invocation, so the resolver unifies
// runx-runtime's features once and a later caller finds nothing to rebuild.
// Steps that need a binary read it from `prebuiltBinEnv` instead of invoking
// cargo again.
import path from "node:path";
import { fileURLToPath } from "node:url";

export const workspaceRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
export const cargo = process.platform === "win32" ? "cargo.exe" : "cargo";
export const cargoTargetDir = process.env.CARGO_TARGET_DIR
  ? path.resolve(workspaceRoot, process.env.CARGO_TARGET_DIR)
  : path.join(workspaceRoot, "crates", "target");

const PACKAGES = [
  "runx-cli",
  "runx-runtime",
  "runx-js-worker",
  "runx-contracts",
  "runx-receipts",
  "runx-core",
  "runx-parser",
];
const BINS = ["runx", "runx-js-worker"];
const EXAMPLES = [
  "runx-harness-fixture-oracles",
  "runx-native-capability-snapshot",
  "runx-schema-artifacts",
  "runx-paid-invocation-fixtures",
  "runx-principal-id-fixtures",
  "runx-maintained-connectors",
  "kernel_eval_batch",
  "parser_eval",
  "runx-x402-fixtures",
];

export const rustBuildArgs = [
  "build",
  "--quiet",
  "--manifest-path",
  path.join(workspaceRoot, "crates", "Cargo.toml"),
  ...PACKAGES.flatMap((name) => ["-p", name]),
  ...BINS.flatMap((name) => ["--bin", name]),
  ...EXAMPLES.flatMap((name) => ["--example", name]),
];

function executable(name) {
  return process.platform === "win32" ? `${name}.exe` : name;
}

export function binaryPath(name) {
  return path.join(cargoTargetDir, "debug", executable(name));
}

export function examplePath(name) {
  return path.join(cargoTargetDir, "debug", "examples", executable(name));
}

/** The prebuilt binaries a check or test runs instead of building its own. */
export const prebuiltBinEnv = {
  RUNX_RUST_CLI_BIN: binaryPath("runx"),
  RUNX_HARNESS_FIXTURE_ORACLE_BIN: examplePath("runx-harness-fixture-oracles"),
  RUNX_CAPABILITY_SNAPSHOT_BIN: examplePath("runx-native-capability-snapshot"),
  RUNX_SCHEMA_ARTIFACTS_BIN: examplePath("runx-schema-artifacts"),
  RUNX_PAID_INVOCATION_FIXTURES_BIN: examplePath("runx-paid-invocation-fixtures"),
  RUNX_PRINCIPAL_ID_FIXTURES_BIN: examplePath("runx-principal-id-fixtures"),
  RUNX_X402_FIXTURES_BIN: examplePath("runx-x402-fixtures"),
  RUNX_OFFICIAL_CONNECTORS_BIN: examplePath("runx-maintained-connectors"),
};
