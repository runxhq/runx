import { spawnSync } from "node:child_process";

interface NativeCommandSpec {
  readonly name: string;
  readonly topLevelUsage: readonly string[];
  readonly usage: readonly string[];
  readonly notes: readonly string[];
  readonly options: readonly string[];
}

interface NativeCommandCatalog {
  readonly schema: "runx.cli_command_catalog.v1";
  readonly root: NativeCommandSpec;
  readonly commands: readonly NativeCommandSpec[];
}

interface CommandAnnotation {
  readonly sideEffect: "none" | "filesystem" | "local-runtime" | "adapter" | "external-stub";
  readonly surfaces: readonly string[];
  readonly cases: readonly string[];
  readonly jsonOutput?: "schema-exact" | "none";
}

interface CommandMatrixEntry extends NativeCommandSpec {
  readonly parity: {
    readonly humanOutput: "semantic" | "none";
    readonly jsonOutput: "schema-exact" | "none";
    readonly receipt: "schema-exact" | "none";
    readonly sideEffect: "none" | "filesystem" | "local-runtime" | "adapter" | "external-stub";
    readonly surfaces: readonly string[];
  };
  readonly cases: readonly string[];
}

interface RuntimeSurfaceDefinition {
  readonly id: string;
  readonly owner: string;
  readonly parityClass: "schema-exact" | "semantic" | "fixture-backed" | "stubbed";
  readonly notes: string;
}

interface RuntimeSurface extends RuntimeSurfaceDefinition {
  readonly coveredBy: readonly string[];
}

export interface OracleCase {
  readonly id: string;
  readonly commandId: string;
  readonly mode: "execute" | "validate";
  readonly argv?: readonly string[];
  readonly expectedExitCode?: number;
  readonly expectJson?: boolean;
  readonly stdoutIncludes?: readonly string[];
  readonly stderrIncludes?: readonly string[];
  readonly env?: Readonly<Record<string, string>>;
  readonly proves: readonly string[];
}

const commandAnnotations: Readonly<Record<string, CommandAnnotation>> = {
  "cli.help": annotation("none", ["cli-presentation"], ["help.top-level", "usage.unsupported"]),
  new: annotation("local-runtime", ["skill-authoring", "graph-runtime", "caller-mediated-resolution", "receipts", "cli-presentation"], ["new.validate"]),
  init: annotation("filesystem", ["workspace-init", "official-skills"], ["init.validate"]),
  verify: annotation("none", ["receipts", "cli-presentation"], ["verify.validate"]),
  history: annotation("none", ["history", "ledger", "receipts"], ["history.execute"]),
  resume: annotation("local-runtime", ["caller-mediated-resolution", "graph-runtime", "receipts", "cli-presentation"], ["resume.validate"]),
  list: annotation("none", ["list", "tool-catalog"], ["list.tools.execute"]),
  login: annotation("filesystem", ["config", "cli-presentation"], ["login.validate"]),
  connect: annotation("external-stub", ["public-api", "connect", "cli-presentation"], ["connect.execute"]),
  connector: annotation("filesystem", ["connectors", "cli-presentation"], ["connector.list.execute", "connector.use.validate"]),
  config: annotation("filesystem", ["config", "cli-presentation"], ["config.set.validate", "config.get.validate", "config.list.execute"]),
  credential: annotation("filesystem", ["config", "skill-resolution", "cli-presentation"], ["credential.validate"]),
  policy: annotation("none", ["policy", "cli-presentation"], ["policy.inspect.validate", "policy.lint.validate"]),
  publish: annotation("external-stub", ["receipts", "cli-presentation"], ["publish.validate"]),
  kernel: annotation("local-runtime", ["graph-runtime", "cli-presentation"], ["kernel.validate"]),
  parser: annotation("local-runtime", ["parser", "cli-presentation"], ["parser.validate"]),
  doctor: annotation("filesystem", ["doctor", "cli-presentation"], ["doctor.validate"]),
  dev: annotation("local-runtime", ["dev", "harness", "receipts"], ["dev.validate"]),
  export: annotation("filesystem", ["skill-export", "cli-presentation"], ["export.validate"]),
  mcp: annotation("adapter", ["mcp"], ["mcp.serve.validate"], "none"),
  turn: annotation("local-runtime", ["mcp", "caller-mediated-resolution", "graph-runtime", "receipts"], ["turn.validate"], "none"),
  assistant: annotation("local-runtime", ["graph-runtime", "receipts", "cli-presentation"], ["assistant.validate"]),
  skill: annotation("local-runtime", ["skill-resolution", "graph-runtime", "receipts", "execution-boundary", "authority", "caller-mediated-resolution", "adapter-process-tool", "adapter-agent", "cli-presentation"], ["skill.run.validate", "skill.inspect.validate"]),
  add: annotation("external-stub", ["registry", "cli-presentation"], ["add.validate"]),
  harness: annotation("local-runtime", ["harness", "receipts", "execution-boundary"], ["harness.execute"]),
  tool: annotation("external-stub", ["tool-catalog", "extension-sdk"], ["tool.build.validate", "tool.search.validate", "tool.inspect.validate"]),
  registry: annotation("external-stub", ["registry", "cli-presentation"], ["registry.validate"]),
};

