import { spawnSync } from "node:child_process";
import path from "node:path";

import { cargo, rustBuildArgs, workspaceRoot } from "../rust-build.mjs";

/** The variable naming each example's prebuilt binary (rust-build.mjs). */
const PREBUILT = {
  parser_eval: "RUNX_PARSER_EVAL_BIN",
  kernel_eval_batch: "RUNX_KERNEL_EVAL_BIN",
};

let built = false;

/**
 * A development-only example binary of a workspace crate, the way the
 * repository's scripts reach build tools that never ship: in the Cargo target
 * directory, from the one Rust build the checks and test wrappers share
 * (rust-build.mjs).
 *
 * When the caller's environment names the prebuilt binary, as verify-fast and
 * the test wrappers do, that's the binary, and cargo never runs. A cargo build
 * replaces each example's file in the target directory even when nothing needs
 * building, so a test that ran one while another test was starting the binary
 * would find it missing. Only a caller with no prebuilt binary, such as a script
 * run by hand, builds it, once per process.
 */
export function rustExampleBinary(example, env = process.env) {
  const prebuilt = PREBUILT[example] && env[PREBUILT[example]];
  if (prebuilt) {
    return prebuilt;
  }
  const targetDir = path.resolve(workspaceRoot, env.CARGO_TARGET_DIR ?? path.join("crates", "target"));
  const binary = path.join(
    targetDir,
    "debug",
    "examples",
    process.platform === "win32" ? `${example}.exe` : example,
  );
  if (!built) {
    const result = spawnSync(cargo, rustBuildArgs, { cwd: workspaceRoot, env, stdio: "inherit" });
    if (result.status !== 0) {
      throw new Error("the shared Rust build failed (scripts/rust-build.mjs)");
    }
    built = true;
  }
  return binary;
}
