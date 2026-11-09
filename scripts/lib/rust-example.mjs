import { spawnSync } from "node:child_process";
import path from "node:path";

import { cargo, rustBuildArgs, workspaceRoot } from "../rust-build.mjs";

let built = false;

/**
 * A development-only example binary of a workspace crate, the way the
 * repository's scripts reach build tools that never ship: in the Cargo target
 * directory, from the one Rust build the checks and test wrappers share
 * (rust-build.mjs), run the first time a process asks. A narrower build would
 * unify runx-runtime's features differently and replace the binary while
 * another process starts it, so this never runs one. Cargo does nothing when
 * that build is fresh, so a target directory shared with another tree never
 * runs a stale binary.
 */
export function rustExampleBinary(example, env = process.env) {
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
