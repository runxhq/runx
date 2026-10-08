import { resolveRunxBinary } from "./runx-binary.js";
export const runxBinary = resolveRunxBinary();
export function ensureRunxBinary(): void { resolveRunxBinary(); }
export function kernelTestEnv(extra: NodeJS.ProcessEnv = {}): NodeJS.ProcessEnv {
  return { ...process.env, RUNX_CWD: process.cwd(), RUNX_RUST_CLI_BIN: runxBinary, ...extra };
}