const surfaceDefinitions: readonly RuntimeSurfaceDefinition[] = [
  surface("cli-presentation", "runx-cli", "semantic", "Human output is normalized semantically; JSON output stays schema-exact."),
  surface("skill-resolution", "runx-cli + runx-runtime + runx-core", "fixture-backed", "Covers local paths, registry refs, and official skill resolution."),
  surface("graph-runtime", "runx-runtime", "fixture-backed", "Covers graph execution, branching, caller handoffs, receipts, and the deterministic decision kernel."),
  surface("receipts", "runx-receipts + runx-runtime + runx-cli", "schema-exact", "Receipt JSON and signature metadata are schema-exact parity surfaces."),
  surface("ledger", "runx-runtime", "schema-exact", "Append-only run state and continuation history must survive cutover."),
  surface("execution-boundary", "runx-contracts + runx-runtime", "schema-exact", "Runtime-observed execution boundaries remain explicit in operator and sealed evidence."),
  surface("harness", "runx-runtime harness via runx-cli", "fixture-backed", "Harness replay mode proves deterministic fixture execution and sealed receipt checks."),
  surface("history", "runx-cli + runx-runtime", "semantic", "Search/filter behavior is command-level parity with normalized output."),
  surface("registry", "runx-cli + runx-runtime registry", "fixture-backed", "Local and hosted registry envelopes are exercised through native registry commands."),
  surface("tool-catalog", "runx-runtime tool catalogs", "fixture-backed", "Catalog discovery, dispatch, and local tool builds use the canonical native or manifest-owned path."),
  surface("mcp", "runx-runtime adapters/mcp", "stubbed", "Protocol behavior uses local servers and deterministic clients."),
  surface("adapter-process-tool", "runx-runtime process tool adapter", "fixture-backed", "Exact process invocation, environment, cwd, supervision, and execution-boundary evidence are parity-critical."),
  surface("adapter-agent", "runx-runtime external agent adapter", "stubbed", "Managed agent calls are represented by local stubs, not live providers."),
  surface("config", "runx-cli", "schema-exact", "RUNX_HOME, encrypted local profiles, and config file behavior are part of CLI parity."),
  surface("public-api", "runx-cli + runx-runtime", "stubbed", "Public API identity and HTTP transport are resolved once and exercised against deterministic local servers."),
  surface("connect", "runx-cli + runx cloud", "stubbed", "The native CLI owns provider-neutral grant lifecycle; governed skills and native provider tools own bounded provider operations."),
  surface("connectors", "runx-cli + runx-runtime kernel", "fixture-backed", "Official connector manifests, instances and readiness are listed and selected natively; import-mcp pins what a server serves."),
  surface("doctor", "runx-cli + runx-runtime doctor", "semantic", "Diagnostics can add ids, but the documented command surface must not disappear."),
  surface("dev", "runx-cli", "fixture-backed", "Development lanes run deterministic or recorded harness fixtures."),
  surface("skill-export", "runx-cli + runx-runtime", "semantic", "Host-agent shims are generated from validated skill packages and delegate back to governed runx skill execution."),
  surface("parser", "runx-parser via runx-cli", "schema-exact", "Native parser evaluation output stays schema-exact."),
  surface("authority", "runx-core/policy", "schema-exact", "Grant, scope, and authority-kind policy remains machine-checkable without OSS brokerage."),
  surface("policy", "runx-core/policy", "schema-exact", "Policy inspection and linting stay machine-checkable before mutation gates run."),
  surface("caller-mediated-resolution", "runx-runtime", "fixture-backed", "Required input, approvals, and agent work keep the same continuation contract."),
  surface("skill-authoring", "runx-runtime + skill-lab", "fixture-backed", "Skill creation uses one digest-bound inspect, plan, bind, validate, harness, and transactional apply lane."),
  surface("workspace-init", "runx-runtime", "semantic", "Deterministic project and global workspace initialization remains separate from skill authoring."),
  surface("official-skills", "runx-cli", "schema-exact", "Prefetch and lockfile behavior stays fixture-backed."),
  surface("list", "runx-cli", "semantic", "Inventory output for tools, skills, graphs, packets, and overlays stays represented."),
  surface("extension-sdk", "packages/extension-sdk", "schema-exact", "External process extension output and manifest validation remain schema-exact."),
];

