import path from "node:path";
import { validateProcessContract, validateProcessListContract, validateProcessOutputContract, validateRunReadContract, validateRunxListReportContract, validateSkillInspectionContract } from "@runxhq/contracts";
import { readProjectArtifact } from "./artifacts.js";
import { decodeSkill, parseJson, parseValue, record } from "./decode.js";
import { invoke, RunxClientError } from "./process.js";
import type { ExecutionBinding, InvocationOptions, NativeConnectorPackageOutput, NativeHarnessOutput, NativePackageOutput, ReceiptVerifyOutput, RunAuthority, RunxClient, RunxClientConfig, SkillInspection } from "./types.js";

export { hostEnvironmentFrom } from "./environment.js";
export { RunxClientError } from "./process.js";
export { executionBindingFromInspection, executionClosureFromInspection, assertInspectionMatchesBinding } from "./inspection.js";
export type * from "./types.js";
export type { ExecutionPackageBinding, ExecutionClosureInspection } from "./inspection.js";

export function createRunxClient(configuration: RunxClientConfig): RunxClient {
  const config = { ...configuration, environment: { ...configuration.environment } };
  for (const name of ["command", "cwd", "projectDir", "home"] as const) {
    if (!config[name]?.trim()) throw new TypeError(`Runx client requires ${name}.`);
  }
  for (const name of ["cwd", "projectDir", "home"] as const) {
    if (!path.isAbsolute(config[name])) throw new TypeError(`Runx ${name} must be absolute.`);
  }
  const timeoutMs = positive(config.timeoutMs ?? 15 * 60 * 1000, "timeoutMs");
  const terminationGraceMs = positive(config.terminationGraceMs ?? 1000, "terminationGraceMs");
  if (timeoutMs > 2147483647 || terminationGraceMs > 2147483647) throw new TypeError("Runx timer limits exceed the platform timer range.");
  const maxOutputBytes = positive(config.maxOutputBytes ?? 64 * 1024 * 1024, "maxOutputBytes");
  const maxInputBytes = positive(config.maxInputBytes ?? 8 * 1024 * 1024, "maxInputBytes");
  const maxArtifactBytes = positive(config.maxArtifactBytes ?? 64 * 1024 * 1024, "maxArtifactBytes");
  const trustedEnv = { ...config.environment, NO_COLOR: "1", RUNX_RUST_CLI: "1", RUNX_CWD: config.cwd, RUNX_PROJECT_DIR: config.projectDir, RUNX_HOME: config.home };
  const execute = (args: readonly string[], options: InvocationOptions, input?: string) => invoke({
    command: config.command, args, cwd: config.cwd, env: { ...skillEnvironment(options.environment), ...trustedEnv },
    timeoutMs, terminationGraceMs, maxOutputBytes, maxInputBytes, input, signal: options.signal,
  });
  // A run's inputs and a resume's resolutions travel on stdin: never in
  // argv, and never in a file another process could read.
  const payload = (value: unknown): string => {
    const text = JSON.stringify(value);
    if (text === undefined) throw new RunxClientError("input_limit", "Runx input is not JSON.");
    return text;
  };
  return {
    skills: {
      list: async (options = {}) => {
        const output = parseJson(await execute(["list", "skills", "--json"], options), [0]);
        try { return validateRunxListReportContract(output); } catch (error) { throw new RunxClientError("contract", "Invalid native skill listing.", { cause: error }); }
      },
      inspect: async (options) => {
        const output = parseJson(await execute(["skill", "inspect", argument(options.skillPath), ...runnerArgs(options.runner), ...authorityArgs(options.runAuthority), ...(options.inputs ? ["--inputs", "-"] : []), "--json"], options, options.inputs ? payload(options.inputs) : undefined), [0]);
        try { return validateSkillInspectionContract(output); } catch (error) { throw new RunxClientError("contract", "Invalid native skill inspection.", { cause: error }); }
      },
      run: async (options) => {
        const receiptDir = absolute(options.receiptDir);
        if (options.binding && options.runner && options.runner !== options.binding.runner) throw new TypeError("Selected runner differs from the execution binding.");
        const result = await execute(["skill", argument(options.skillPath), ...runnerArgs(options.binding?.runner ?? options.runner), ...bindingArgs(options.binding), ...authorityArgs(options.runAuthority), ...(options.act ? ["--act", JSON.stringify(options.act)] : []), ...(options.context ? ["--context", JSON.stringify(options.context)] : []), ...(options.runId ? ["--run-id", options.runId] : []), "--inputs", "-", "--receipt-dir", receiptDir, "--json"], options, payload(options.inputs ?? {}));
        return await decodeSkill(result, { projectDir: config.projectDir, receiptDir, maxArtifactBytes });
      },
    },
    runs: {
      read: async (options) => {
        if (options.after !== undefined && !/^[A-Za-z0-9_-]+$/u.test(options.after)) throw new TypeError("Invalid run read cursor.");
        // The cursor goes inline: base64url may begin with "-".
        const output = parseJson(await execute(["history", argument(options.runId), ...(options.after ? [`--after=${options.after}`] : []), "--json"], options), [0]);
        try { return validateRunReadContract(output); } catch (error) { throw new RunxClientError("contract", "Invalid native run read.", { cause: error }); }
      },
      resume: async (options) => {
        const receiptDir = absolute(options.receiptDir);
        const result = await execute(["resume", argument(options.runId), "-", ...bindingArgs(options.binding), "--receipt-dir", receiptDir, "--json"], options, payload(options.resolutions));
        return await decodeSkill(result, { projectDir: config.projectDir, receiptDir, maxArtifactBytes });
      },
    },
    turns: {
      server: (options) => ({
        command: config.command,
        args: ["mcp", "--run", argument(options.runId), "--receipt-dir", absolute(options.receiptDir)],
        env: Object.fromEntries(
          Object.entries({ ...skillEnvironment(options.environment), ...trustedEnv })
            .filter((entry): entry is [string, string] => typeof entry[1] === "string"),
        ),
        cwd: config.cwd,
      }),
    },
    processes: {
      list: async (options = {}) => {
        const output = parseJson(await execute(["process", "list", ...(options.actor ? [`--actor=${options.actor}`] : []), "--json"], options), [0]);
        try { return validateProcessListContract(output); } catch (error) { throw new RunxClientError("contract", "Invalid native process listing.", { cause: error }); }
      },
      read: async (options) => {
        for (const [name, value] of [["after", options.after], ["maxBytes", options.maxBytes]] as const) {
          if (value !== undefined && (!Number.isSafeInteger(value) || value < 0)) throw new TypeError(`Invalid process read ${name}.`);
        }
        const output = parseJson(await execute(["process", "read", argument(options.processId), ...(options.after !== undefined ? ["--after", String(options.after)] : []), ...(options.maxBytes !== undefined ? ["--max-bytes", String(options.maxBytes)] : []), "--json"], options), [0]);
        try { return validateProcessOutputContract(output); } catch (error) { throw new RunxClientError("contract", "Invalid native process read.", { cause: error }); }
      },
      stop: async (options) => {
        const output = parseJson(await execute(["process", "stop", argument(options.processId), "--json"], options), [0]);
        try { return validateProcessContract(output); } catch (error) { throw new RunxClientError("contract", "Invalid native process.", { cause: error }); }
      },
    },
    packages: {
      materialize: async (options) => {
        const output = parseJson(await execute(["registry", "package", argument(options.skillPath), "--json"], options), [0]);
        if (output.status !== "success" || !record(output.registry) || output.registry.action !== "package" || !record(output.registry.package)
          || typeof output.registry.package.markdown !== "string" || typeof output.registry.package_digest !== "string"
          || !/^sha256:[0-9a-f]{64}$/u.test(output.registry.package_digest)) throw new RunxClientError("contract", "Invalid native package material.");
        return output as NativePackageOutput;
      },
      materializeConnector: async (options) => {
        const output = parseJson(await execute(["registry", "package", argument(options.connectorPath), "--json"], options), [0]);
        const connector = record(output.registry) ? output.registry.connector : undefined;
        if (output.status !== "success" || !record(output.registry) || output.registry.action !== "connector_package" || !record(connector)
          || !["connector", "version", "content_digest", "manifest_digest", "connector_digest"].every((field) => typeof connector[field] === "string")
          || !record(connector.manifest) || !record(connector.digests) || !Array.isArray(connector.files)
          || !connector.files.every((file) => record(file) && typeof file.path === "string" && typeof file.content === "string")) {
          throw new RunxClientError("contract", "Invalid native connector package.");
        }
        return output as NativeConnectorPackageOutput;
      },
    },
    harness: {
      run: async (options) => {
        const output = parseJson(await execute(["harness", argument(options.skillPath), "--json", "--receipt-dir", absolute(options.receiptDir)], options), [0, 1]);
        if (!["passed", "failed", "not_declared"].includes(String(output.status)) || ![output.case_count, output.assertion_error_count, output.graph_case_count].every((value) => typeof value === "number" && Number.isSafeInteger(value) && value >= 0) || ![output.assertion_errors, output.case_names, output.receipt_ids].every((value) => Array.isArray(value) && value.every((item) => typeof item === "string"))) throw new RunxClientError("contract", "Invalid native harness summary.");
        return output as unknown as NativeHarnessOutput;
      },
    },
    artifacts: {
      read: async (reference, options = {}) => await readProjectArtifact(reference, config.projectDir, positive(options.maxBytes ?? maxArtifactBytes, "maxBytes")),
    },
    secrets: {
      read: async (options) => parseValue(await execute(["secret", "read", argument(options.name)], options)),
    },
    receipts: {
      verify: async (options) => {
        const output = parseJson(await execute(["verify", "--receipt", "-", "--trust", options.trust ?? "home", "--json"], options, options.receipt), [0, 3]);
        if (output.schema !== "runx.verify_verdict.v1" || typeof output.valid !== "boolean" || (output.receipt_id !== null && typeof output.receipt_id !== "string")) throw new RunxClientError("contract", "Invalid native receipt verification verdict.");
        return output as ReceiptVerifyOutput;
      },
    },
  };
}

