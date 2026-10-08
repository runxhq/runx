import type { SkillOutputContract } from "@runxhq/contracts";

export function summarizeSkillOutput(result: SkillOutputContract): string {
  const name = typeof result.skill_name === "string" ? result.skill_name : "Runx skill";
  if (result.status === "needs_agent" || result.status === "needs_approval") {
    return `${name} needs ${result.status === "needs_approval" ? "approval" : "agent input"} at ${result.run_id ?? "unknown run"}.`;
  }
  if (result.status === "payment_required") return `${name} requires payment at ${result.run_id ?? "unknown run"}.`;
  if (result.status === "policy_denied") return `${name} was denied by policy.`;
  const outcome = result.outcome ?? "failed";
  return `${name} ${outcome}.${result.receipt_id ? ` Inspect receipt ${result.receipt_id}.` : ""}`;
}

export function toOpenAiResponse(result: SkillOutputContract) {
  return { role: "tool" as const, content: [{ type: "text" as const, text: summarizeSkillOutput(result) }], structuredContent: { runx: result } };
}
export function toAnthropicResponse(result: SkillOutputContract) {
  return { content: [{ type: "text" as const, text: summarizeSkillOutput(result) }], metadata: { runx: result } };
}
export function toVercelAiResponse(result: SkillOutputContract) {
  return { messages: [{ role: "assistant" as const, content: summarizeSkillOutput(result) }], data: { runx: result } };
}
export function toLangChainResponse(result: SkillOutputContract) {
  return { content: summarizeSkillOutput(result), additional_kwargs: { runx: result } };
}
export function toCrewAiResponse(result: SkillOutputContract) {
  return { raw: summarizeSkillOutput(result), json_dict: { runx: result } };
}
