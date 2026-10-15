import {
  type DeepReadonly,
  generatedSchema,
  validateContractSchema,
} from "../internal.js";
import type {
  AgentActInvocationContract,
  ApprovalGateContract,
  QuestionContract,
} from "./agent-act.js";

export const resolutionRequestSchema = generatedSchema("resolution-request.schema.json");
export type InputResolutionRequestContract = DeepReadonly<{
  id: string;
  kind: "input";
  questions: readonly QuestionContract[];
}>;
export type ApprovalResolutionRequestContract = DeepReadonly<{
  id: string;
  kind: "approval";
  gate: ApprovalGateContract;
}>;
export type AgentActResolutionRequestContract = DeepReadonly<{
  id: string;
  kind: "agent_act";
  invocation: AgentActInvocationContract;
}>;
/**
 * A step or call waiting to run again. A bounded retry (`retry_after_seconds`
 * above zero) is runx's own: it runs after the delay on the next resume. A
 * claimed call whose effect runx could not confirm (`retry_after_seconds`
 * zero) runs again only on the person's `{"retry": true}` under `approvals`,
 * and only when `resend` is not `unsafe`; a close ends the run with the
 * effect still unconfirmed.
 */
export type RetryResolutionRequestContract = DeepReadonly<{
  id: string;
  kind: "retry";
  reason: string;
  retry_after_seconds: number;
  /** Absent when safe: sending it again repeats nothing. */
  resend?: "safe" | "unsafe";
}>;
export type ResolutionRequestContract =
  | InputResolutionRequestContract
  | ApprovalResolutionRequestContract
  | AgentActResolutionRequestContract
  | RetryResolutionRequestContract;

export const resolutionResponseSchema = generatedSchema("resolution-response.schema.json");
export type ResolutionResponseContract = DeepReadonly<{
  actor: "human" | "agent";
  payload: unknown;
}>;

export const actResultEnvelopeSchema = generatedSchema("act-result.schema.json");
export type ActResultTerminalStatusContract = "sealed" | "failure";
export type ActResultSignalContract =
  | "SIGABRT"
  | "SIGALRM"
  | "SIGBUS"
  | "SIGCHLD"
  | "SIGCONT"
  | "SIGFPE"
  | "SIGHUP"
  | "SIGILL"
  | "SIGINT"
  | "SIGIO"
  | "SIGIOT"
  | "SIGKILL"
  | "SIGPIPE"
  | "SIGPOLL"
  | "SIGPROF"
  | "SIGPWR"
  | "SIGQUIT"
  | "SIGSEGV"
  | "SIGSTKFLT"
  | "SIGSTOP"
  | "SIGSYS"
  | "SIGTERM"
  | "SIGTRAP"
  | "SIGTSTP"
  | "SIGTTIN"
  | "SIGTTOU"
  | "SIGUNUSED"
  | "SIGURG"
  | "SIGUSR1"
  | "SIGUSR2"
  | "SIGVTALRM"
  | "SIGWINCH"
  | "SIGXCPU"
  | "SIGXFSZ"
  | "SIGBREAK"
  | "SIGLOST"
  | "SIGINFO";
export type ActResultTerminalEnvelopeContract = DeepReadonly<{
  status: ActResultTerminalStatusContract;
  stdout: string;
  stderr: string;
  exitCode: number | null;
  signal: ActResultSignalContract | null;
  durationMs: number;
  errorMessage?: string;
  metadata?: Readonly<Record<string, unknown>>;
}>;
export type ActResultNeedsAgentEnvelopeContract = DeepReadonly<{
  status: "needs_agent";
  stdout: string;
  stderr: string;
  exitCode: null;
  signal: null;
  durationMs: number;
  request: ResolutionRequestContract;
  errorMessage?: string;
  metadata?: Readonly<Record<string, unknown>>;
}>;
export type ActResultEnvelopeContract =
  | ActResultTerminalEnvelopeContract
  | ActResultNeedsAgentEnvelopeContract;

export function validateResolutionRequestContract(
  value: unknown,
  label = "resolution_request",
): ResolutionRequestContract {
  return validateContractSchema(resolutionRequestSchema, value, label) as ResolutionRequestContract;
}

export function validateResolutionResponseContract(
  value: unknown,
  label = "resolution_response",
): ResolutionResponseContract {
  return validateContractSchema(resolutionResponseSchema, value, label) as ResolutionResponseContract;
}

export function validateActResultEnvelopeContract(
  value: unknown,
  label = "act_result",
): ActResultEnvelopeContract {
  return validateContractSchema(actResultEnvelopeSchema, value, label) as ActResultEnvelopeContract;
}
