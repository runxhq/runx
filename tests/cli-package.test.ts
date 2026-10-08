import { execFile } from "node:child_process";
import { readFile, stat } from "node:fs/promises";
import path from "node:path";
import { promisify } from "node:util";

import { describe, expect, it } from "vitest";

const execFileAsync = promisify(execFile);
const workspaceRoot = process.cwd();
const cliPackageRoot = path.join(workspaceRoot, "packages", "cli");
const cliBinEntry = path.join(cliPackageRoot, "bin", "runx");
const npm = process.platform === "win32" ? "npm.cmd" : "npm";

describe("CLI package", () => {
  it("ships an executable selector without a TypeScript command backend", async () => {
    const entry = await stat(cliBinEntry);
    expect(entry.isFile()).toBe(true);
    expect(entry.mode & 0o111).not.toBe(0);

    const selector = await readFile(cliBinEntry, "utf8");
    expect(selector).toContain("#!/usr/bin/env node");
    expect(selector).toContain("spawnSync(binaryPath, process.argv.slice(2)");
    for (const token of ["packages/cli/src", "packages/cli/dist", "RUNX_JS_BIN", "npm exec"]) {
      expect(selector, `selector contains ${token}`).not.toContain(token);
    }

    await expect(execFileAsync(cliBinEntry, ["config", "list", "--json"], {
      cwd: workspaceRoot,
      timeout: 30_000,
      maxBuffer: 1024 * 1024,
    })).rejects.toMatchObject({
      stderr: expect.stringContaining(`runx native package ${currentNativePackageName()} is not installed`),
    });
  });

  it("records selector topology for every supported native package", async () => {
    const [topologyText, packageText] = await Promise.all([
      readFile(path.join(cliPackageRoot, "native", "supported-platforms.json"), "utf8"),
      readFile(path.join(cliPackageRoot, "package.json"), "utf8"),
    ]);
    const topology = JSON.parse(topologyText) as {
      readonly schema: string;
      readonly selectorPackage: string;
      readonly nativePackages: Record<string, { readonly package: string; readonly binary: string; readonly worker: string }>;
    };
    const packageJson = JSON.parse(packageText) as {
      readonly name: string;
      readonly bin?: { readonly runx?: string };
      readonly files?: readonly string[];
      readonly runx?: {
        readonly nativeSelector?: {
          readonly schema?: string;
          readonly supportedPlatforms?: readonly string[];
          readonly nativePackagePattern?: string;
        };
      };
      readonly optionalDependencies?: Record<string, string>;
    };

    expect(topology).toMatchObject({
      schema: "runx.rust_cli_selector_topology.v1",
      selectorPackage: "@runxhq/cli",
    });
    expect(packageJson).toMatchObject({
      name: "@runxhq/cli",
      bin: { runx: "./bin/runx" },
      files: ["LICENSE", "bin/runx", "native/supported-platforms.json"],
      runx: {
        nativeSelector: {
          schema: "runx.rust_cli_selector_topology.v1",
          nativePackagePattern: "@runxhq/cli-${platform}",
        },
      },
    });
    for (const field of ["main", "types", "exports", "dependencies", "devDependencies", "peerDependencies", "optionalDependencies", "scripts"]) {
      expect(packageJson, `selector manifest contains stale ${field}`).not.toHaveProperty(field);
    }
    const supportedPlatforms = [
      "darwin-arm64",
      "darwin-x64",
      "linux-arm64",
      "linux-x64",
      "win32-x64",
    ];
    expect(Object.keys(topology.nativePackages).sort()).toEqual(supportedPlatforms);
    expect(packageJson.runx?.nativeSelector?.supportedPlatforms).toEqual(supportedPlatforms);
    for (const [platform, entry] of Object.entries(topology.nativePackages)) {
      expect(entry.package).toBe(`@runxhq/cli-${platform}`);
      expect(entry.worker).toBe(platform === "win32-x64" ? "bin/runx-js-worker.exe" : "bin/runx-js-worker");
    }
  });

  it("packs @runxhq/cli as selector artifacts only", async () => {
    const { stdout } = await execFileAsync(npm, ["pack", "--dry-run", "--json"], {
      cwd: cliPackageRoot,
      timeout: 30_000,
      maxBuffer: 1024 * 1024,
    });
    const [pack] = JSON.parse(stdout) as [
      {
        readonly name: string;
        readonly version: string;
        readonly files: readonly { readonly path: string }[];
      },
    ];

    expect(pack.name).toBe("@runxhq/cli");
    expect(pack.version).not.toBe("0.0.0");
    const files = pack.files.map((file) => file.path);
    expect(files).toEqual(expect.arrayContaining([
      `bin/${path.basename(cliBinEntry)}`,
      "native/supported-platforms.json",
      "package.json",
      "LICENSE",
    ]));
    expect(files.some((file) => /^(dist|src|tools|node_modules|\.runx)\//u.test(file))).toBe(false);
    expect(files.some((file) => /^bin\/runx\.(?:js|mjs|cjs)$/u.test(file))).toBe(false);

    const textFiles = files.filter((file) => /\.(?:json|md|txt|js|mjs|cjs|ts|tsx)$/u.test(file));
    const forbiddenTokens = [
      "npm exec",
      "packages/cli/src",
      "packages/cli/dist",
      "process.execPath",
    ];
    for (const file of textFiles) {
      const contents = await readFile(path.join(cliPackageRoot, file), "utf8");
      for (const token of forbiddenTokens) {
        expect(contents, `${file} contains ${token}`).not.toContain(token);
      }
    }
  }, 60_000);
});

function currentNativePackageName(): string {
  return `@runxhq/cli-${process.platform}-${process.arch}`;
}
