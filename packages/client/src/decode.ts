import path from "node:path";
import { validateResolutionRequestContract, validateSkillOutputContract, type SkillOutputContract } from "@runxhq/contracts";
import { readProjectArtifact } from "./artifacts.js";
import { RunxClientError, type ProcessResult } from "./process.js";
import { readReceiptTree } from "./receipt-tree.js";
import type { SkillRunOutput } from "./types.js";

export function record(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

export function parseJson(result: ProcessResult, exits: readonly number[]): Record<string, unknown> {
  if (result.exitCode === null || !exits.includes(result.exitCode)) {
    throw new RunxClientError("exit", `Runx exited with code ${result.exitCode}: ${failureDetail(result)}`);
  }
  try {
    const output: unknown = JSON.parse(result.stdout);
    if (!record(output)) throw new Error("Expected a JSON object.");
    return output;
  } catch (error) { throw new RunxClientError("contract", "Runx did not return a JSON object.", { cause: error }); }
}

/** Why runx failed, in its own words: the `--json` failure envelope it wrote, else its stderr. */
function failureDetail(result: ProcessResult): string {
  try {
    const output: unknown = JSON.parse(result.stdout);
    if (record(output) && record(output.error) && typeof output.error.message === "string") return output.error.message.slice(0, 2048);
  } catch {
    // Not an envelope: stderr is all runx said.
  }
  return result.stderr.trim().slice(0, 2048);
}

export async function decodeSkill(result: ProcessResult, config: { projectDir: string; receiptDir: string; maxArtifactBytes: number }): Promise<SkillRunOutput> {
  try {
    const compact = validateSkillOutputContract(parseJson(result, [0, 1, 2]));
    if (compact.status !== "failure" && (compact.schema !== "runx.skill_run.v1" || !compact.outcome)) throw new Error("Native run omitted schema or outcome.");
    if (compact.status === "sealed" && (!compact.receipt_id || !record(compact.closure))) throw new Error("Sealed native output omitted its receipt or closure.");
    const expectedExit = compact.status === "sealed"
      ? (record(compact.closure) && compact.closure.disposition === "closed" ? 0 : compact.outcome === "deferred" ? 2 : 1)
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
    const fullRefs = compact.artifact_refs?.filter((ref) => path.basename(ref.path).startsWith("full-output-")) ?? [];
    if (fullRefs.length > 1) throw new Error("Multiple full-output artifacts.");
    if (fullRefs[0]) {
      const full = await artifact(fullRefs[0]);
      if (!record(full)) throw new Error("Full-output artifact is not an object.");
      for (const key of ["schema", "run_id", "receipt_id", "skill_name", "runner"]) {
        if (compact[key] !== undefined && JSON.stringify(full[key]) !== JSON.stringify(compact[key])) throw new Error(`Full-output artifact disagrees on ${key}.`);
      }
      // The full artifact precedes the CLI's outcome/pending-status projection.
      const nativeStatus = full.status === "needs_agent" && compact.status === "needs_approval" ? "needs_approval" : full.status;
      if (nativeStatus !== compact.status) throw new Error("Full-output artifact disagrees on status.");
      if (record(compact.closure) && (!record(full.closure) || full.closure.disposition !== compact.closure.disposition || full.closure.reason !== compact.closure.reason)) throw new Error("Full-output artifact disagrees on closure.");
      output = validateSkillOutputContract({ ...full, status: compact.status, outcome: compact.outcome });
    } else if (compact.diagnostics_ref) {
      const diagnostics = await artifact(compact.diagnostics_ref);
      if (!record(diagnostics) || Object.keys(diagnostics).some((key) => !["context", "trace", "execution"].includes(key))) throw new Error("Invalid native diagnostics artifact.");
      output = { ...compact, ...diagnostics };
    }
    let requests: readonly unknown[] = output.requests ?? [];
    if (output.request_set_ref) {
      if (requests.length) throw new Error("Native output contains both requests and a spilled request set.");
      const set = await artifact(output.request_set_ref);
      if (!Array.isArray(set)) throw new Error("Request-set artifact is not an array.");
      requests = set;
    }
    const hydrated = [];
    for (const request of requests) {
      if (record(request) && request.artifact_ref !== undefined) {
        const full = await artifact(request.artifact_ref);
        if (!record(request.artifact_ref) || request.request_digest !== request.artifact_ref.digest || !record(full) || full.id !== request.id || full.kind !== request.kind) throw new Error("Resolution request artifact identity differs from its summary.");
        hydrated.push(validateResolutionRequestContract(full));
      } else hydrated.push(validateResolutionRequestContract(request));
    }
    if (["needs_agent", "needs_approval"].includes(output.status) && !hydrated.length) throw new Error("Pending run has no resolution requests.");
    let value = output.result;
    // artifact_ref is also ordinary provider/skill data. Only the CLI's
    // compact wrapper can be hydrated; a full-output artifact contains the
    // original result before compact projection.
    if (!fullRefs.length && record(value) && record(value.artifact_ref)
      && value.artifact_ref.schema === "runx.project_artifact_ref.v1"
      && Object.keys(value).every((key) => key === "schema" || key === "artifact_ref")) {
      value = await artifact(value.artifact_ref);
    }
    // The run's signed record is the receipt tree in its receipt directory.
    const receiptTree = output.receipt_id === undefined
      ? undefined
      : await readReceiptTree(output.receipt_id, config.receiptDir, budget);
    return { ...output, ...(value === undefined ? {} : { result: value }), requests: hydrated, ...(receiptTree ? { receiptTree } : {}) };
  } catch (error) {
    if (error instanceof RunxClientError) throw error;
    throw new RunxClientError("contract", `Invalid native skill output: ${error instanceof Error ? error.message : String(error)}`, { cause: error });
  }
}
