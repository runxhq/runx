import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const workspaceRoot = path.resolve(fileURLToPath(new URL("../..", import.meta.url)));
const built = new Set();

/**
 * A development-only example binary of a workspace crate, the way the
 * repository's scripts reach build tools that never ship: in the Cargo target
 * directory, built once when it is missing.
 */
export function rustExampleBinary(crate, example, env = process.env) {
  const targetDir = path.resolve(workspaceRoot, env.CARGO_TARGET_DIR ?? path.join("crates", "target"));
  const binary = path.join(
    targetDir,
    "debug",
    "examples",
    process.platform === "win32" ? `${example}.exe` : example,
  );
  if (!existsSync(binary) && !built.has(binary)) {
    const result = spawnSync(
      process.platform === "win32" ? "cargo.exe" : "cargo",
      ["build", "--quiet", "--manifest-path", path.join(workspaceRoot, "crates", "Cargo.toml"), "-p", crate, "--example", example],
      { cwd: workspaceRoot, env, stdio: "inherit" },
    );
    if (result.status !== 0) {
      throw new Error(`failed to build the ${example} example of ${crate}`);
    }
    built.add(binary);
  }
  return binary;
}
