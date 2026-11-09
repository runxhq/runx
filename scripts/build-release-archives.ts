import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { copyFileSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

// Packages a built native binary into the raw release archive the GitHub
// Release hub serves: runx-<version>-<target>.(tar.gz|zip) plus a .sha256.
// Homebrew, Scoop, winget, AUR and direct downloads all consume these by URL +
// checksum, so this is the single artifact every non-npm channel points at.

const workspaceRoot = path.resolve(fileURLToPath(new URL("..", import.meta.url)));

interface Options {
  readonly binary: string;
  readonly worker: string;
  readonly target: string;
  readonly version: string;
  readonly outDir: string;
}

const options = parseArgs(process.argv.slice(2));
const isWindows = options.target.includes("windows");
const binaryName = isWindows ? "runx.exe" : "runx";
const workerName = isWindows ? "runx-js-worker.exe" : "runx-js-worker";
const stem = `runx-${options.version}-${options.target}`;
const archiveName = isWindows ? `${stem}.zip` : `${stem}.tar.gz`;

const outDir = path.resolve(workspaceRoot, options.outDir);
const stageDir = path.join(outDir, stem);
rmSync(stageDir, { recursive: true, force: true });
mkdirSync(stageDir, { recursive: true });

copyFileSync(path.resolve(workspaceRoot, options.binary), path.join(stageDir, binaryName));
copyFileSync(path.resolve(workspaceRoot, options.worker), path.join(stageDir, workerName));
for (const doc of ["LICENSE", "README.md"]) {
  const source = path.join(workspaceRoot, "packages", "cli", doc);
  try {
    copyFileSync(source, path.join(stageDir, doc));
  } catch {
    // README/LICENSE are best-effort; the binary is the required payload.
  }
}

const archivePath = path.join(outDir, archiveName);
if (isWindows) {
  // GitHub windows-latest methods ship 7-Zip preinstalled but not `zip`.
  // Force the zip container with -tzip and run quietly with -bso0 -bsp0.
  execFileSync("7z", ["a", "-tzip", "-bso0", "-bsp0", archivePath, stem], {
    cwd: outDir,
  });
} else {
  execFileSync("tar", ["-czf", archivePath, "-C", outDir, stem]);
}

const sha256 = createHash("sha256").update(readFileSync(archivePath)).digest("hex");
writeFileSync(`${archivePath}.sha256`, `${sha256}  ${archiveName}\n`);
rmSync(stageDir, { recursive: true, force: true });

console.log(JSON.stringify({
  status: "archived",
  target: options.target,
  archive: archiveName,
  sha256,
}, null, 2));

function parseArgs(argv: readonly string[]): Options {
  let binary = "";
  let worker = "";
  let target = "";
  let version = "";
  let outDir = "dist/archives";
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--binary") { binary = argv[index + 1] ?? ""; index += 1; continue; }
    if (arg === "--worker") { worker = argv[index + 1] ?? ""; index += 1; continue; }
    if (arg === "--target") { target = argv[index + 1] ?? ""; index += 1; continue; }
    if (arg === "--version") { version = argv[index + 1] ?? ""; index += 1; continue; }
    if (arg === "--out-dir") { outDir = argv[index + 1] ?? ""; index += 1; continue; }
    throw new Error(`unknown argument: ${arg}`);
  }
  if (!binary) throw new Error("--binary requires a path");
  if (!worker) throw new Error("--worker requires a path");
  if (!target) throw new Error("--target requires a rust target triple");
  if (!version) throw new Error("--version requires a value");
  return { binary, worker, target, version, outDir };
}
