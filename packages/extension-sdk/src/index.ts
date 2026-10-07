export const extensionSdkPackage = "@runxhq/extension-sdk";

export { defineTool, failure } from "./tool.js";
export type {
  DefinedTool,
  ToolDefinition,
  ToolFailure,
  ToolRunContext,
} from "./tool.js";
export { firstNonEmptyString, isRecord, prune } from "./values.js";
