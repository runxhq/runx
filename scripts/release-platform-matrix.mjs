import path from "node:path";
import { fileURLToPath } from "node:url";

import { loadRustCliPlatforms } from "./rust-cli-topology.mjs";

const workspaceRoot = path.resolve(fileURLToPath(new URL("..", import.meta.url)));
const include = loadRustCliPlatforms(workspaceRoot).map((platform) => {
  return {
    platform: platform.key,
    runner: platform.host,
    target: platform.rustTarget,
    ext: platform.archiveExtension,
    binary: platform.binaryName,
    worker: platform.workerName,
  };
});

process.stdout.write(`${JSON.stringify({ include })}\n`);
