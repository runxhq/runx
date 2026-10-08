import { execFileSync } from "node:child_process";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";

import { describe, expect, it } from "vitest";

const selectorScript = path.resolve("..", ".github", "scripts", "select-registry-publish-paths.mjs");

describe("registry publish selector", () => {
  it("detects nested skill and binding changes from an oss gitlink-only bump", async () => {
    const root = await mkdtemp(path.join(os.tmpdir(), "runx-registry-selector-"));
    try {
      const oss = path.join(root, "oss");
      await mkdir(path.join(oss, "skills", "sourcey"), { recursive: true });
      await mkdir(path.join(oss, "bindings", "nilstate", "icey-server-operator"), { recursive: true });
      await writeFile(path.join(oss, "skills", "sourcey", "SKILL.md"), "---\nname: sourcey\n---\n\nSourcey.\n");
      await writeFile(path.join(oss, "bindings", "nilstate", "icey-server-operator", "binding.json"), "{}\n");
      await writeFile(path.join(oss, "bindings", "nilstate", "icey-server-operator", "run.yaml"), "skill: icey-server-operator\n");

      git(oss, ["init"]);
      configureGit(oss);
      git(oss, ["add", "."]);
      git(oss, ["commit", "-m", "initial oss"]);

      git(root, ["init"]);
      configureGit(root);
      git(root, ["add", "oss"]);
      git(root, ["commit", "-m", "initial gitlink"]);
      const before = git(root, ["rev-parse", "HEAD"]).trim();

      await writeFile(path.join(oss, "skills", "sourcey", "SKILL.md"), "---\nname: sourcey\n---\n\nSourcey changed.\n");
      await writeFile(path.join(oss, "bindings", "nilstate", "icey-server-operator", "run.yaml"), "skill: icey-server-operator\n# changed\n");
      git(oss, ["add", "."]);
      git(oss, ["commit", "-m", "change nested publish inputs"]);
      git(root, ["add", "oss"]);
      git(root, ["commit", "-m", "bump oss gitlink"]);
      const after = git(root, ["rev-parse", "HEAD"]).trim();

      const result = runSelector(root, "--event", "push", "--before", before, "--after", after);

      expect(result).toEqual({
        skills: ["oss/skills/sourcey"],
        bindings: ["oss/bindings/nilstate/icey-server-operator/binding.json"],
      });
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("normalizes explicit workflow dispatch paths and rejects unsafe paths", async () => {
    const root = await mkdtemp(path.join(os.tmpdir(), "runx-registry-dispatch-"));
    try {
      const result = runSelector(
        root,
        "--event",
        "workflow_dispatch",
        "--skill-paths",
        "skills/sourcey, oss/skills/issue-to-pr/SKILL.md",
        "--profile-paths",
        "bindings/nilstate/icey-server-operator, oss/bindings/runx/sourcey/binding.json",
      );

      expect(result).toEqual({
        skills: ["oss/skills/issue-to-pr", "oss/skills/sourcey"],
        bindings: [
          "oss/bindings/nilstate/icey-server-operator/binding.json",
          "oss/bindings/runx/sourcey/binding.json",
        ],
      });

      expect(() => runSelector(root, "--event", "workflow_dispatch", "--skill-paths", "../secret")).toThrow(
        /Parent paths are not allowed/,
      );
      expect(() => runSelector(root, "--event", "workflow_dispatch", "--skill-paths", "skills/..")).toThrow(
        /Dot path segments are not allowed/,
      );
      expect(() => runSelector(root, "--event", "workflow_dispatch", "--profile-paths", "bindings/../evil")).toThrow(
        /Dot path segments are not allowed/,
      );
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  it("does not select deleted nested publish paths from an oss gitlink bump", async () => {
    const root = await mkdtemp(path.join(os.tmpdir(), "runx-registry-selector-delete-"));
    try {
      const oss = path.join(root, "oss");
      await mkdir(path.join(oss, "skills", "removed"), { recursive: true });
      await mkdir(path.join(oss, "bindings", "fixture", "removed"), { recursive: true });
      await writeFile(path.join(oss, "skills", "removed", "SKILL.md"), "---\nname: removed\n---\n\nRemoved.\n");
      await writeFile(path.join(oss, "bindings", "fixture", "removed", "binding.json"), "{}\n");

      git(oss, ["init"]);
      configureGit(oss);
      git(oss, ["add", "."]);
      git(oss, ["commit", "-m", "initial oss"]);

      git(root, ["init"]);
      configureGit(root);
      git(root, ["add", "oss"]);
      git(root, ["commit", "-m", "initial gitlink"]);
      const before = git(root, ["rev-parse", "HEAD"]).trim();

      await rm(path.join(oss, "skills", "removed"), { recursive: true, force: true });
      await rm(path.join(oss, "bindings", "fixture", "removed"), { recursive: true, force: true });
      git(oss, ["add", "-A"]);
      git(oss, ["commit", "-m", "delete publish paths"]);
      git(root, ["add", "oss"]);
      git(root, ["commit", "-m", "bump oss gitlink"]);
      const after = git(root, ["rev-parse", "HEAD"]).trim();

      expect(runSelector(root, "--event", "push", "--before", before, "--after", after)).toEqual({
        skills: [],
        bindings: [],
      });
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });
});

function runSelector(root: string, ...args: readonly string[]): { skills: string[]; bindings: string[] } {
  const output = execFileSync("node", [selectorScript, "--root", root, ...args], {
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
  });
  return JSON.parse(output) as { skills: string[]; bindings: string[] };
}

function git(cwd: string, args: readonly string[]): string {
  return execFileSync("git", args, {
    cwd,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
  });
}

function configureGit(cwd: string): void {
  git(cwd, ["config", "user.email", "runx-test@example.test"]);
  git(cwd, ["config", "user.name", "runx test"]);
}