function positive(value: number, name: string): number {
  if (!Number.isSafeInteger(value) || value <= 0) throw new TypeError(`Runx ${name} must be a positive safe integer.`);
  return value;
}
function argument(value: string): string {
  if (!value.trim() || value.startsWith("-") || value.includes("\0")) throw new TypeError("Invalid native positional argument.");
  return value;
}
function absolute(value: string): string {
  if (!path.isAbsolute(value)) throw new TypeError("Runx receiptDir must be absolute.");
  return argument(value);
}
function runnerArgs(value?: string): string[] { return value ? [argument(value)] : []; }
function authorityArgs(authority?: RunAuthority): string[] { return authority ? ["--run-authority", JSON.stringify(authority)] : []; }
function bindingArgs(binding?: ExecutionBinding): string[] {
  if (!binding) return [];
  argument(binding.runner);
  if (![binding.packageDigest, binding.executionClosureDigest].every((value) => /^sha256:[0-9a-f]{64}$/u.test(value))) throw new TypeError("Invalid native execution binding digests.");
  return ["--package-digest", binding.packageDigest, "--execution-closure-digest", binding.executionClosureDigest];
}
function skillEnvironment(environment?: Readonly<Record<string, string>>): NodeJS.ProcessEnv {
  for (const [name, value] of Object.entries(environment ?? {})) {
    if (!/^[_A-Za-z][_A-Za-z0-9]*$/u.test(name) || typeof value !== "string" || value.includes("\0")) throw new TypeError(`Invalid skill environment variable '${name}'.`);
    if (/^(RUNX_|NODE_|LD_|DYLD_)/u.test(name.toUpperCase()) || ["PATH", "HOME", "NO_COLOR", "BASH_ENV", "ENV", "COMSPEC", "PATHEXT"].includes(name.toUpperCase())) throw new TypeError(`Skill environment cannot override runtime-owned variable '${name}'.`);
  }
  return { ...environment };
}
