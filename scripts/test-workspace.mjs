import { spawn, spawnSync } from "node:child_process";
import { cargo, prebuiltBinEnv, rustBuildArgs, workspaceRoot } from "./rust-build.mjs";

const pnpm = process.platform === "win32" ? "pnpm.cmd" : "pnpm";
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
        ...prebuiltBinEnv,
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

// The shared build: after verify:fast it finds nothing to rebuild.
function ensureRustKernelBin() {
  const result = spawnSync(cargo, rustBuildArgs, { cwd: workspaceRoot, stdio: "inherit", env: process.env });
  if (result.status !== 0) {
    process.exit(result.status ?? 1);
  }
}
