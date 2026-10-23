import { createHash } from "node:crypto";
import { constants } from "node:fs";
import { lstat, open, realpath } from "node:fs/promises";
import path from "node:path";
import { validateProjectArtifactContract } from "@runxhq/contracts";
import { RunxClientError } from "./process.js";

export async function readBoundedJson(file: string, root: string, maximum: number): Promise<{ bytes: Buffer; value: unknown }> {
  const canonicalRoot = await realpath(root);
  const canonicalFile = await realpath(file);
  const relative = path.relative(canonicalRoot, canonicalFile);
  if (!relative || relative === ".." || relative.startsWith(`..${path.sep}`) || path.isAbsolute(relative)) {
    throw new RunxClientError("artifact", "Native artifact is outside its configured root.");
  }
  // Open the original path so a final-component symlink cannot be accepted
  // merely because realpath resolved it into the permitted root.
  const expected = await lstat(canonicalFile);
  const handle = await open(file, constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK);
  try {
    const stat = await handle.stat();
    if (stat.dev !== expected.dev || stat.ino !== expected.ino) throw new RunxClientError("artifact", "Native artifact changed identity before opening.");
    if (!stat.isFile() || stat.size <= 0 || stat.size > maximum) {
      throw new RunxClientError("artifact", `Native artifact must be a regular file of at most ${maximum} bytes.`);
    }
    // Bounded reads also handle files growing after stat.
    const buffer = Buffer.alloc(Math.min(stat.size + 1, maximum + 1));
    let bytes = 0;
    while (bytes < buffer.length) {
      const read = await handle.read(buffer, bytes, buffer.length - bytes, bytes);
      if (read.bytesRead === 0) break;
      bytes += read.bytesRead;
    }
    if (bytes > maximum || bytes !== stat.size) throw new RunxClientError("artifact", "Native artifact changed size while reading.");
    return { bytes: buffer.subarray(0, bytes), value: parseExactJson(buffer.subarray(0, bytes).toString("utf8")) };
  } finally { await handle.close(); }
}

// JSON.parse rounds an integer past 2^53, which would change what a recomputed
// id or digest covers; such a value is refused rather than read wrong.
function parseExactJson(text: string): unknown {
  return JSON.parse(text, (_key, value: unknown, context?: { source?: string }) => {
    if (typeof value === "number" && Number.isInteger(value) && !Number.isSafeInteger(value) && /^-?\d+$/u.test(context?.source ?? "")) {
      throw new RunxClientError("artifact", `Native artifact holds an integer this client cannot read exactly: ${context?.source}.`);
    }
    return value;
  });
}

export async function readProjectArtifact(reference: unknown, projectDir: string, maximum: number): Promise<unknown> {
  try {
    const artifact = validateProjectArtifactContract(reference);
    if (!Number.isSafeInteger(artifact.bytes) || artifact.bytes <= 0 || !/^sha256:[0-9a-f]{64}$/u.test(artifact.digest) || artifact.uri !== `runx:project-artifact:${artifact.digest}` || artifact.media_type !== "application/json") {
      throw new Error("Invalid project artifact content address.");
    }
    const result = await readBoundedJson(artifact.path, path.join(projectDir, "artifacts"), maximum);
    if (artifact.bytes !== result.bytes.byteLength || artifact.digest !== `sha256:${createHash("sha256").update(result.bytes).digest("hex")}`) {
      throw new Error("Project artifact digest or byte count differs from its descriptor.");
    }
    return result.value;
  } catch (error) {
    throw new RunxClientError("artifact", `Could not read digest-bound native artifact: ${error instanceof Error ? error.message : String(error)}`, { cause: error });
  }
}
