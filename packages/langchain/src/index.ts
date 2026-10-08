import { tool, type StructuredToolInterface } from "@langchain/core/tools";
import type { RunxClient, SkillRunOptions, SkillRunOutput } from "@runxhq/client";

export const langchainPackage = "@runxhq/langchain";

export interface RunxLangChainToolOptions {
  readonly name: string;
  readonly description: string;
  readonly schema: object;
  readonly client: Pick<RunxClient, "skills">;
  readonly runOptions: Omit<SkillRunOptions, "inputs">;
  readonly mapInput?: (input: unknown) => Readonly<Record<string, unknown>>;
  readonly formatOutput?: (result: SkillRunOutput) => unknown;
}

/** A finite tool invocation. The host owns pending resolution and continuation. */
export function createRunxLangChainTool(options: RunxLangChainToolOptions): StructuredToolInterface {
  return tool(async (input) => {
    const result = await options.client.skills.run({
      ...options.runOptions,
      inputs: options.mapInput ? options.mapInput(input) : toInputRecord(input),
    });
    if (result.status !== "sealed" || result.outcome !== "completed") {
      throw new Error(`runx workflow '${options.name}' returned ${result.status} (${result.outcome ?? "failed"}); the host must handle this outcome.`);
    }
    const output = options.formatOutput?.(result) ?? result.result ?? result;
    return typeof output === "string" ? output : JSON.stringify(output);
  }, {
    name: options.name,
    description: options.description,
    schema: options.schema as never,
  });
}

function toInputRecord(input: unknown): Readonly<Record<string, unknown>> {
  if (typeof input === "object" && input !== null && !Array.isArray(input)) return input as Readonly<Record<string, unknown>>;
  return typeof input === "string" ? { input } : { value: input };
}