export interface CliFeatureParityContract {
  readonly commands: readonly CommandMatrixEntry[];
  readonly surfaces: readonly RuntimeSurface[];
  readonly cases: readonly OracleCase[];
}

export function loadCliFeatureParityContract(runx: string): CliFeatureParityContract {
  const commands = bindNativeCommands(readNativeCommandCatalog(runx), commandAnnotations);
  return {
    commands,
    surfaces: bindRuntimeSurfaces(commands, surfaceDefinitions),
    cases: oracleCases(commands),
  };
}

function oracleCases(commands: readonly CommandMatrixEntry[]): readonly OracleCase[] {
  const executableCases = [
    execute(commands, "help.top-level", "cli.help", ["--help"], 0, false, ["Usage:", "runx skill", "runx harness"], []),
    execute(commands, "usage.unsupported", "cli.help", ["not-a-command"], 64, false, [], ["unknown command not-a-command"]),
    execute(commands, "config.list.execute", "config", ["config", "list", "--json"], 0, true, [], []),
    execute(commands, "harness.execute", "harness", ["harness", "fixtures/cli-parity/harness/echo-skill.yaml", "--json"], 0, true, [], []),
    execute(
      commands,
      "history.execute",
      "history",
      ["history", "--json"],
      0,
      true,
      ["\"projector_id\": \"runx-runtime.run-record.v1\"", "\"runs\": []"],
      [],
    ),
    execute(commands, "list.tools.execute", "list", ["list", "tools", "--json"], 0, true, [], []),
    execute(commands, "connector.list.execute", "connector", ["connector", "list", "--json"], 0, true, ["\"github\""], []),
    execute(commands, "connect.execute", "connect", ["connect", "list", "--api-base-url", "http://127.0.0.1:9", "--json"], 1, true, ["not publicly routable"], [], { RUNX_PUBLIC_API_TOKEN: "rxk_fixture" }),
  ];
  const executableCaseIds = new Set(executableCases.map((testCase) => testCase.id));

  return [
    ...executableCases,
    ...commands.flatMap((entry) => entry.cases
      .filter((caseId) => !executableCaseIds.has(caseId))
      .map((caseId) => validate(caseId, entry.name, entry.parity.surfaces))),
  ];
}

function annotation(
  sideEffect: CommandAnnotation["sideEffect"],
  surfaces: readonly string[],
  casesForCommand: readonly string[],
  jsonOutput: CommandAnnotation["jsonOutput"] = "schema-exact",
): CommandAnnotation {
  return { sideEffect, surfaces, cases: casesForCommand, jsonOutput };
}

function readNativeCommandCatalog(runx: string): NativeCommandCatalog {
  const result = spawnSync(runx, ["--help", "--json"], {
    cwd: process.cwd(),
    encoding: "utf8",
    maxBuffer: 1024 * 1024,
  });
  if (result.error) {
    throw result.error;
  }
  if (result.status !== 0) {
    throw new Error(
      `native command catalog failed with exit ${result.status ?? "signal"}: ${(result.stderr ?? "").trim()}`,
    );
  }

  const parsed: unknown = JSON.parse(result.stdout);
  if (!isNativeCommandCatalog(parsed)) {
    throw new Error("runx --help --json returned an invalid runx.cli_command_catalog.v1 payload");
  }
  return parsed;
}

