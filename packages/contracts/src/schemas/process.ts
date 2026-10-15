import {
  type DeepReadonly,
  generatedSchema,
  validateContractSchema,
} from "../internal.js";

/** A terminal's size, in character cells. */
export type TerminalSizeContract = DeepReadonly<{ rows: number; cols: number }>;

export type ProcessStateContract = "starting" | "running" | "exited";

export type ProcessExitReasonContract = "exited" | "stopped" | "timed_out" | "failed";

/** How a process ended; `message` says why one failed. */
export type ProcessExitContract = DeepReadonly<{
  reason: ProcessExitReasonContract;
  code?: number;
  signal?: number;
  message?: string;
  at: string;
}>;

/**
 * A process runx supervises apart from any driver (`runx.process.v1`):
 * started by one call, read, written and stopped by later ones until it
 * exits. It belongs to the actor whose run started it.
 */
export type RunxProcessContract = DeepReadonly<{
  process_id: string;
  command: string;
  args?: readonly string[];
  cwd: string;
  tty?: TerminalSizeContract;
  max_seconds: number;
  actor?: string;
  created_at: string;
  state: ProcessStateContract;
  pid?: number;
  started_at?: string;
  exit?: ProcessExitContract;
}>;

export type ProcessStreamContract = "stdout" | "stderr" | "terminal";

/**
 * What a process wrote after a cursor (`runx.process.output.v1`). `next` is
 * the cursor the following read passes as `after`; `dropped` counts the log
 * bytes before `from` it no longer keeps.
 */
export type ProcessOutputContract = DeepReadonly<{
  process_id: string;
  state: ProcessStateContract;
  exit?: ProcessExitContract;
  from: number;
  next: number;
  dropped: number;
  chunks: readonly { stream: ProcessStreamContract; text: string }[];
}>;

/** The processes in a project, most recently changed first (`runx.process.list.v1`). */
export type ProcessListContract = DeepReadonly<{ processes: readonly RunxProcessContract[] }>;

export const processSchema = generatedSchema<RunxProcessContract>("process.schema.json");
export const processOutputSchema = generatedSchema<ProcessOutputContract>("process-output.schema.json");
export const processListSchema = generatedSchema<ProcessListContract>("process-list.schema.json");

export function validateProcessContract(value: unknown, label = "process"): RunxProcessContract {
  return validateContractSchema(processSchema, value, label);
}

export function validateProcessOutputContract(value: unknown, label = "process_output"): ProcessOutputContract {
  return validateContractSchema(processOutputSchema, value, label);
}

export function validateProcessListContract(value: unknown, label = "process_list"): ProcessListContract {
  return validateContractSchema(processListSchema, value, label);
}
