import { spawn, spawnSync } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";

const workspaceRoot = path.resolve(fileURLToPath(new URL("..", import.meta.url)));
const pnpm = process.platform === "win32" ? "pnpm.cmd" : "pnpm";
const cargo = process.platform === "win32" ? "cargo.exe" : "cargo";
const rustKernelBin = path.join(
  workspaceRoot,
  "crates",
  "target",
  "debug",
  process.platform === "win32" ? "runx.exe" : "runx",
);
const forwardedArgs = process.argv.slice(2).filter((arg) => arg !== "--");

ensureRustKernelBin();

if (forwardedArgs.length > 0) {
  await runVitest(["run", ...forwardedArgs]);
} else {
  await runVitest(["run", "--exclude", "tests/cli-package.test.ts"]);
  await runVitest(["run", "tests/cli-package.test.ts"], { RUNX_VITEST_BATCH: "cli-package" });
}

async function runVitest(args, extraEnv = {}) {
  await new Promise((resolve, reject) => {
    const child = spawn(pnpm, ["exec", "vitest", ...args], {
      cwd: workspaceRoot,
      stdio: "inherit",
      env: {
        ...process.env,
        // Point every subprocess-backed suite at the single prebuilt binary so the
        // kernel-parity / parser / CLI eval paths never cold-start a debug binary
        // under parallel load.
        RUNX_RUST_CLI_BIN: rustKernelBin,
        ...extraEnv,
      },
    });
    child.on("error", reject);
    child.on("exit", (code) => {
      if (code === 0) {
        resolve();
      } else {
        reject(new Error(`vitest ${args.join(" ")} exited with ${code}`));
      }
    });
  });
}

function ensureRustKernelBin() {
  const result = spawnSync(
    cargo,
    [
      "build",
      "--quiet",
      "--manifest-path",
      "crates/Cargo.toml",
      "-p",
      "runx-cli",
      "-p",
      "runx-js-worker",
      "-p",
      "runx-core",
      "--bins",
      "--example",
      "kernel_eval_batch",
    ],
    {
      cwd: workspaceRoot,
      stdio: "inherit",
      env: process.env,
    },
  );
  if (result.status !== 0) {
    process.exit(result.status ?? 1);
  }
}