function bindNativeCommands(
  catalog: NativeCommandCatalog,
  annotations: Readonly<Record<string, CommandAnnotation>>,
): readonly CommandMatrixEntry[] {
  const nativeCommands = [catalog.root, ...catalog.commands];
  const nativeNames = new Set<string>();
  for (const command of nativeCommands) {
    if (!nativeNames.add(command.name)) {
      throw new Error(`native command catalog contains duplicate command '${command.name}'`);
    }
  }

  const missingAnnotations = nativeCommands
    .map((command) => command.name)
    .filter((name) => annotations[name] === undefined);
  const unknownAnnotations = Object.keys(annotations).filter((name) => !nativeNames.has(name));
  if (missingAnnotations.length > 0 || unknownAnnotations.length > 0) {
    throw new Error([
      "native command catalog and parity annotations disagree",
      `Missing annotations: ${missingAnnotations.join(", ") || "none"}`,
      `Unknown annotations: ${unknownAnnotations.join(", ") || "none"}`,
    ].join("\n"));
  }

  return nativeCommands.map((command) => {
    const metadata = annotations[command.name];
    if (!metadata) {
      throw new Error(`missing parity annotation for '${command.name}'`);
    }
    return {
      ...command,
      parity: {
        humanOutput: "semantic",
        jsonOutput: metadata.jsonOutput ?? "schema-exact",
        receipt: metadata.surfaces.includes("receipts") ? "schema-exact" : "none",
        sideEffect: metadata.sideEffect,
        surfaces: metadata.surfaces,
      },
      cases: metadata.cases,
    };
  });
}

function isNativeCommandCatalog(value: unknown): value is NativeCommandCatalog {
  if (!isObject(value) || value.schema !== "runx.cli_command_catalog.v1") {
    return false;
  }
  return isNativeCommandSpec(value.root)
    && Array.isArray(value.commands)
    && value.commands.every(isNativeCommandSpec);
}

function isNativeCommandSpec(value: unknown): value is NativeCommandSpec {
  return isObject(value)
    && typeof value.name === "string"
    && isStringArray(value.topLevelUsage)
    && isStringArray(value.usage)
    && isStringArray(value.notes)
    && isStringArray(value.options);
}

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isStringArray(value: unknown): value is string[] {
  return Array.isArray(value) && value.every((entry) => typeof entry === "string");
}

function bindRuntimeSurfaces(
  commands: readonly CommandMatrixEntry[],
  definitions: readonly RuntimeSurfaceDefinition[],
): readonly RuntimeSurface[] {
  const definedIds = new Set(definitions.map((definition) => definition.id));
  if (definedIds.size !== definitions.length) {
    throw new Error("runtime surface definitions contain duplicate ids");
  }
  const unknownIds = new Set(
    commands.flatMap((command) =>
      command.parity.surfaces.filter((surfaceId) => !definedIds.has(surfaceId)),
    ),
  );
  if (unknownIds.size > 0) {
    throw new Error(`command annotations reference unknown runtime surfaces: ${[...unknownIds].join(", ")}`);
  }

  return definitions.map((definition) => ({
    ...definition,
    coveredBy: commands
      .filter((command) => command.parity.surfaces.includes(definition.id))
      .map((command) => command.name),
  }));
}

function surface(
  id: string,
  owner: string,
  parityClass: RuntimeSurfaceDefinition["parityClass"],
  notes: string,
): RuntimeSurfaceDefinition {
  return { id, owner, parityClass, notes };
}

function execute(
  commands: readonly CommandMatrixEntry[],
  id: string,
  commandId: string,
  argv: readonly string[],
  expectedExitCode: number,
  expectJson: boolean,
  stdoutIncludes: readonly string[],
  stderrIncludes: readonly string[],
  env?: Readonly<Record<string, string>>,
): OracleCase {
  const command = commands.find((entry) => entry.name === commandId);
  if (!command) {
    throw new Error(`executable parity case '${id}' references unknown command '${commandId}'`);
  }
  return {
    id,
    commandId,
    mode: "execute",
    argv,
    expectedExitCode,
    expectJson,
    stdoutIncludes,
    stderrIncludes,
    ...(env ? { env } : {}),
    proves: command.parity.surfaces,
  };
}

function validate(id: string, commandId: string, proves: readonly string[]): OracleCase {
  return { id, commandId, mode: "validate", proves };
}
