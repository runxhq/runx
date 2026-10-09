import { spawnSync } from "node:child_process";
import path from "node:path";

const workspaceRoot = process.cwd();
const snapshotPath = path.join(
  workspaceRoot,
  "fixtures",
  "tool-catalogs",
  "native-capabilities.snapshot.json",
);
const check = process.argv.includes("--check");
const cargo = process.platform === "win32" ? "cargo.exe" : "cargo";
const prebuilt = process.env.RUNX_CAPABILITY_SNAPSHOT_BIN;
const command = prebuilt || cargo;
const args = prebuilt
  ? ["--out", snapshotPath]
  : [
      "run",
      "--quiet",
      "--manifest-path",
      path.join(workspaceRoot, "crates", "Cargo.toml"),
      "-p",
      "runx-runtime",
      "--bin",
      "runx-native-capability-snapshot",
      "--",
      "--out",
      snapshotPath,
    ];

if (check) args.push("--check");

const result = spawnSync(command, args, {
  cwd: workspaceRoot,
  env: process.env,
  stdio: "inherit",
});
if (result.error) throw result.error;
process.exit(result.status ?? 1);
