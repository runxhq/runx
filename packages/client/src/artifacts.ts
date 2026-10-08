import { createHash } from "node:crypto";
import { constants } from "node:fs";
import { lstat, open, realpath } from "node:fs/promises";
import path from "node:path";
import { validateProjectArtifactRefContract } from "@runxhq/contracts";
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
    return { bytes: buffer.subarray(0, bytes), value: JSON.parse(buffer.subarray(0, bytes).toString("utf8")) };
  } finally { await handle.close(); }
}

export async function readProjectArtifact(reference: unknown, projectDir: string, maximum: number): Promise<unknown> {
  try {
    const ref = validateProjectArtifactRefContract(reference);
    if (!Number.isSafeInteger(ref.bytes) || ref.bytes <= 0 || !/^sha256:[0-9a-f]{64}$/u.test(ref.digest) || ref.ref !== `runx:project-artifact:${ref.digest}` || ref.media_type !== "application/json") {
      throw new Error("Invalid project artifact content address.");
    }
    const result = await readBoundedJson(ref.path, path.join(projectDir, "artifacts"), maximum);
    if (ref.bytes !== result.bytes.byteLength || ref.digest !== `sha256:${createHash("sha256").update(result.bytes).digest("hex")}`) {
      throw new Error("Project artifact digest or byte count differs from its reference.");
    }
    return result.value;
  } catch (error) {
    throw new RunxClientError("artifact", `Could not read digest-bound native artifact: ${error instanceof Error ? error.message : String(error)}`, { cause: error });
  }
}
