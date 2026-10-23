import path from "node:path";
import { validateResolutionRequestContract, validateSkillOutputContract, type SkillOutputContract, type WaitedRequestContract } from "@runxhq/contracts";
import { readProjectArtifact } from "./artifacts.js";
import { RunxClientError, type ProcessResult } from "./process.js";
import { readReceiptTree } from "./receipt-tree.js";
import type { SkillRunOutput } from "./types.js";

export function record(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

export function parseJson(result: ProcessResult, exits: readonly number[]): Record<string, unknown> {
  if (result.exitCode === null || !exits.includes(result.exitCode)) {
    const failure = nativeFailure(result);
    throw new RunxClientError("exit", `Runx exited with code ${result.exitCode}: ${failure.message}`, { code: failure.code });
  }
  try {
    const output: unknown = JSON.parse(result.stdout);
    if (!record(output)) throw new Error("Expected a JSON object.");
    return output;
  } catch (error) { throw new RunxClientError("contract", "Runx did not return a JSON object.", { cause: error }); }
}

/** The one value a command that prints only a value printed: its stdout, without the newline it ends with. */
export function parseValue(result: ProcessResult): string {
  if (result.exitCode !== 0) {
    const failure = nativeFailure(result);
    throw new RunxClientError("exit", `Runx exited with code ${result.exitCode}: ${failure.message}`, { code: failure.code });
  }
  return result.stdout.endsWith("\n") ? result.stdout.slice(0, -1) : result.stdout;
}

/** Why runx failed, in its own words: the `--json` failure envelope it wrote, else its stderr. */
function nativeFailure(result: ProcessResult): { message: string; code?: string } {
  try {
    const output: unknown = JSON.parse(result.stdout);
    if (record(output) && record(output.error) && typeof output.error.message === "string") {
      return { message: output.error.message.slice(0, 2048), ...(typeof output.error.code === "string" ? { code: output.error.code } : {}) };
    }
  } catch {
    // Not an envelope: stderr is all runx said.
  }
  return { message: result.stderr.trim().slice(0, 2048) };
}

export async function decodeSkill(result: ProcessResult, config: { projectDir: string; receiptDir: string; maxArtifactBytes: number }): Promise<SkillRunOutput> {
  try {
    const compact = validateSkillOutputContract(parseJson(result, [0, 1, 2]));
    if (compact.status !== "failure" && (compact.schema !== "runx.skill_run.v1" || !compact.outcome)) throw new Error("Native run omitted schema or outcome.");
    if (compact.status === "sealed" && !compact.receipt_id) throw new Error("Sealed native output omitted its receipt.");
    const expectedExit = compact.status === "sealed"
      ? (compact.outcome === "completed" ? 0 : compact.outcome === "deferred" ? 2 : 1)
      : ["needs_agent", "needs_approval", "payment_required"].includes(compact.status) ? 2 : 1;
    if (result.exitCode !== expectedExit) throw new Error("Native exit code disagrees with its execution outcome.");
    let budget = config.maxArtifactBytes;
    const artifact = async (ref: unknown) => {
      if (!record(ref) || typeof ref.bytes !== "number" || ref.bytes > budget) throw new Error("Native artifacts exceed the invocation byte budget.");
      const value = await readProjectArtifact(ref, config.projectDir, budget);
      budget -= ref.bytes;
      return value;
    };
    let output: SkillOutputContract = compact;
    const fullArtifacts = compact.artifacts?.filter((ref) => path.basename(ref.path).startsWith("full-output-")) ?? [];
    if (fullArtifacts.length > 1) throw new Error("Multiple full-output artifacts.");
    if (fullArtifacts[0]) {
      const full = await artifact(fullArtifacts[0]);
      if (!record(full)) throw new Error("Full-output artifact is not an object.");
      for (const key of ["schema", "status", "outcome", "seal_reason", "run_id", "receipt_id", "skill_name", "runner"] as const) {
        if (compact[key] !== undefined && JSON.stringify(full[key]) !== JSON.stringify(compact[key])) throw new Error(`Full-output artifact disagrees on ${key}.`);
      }
      output = validateSkillOutputContract(full);
    } else if (compact.diagnostics_artifact) {
      const diagnostics = await artifact(compact.diagnostics_artifact);
      if (!record(diagnostics) || Object.keys(diagnostics).some((key) => !["context", "trace", "execution"].includes(key))) throw new Error("Invalid native diagnostics artifact.");
      output = { ...compact, ...diagnostics };
    }
    // What the run waits on comes from the compact summaries runx always
    // writes, each with the digest an answer binds to; a full-output copy
    // carries the requests without it.
    let summaries: readonly unknown[] = compact.requests ?? [];
    if (compact.request_set_artifact) {
      if (summaries.length) throw new Error("Native output contains both requests and a spilled request set.");
      const set = await artifact(compact.request_set_artifact);
      if (!Array.isArray(set)) throw new Error("Request-set artifact is not an array.");
      summaries = set;
    }
    const waiting: WaitedRequestContract[] = [];
    for (const summary of summaries) {
      if (!record(summary) || typeof summary.id !== "string" || typeof summary.request_digest !== "string" || !record(summary.artifact) || summary.request_digest !== summary.artifact.digest) {
        throw new Error("A waited request lacks its digest-bound summary.");
      }
      const full = await artifact(summary.artifact);
      if (!record(full) || full.id !== summary.id || full.kind !== summary.kind) throw new Error("Resolution request artifact identity differs from its summary.");
      waiting.push({ address: summary.id, request_digest: summary.request_digest, request: validateResolutionRequestContract(full) });
    }
    if (["needs_agent", "needs_approval"].includes(output.status) && !waiting.length) throw new Error("Pending run has no resolution requests.");
    let value = output.result;
    // A result's own data may hold an `artifact` field. Only the CLI's compact
    // wrapper, exactly `schema` and a project artifact, is hydrated; a
    // full-output artifact contains the original result before compact projection.
    if (!fullArtifacts.length && record(value) && record(value.artifact)
      && value.artifact.schema === "runx.project_artifact.v1"
      && Object.keys(value).every((key) => key === "schema" || key === "artifact")) {
      value = await artifact(value.artifact);
    }
    // The run's signed record is the receipt tree in its receipt directory.
    const receiptTree = output.receipt_id === undefined
      ? undefined
      : await readReceiptTree(output.receipt_id, config.receiptDir, budget);
    return { ...output, ...(value === undefined ? {} : { result: value }), requests: waiting, ...(receiptTree ? { receiptTree } : {}) };
  } catch (error) {
    if (error instanceof RunxClientError) throw error;
    throw new RunxClientError("contract", `Invalid native skill output: ${error instanceof Error ? error.message : String(error)}`, { cause: error });
  }
}
