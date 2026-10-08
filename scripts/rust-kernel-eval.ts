import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const workspaceRoot = path.resolve(fileURLToPath(new URL("..", import.meta.url)));
const defaultRunxBinary = path.join(
  workspaceRoot,
  "crates",
  "target",
  "debug",
  process.platform === "win32" ? "runx.exe" : "runx",
);

export type GraphStatus = "pending" | "running" | "succeeded" | "failed";
export type GraphStepStatus = "pending" | "running" | "succeeded" | "failed";
export type StepStatus = "pending" | "admitted" | "running" | "succeeded" | "failed";

export interface SingleStepState {
  readonly stepId: string;
  readonly status: StepStatus;
  readonly startedAt?: string;
  readonly completedAt?: string;
  readonly error?: string;
}

export interface StepAdmissionWitness {
  readonly stepId: string;
  readonly receiptId: string;
  readonly authority?: unknown;
}

export type SingleStepEvent =
  | { readonly type: "admit" }
  | { readonly type: "start"; readonly at: string }
  | { readonly type: "succeed"; readonly at: string; readonly admissionWitness: StepAdmissionWitness }
  | { readonly type: "fail"; readonly at: string; readonly error: string };

export interface SequentialGraphStepDefinition {
  readonly id: string;
  readonly contextFrom?: readonly string[];
}

export interface SequentialGraphStepState {
  readonly stepId: string;
  readonly status: GraphStepStatus;
  readonly attempts: number;
  readonly startedAt?: string;
  readonly completedAt?: string;
  readonly receiptId?: string;
  readonly outputs?: Readonly<Record<string, unknown>>;
  readonly error?: string;
}

export interface SequentialGraphState {
  readonly graphId: string;
  readonly status: GraphStatus;
  readonly steps: readonly SequentialGraphStepState[];
}

export type SequentialGraphEvent =
  | { readonly type: "start_step"; readonly stepId: string; readonly at: string }
  | {
      readonly type: "step_succeeded";
      readonly stepId: string;
      readonly at: string;
      readonly receiptId: string;
      readonly admissionWitness: StepAdmissionWitness;
      readonly outputs?: Readonly<Record<string, unknown>>;
    }
  | { readonly type: "step_failed"; readonly stepId: string; readonly at: string; readonly error: string }
  | { readonly type: "complete" }
  | { readonly type: "fail_graph"; readonly error: string };

export interface RustKernelEvalOptions {
  readonly command?: string;
  readonly argsPrefix?: readonly string[];
  readonly cwd?: string;
  readonly env?: NodeJS.ProcessEnv;
  readonly timeoutMs?: number;
}

interface KernelSuccessEnvelope {
  readonly status: "success";
  readonly result: {
    readonly kind: "output";
    readonly value: unknown;
  };
}

interface KernelErrorEnvelope {
  readonly status: "error";
  readonly code: string;
  readonly message: string;
}

export class RustKernelEvalError extends Error {
  readonly code?: string;

  constructor(message: string, code?: string, options?: ErrorOptions) {
    super(message, options);
    this.name = "RustKernelEvalError";
    this.code = code;
  }
}

export function evaluateRustKernelInputSync(
  input: unknown,
  options: RustKernelEvalOptions = {},
): unknown {
  const command = resolveRustKernelCommand(options);
  const result = spawnSync(command, [...(options.argsPrefix ?? []), "kernel", "eval", "--input", "-", "--json"], {
    cwd: options.cwd ?? workspaceRoot,
    encoding: "utf8",
    env: {
      ...process.env,
      ...(options.env ?? {}),
      NO_COLOR: "1",
      RUNX_CWD: options.cwd ?? workspaceRoot,
      RUNX_RUST_CLI: "1",
    },
    input: JSON.stringify(input),
    maxBuffer: 8 * 1024 * 1024,
    timeout: options.timeoutMs ?? 10_000,
  });

  if (result.error) {
    throw new RustKernelEvalError(`Failed to run Rust kernel eval command '${command}': ${result.error.message}`, undefined, {
      cause: result.error,
    });
  }

  const stdout = result.stdout ?? "";
  const stderr = result.stderr ?? "";
  const parsed = parseKernelEnvelope(stdout);
  if (result.status !== 0) {
    if (isKernelErrorEnvelope(parsed)) {
      throw new RustKernelEvalError(parsed.message, parsed.code);
    }
    throw new RustKernelEvalError(
      `Rust kernel eval failed with exit ${result.status}: ${firstNonEmpty(stderr, stdout, "no output")}`,
    );
  }

  if (!isKernelSuccessEnvelope(parsed)) {
    throw new RustKernelEvalError("Rust kernel eval returned an invalid success envelope.");
  }
  return parsed.result.value;
}

function resolveRustKernelCommand(options: RustKernelEvalOptions): string {
  const command = options.command
    ?? options.env?.RUNX_RUST_CLI_BIN
    ?? process.env.RUNX_RUST_CLI_BIN;
  if (command) {
    return command;
  }
  if (existsSync(defaultRunxBinary)) {
    return defaultRunxBinary;
  }
  throw new RustKernelEvalError(
    `Rust kernel eval requires RUNX_RUST_CLI_BIN or a built CLI at ${path.relative(workspaceRoot, defaultRunxBinary)}.`,
  );
}

function parseKernelEnvelope(stdout: string): unknown {
  try {
    return JSON.parse(stdout);
  } catch (error) {
    throw new RustKernelEvalError(`Rust kernel eval returned invalid JSON: ${errorMessage(error)}`, undefined, {
      cause: error,
    });
  }
}

function isKernelSuccessEnvelope(value: unknown): value is KernelSuccessEnvelope {
  return isRecord(value)
    && value.status === "success"
    && isRecord(value.result)
    && value.result.kind === "output";
}

function isKernelErrorEnvelope(value: unknown): value is KernelErrorEnvelope {
  return isRecord(value)
    && value.status === "error"
    && typeof value.code === "string"
    && typeof value.message === "string";
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function firstNonEmpty(...values: readonly string[]): string {
  return values.map((value) => value.trim()).find((value) => value.length > 0) ?? "";
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
