import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { test } from "node:test";

const root = path.resolve(fileURLToPath(new URL("..", import.meta.url)));
const script = path.join(root, "scripts/build-channel-input.mjs");
const topology = JSON.parse(readFileSync(path.join(root, "packages/cli/native/supported-platforms.json"), "utf8"));
const version = JSON.parse(readFileSync(path.join(root, "packages/cli/package.json"), "utf8")).version;
const homepage = JSON.parse(readFileSync(path.join(root, "packages/cli/package.json"), "utf8")).homepage;

test("channel input binds every platform to actual archive bytes", () => {
  const dir = mkdtempSync(path.join(os.tmpdir(), "runx-channel-input-"));
  try {
    for (const [platform, entry] of Object.entries(topology.nativePackages)) {
      const file = `runx-${version}-${entry.rustTarget}.${entry.archiveExtension}`;
      const stem = `runx-${version}-${entry.rustTarget}`;
      const stage = path.join(dir, stem);
      mkdirSync(stage);
      writeFileSync(path.join(stage, path.basename(entry.binary)), `binary for ${platform}`);
      writeFileSync(path.join(stage, path.basename(entry.worker)), `worker for ${platform}`);
      makeArchive(path.join(dir, file), dir, stem, entry.archiveExtension);
      const sha256 = createHash("sha256").update(readFileSync(path.join(dir, file))).digest("hex");
      writeFileSync(path.join(dir, `${file}.sha256`), `${sha256}  ${file}\n`);
    }
    const commit = execFileSync("git", ["rev-parse", "HEAD"], { cwd: root, encoding: "utf8" }).trim();
    const args = [script, "--version", version, "--tag", `cli-v${version}`, "--commit", commit, "--repo", "runxhq/runx", "--archives", dir, "--out", path.join(dir, "input.json")];
    execFileSync(process.execPath, args);
    const input = JSON.parse(readFileSync(path.join(dir, "input.json"), "utf8"));
    assert.equal(Object.keys(input.artifacts).length, Object.keys(topology.nativePackages).length);
    assert.equal(input.commit_ref, commit);
    assert.equal(input.homepage, homepage);
    for (const [platform, entry] of Object.entries(topology.nativePackages)) {
      const artifact = input.artifacts[entry.rustTarget];
      assert.equal(artifact.size, readFileSync(path.join(dir, artifact.file)).length);
      assert.equal(artifact.binary.sha256, createHash("sha256").update(`binary for ${platform}`).digest("hex"));
      assert.equal(artifact.worker.sha256, createHash("sha256").update(`worker for ${platform}`).digest("hex"));
    }

    const changed = Object.values(topology.nativePackages)[0];
    const changedFile = `runx-${version}-${changed.rustTarget}.${changed.archiveExtension}`;
    writeFileSync(path.join(dir, changedFile), "tampered archive");
    assert.throws(() => execFileSync(process.execPath, args, { stdio: "pipe" }), /checksum sidecar does not match archive bytes/u);

    const stem = `runx-${version}-${changed.rustTarget}`;
    const worker = path.join(dir, stem, path.basename(changed.worker));
    rmSync(worker);
    makeArchive(path.join(dir, changedFile), dir, stem, changed.archiveExtension);
    const sha256 = createHash("sha256").update(readFileSync(path.join(dir, changedFile))).digest("hex");
    writeFileSync(path.join(dir, `${changedFile}.sha256`), `${sha256}  ${changedFile}\n`);
    assert.throws(() => execFileSync(process.execPath, args, { stdio: "pipe" }), /Command failed/u);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

function makeArchive(archive, dir, stem, extension) {
  rmSync(archive, { force: true });
  if (extension === "zip") {
    execFileSync("python3", [
      "-c",
      "import pathlib,sys,zipfile; root=pathlib.Path(sys.argv[1]); stem=sys.argv[2]; z=zipfile.ZipFile(sys.argv[3], 'w'); [z.write(p, p.relative_to(root)) for p in (root/stem).iterdir()]; z.close()",
      dir, stem, archive,
    ]);
  } else {
    execFileSync("tar", ["-czf", archive, "-C", dir, stem]);
  }
}
