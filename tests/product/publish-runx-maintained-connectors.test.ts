import { spawnSync } from "node:child_process";
import { chmodSync, copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { afterEach, describe, expect, it } from "vitest";

const script = fileURLToPath(new URL("../../scripts/publish-runx-maintained-connectors.mjs", import.meta.url));

/**
 * A runx stand-in: `package` answers with the facts in `<dir>/package.json`,
 * and `publish` records into `registry.json`, answering unchanged for a version
 * it holds. `<dir>/recorded.json` overrides what the registry records.
 */
const FAKE_RUNX = `#!/usr/bin/env node
const fs = require("node:fs");
const path = require("node:path");
const [, , , action, directory] = process.argv;
const facts = JSON.parse(fs.readFileSync(path.join(directory, "package.json"), "utf8"));
if (action === "package") {
  process.stdout.write(JSON.stringify({ status: "success", registry: { action: "connector_package", connector: facts } }));
  process.exit(0);
}
const store = path.join(process.cwd(), "registry.json");
const held = fs.existsSync(store) ? JSON.parse(fs.readFileSync(store, "utf8")) : {};
const override = path.join(directory, "recorded.json");
const recorded = fs.existsSync(override) ? JSON.parse(fs.readFileSync(override, "utf8")) : facts;
const key = recorded.ref + "@" + recorded.version;
const status = held[key] ? "unchanged" : "published";
held[key] = recorded.content_digest;
fs.writeFileSync(store, JSON.stringify(held));
process.stdout.write(JSON.stringify({ status: "success", registry: { action: "connector_publish", publish: { status, ...recorded } } }));
`;

const digest = (character: string) => `sha256:${character.repeat(64)}`;
const entry = (name: string, character: string) => ({ ref: name, version: `sha-${character.repeat(12)}`, content_digest: digest(character) });

let roots: string[] = [];
afterEach(() => {
  for (const root of roots) rmSync(root, { recursive: true, force: true });
  roots = [];
});

function workspace(connectors: readonly ReturnType<typeof entry>[]) {
  const root = mkdtempSync(path.join(os.tmpdir(), "runx-publish-official-"));
  roots.push(root);
  mkdirSync(path.join(root, "scripts"));
  copyFileSync(script, path.join(root, "scripts/publish-runx-maintained-connectors.mjs"));
  const runx = path.join(root, "runx");
  writeFileSync(runx, FAKE_RUNX);
  chmodSync(runx, 0o755);
  mkdirSync(path.join(root, "connectors"));
  mkdirSync(path.join(root, "registry"));
  writeFileSync(path.join(root, "registry/runx-maintained-connectors.lock.json"), JSON.stringify({ connectors }));
  for (const connector of connectors) {
    // A connector lives at connectors/<name>/: its name is its path.
    const directory = path.join(root, "connectors", ...connector.ref.split("/"));
    mkdirSync(directory, { recursive: true });
    writeFileSync(path.join(directory, "package.json"), JSON.stringify(connector));
  }
  const run = (...args: string[]) => spawnSync(process.execPath, [path.join(root, "scripts/publish-runx-maintained-connectors.mjs"), ...args], {
    cwd: root,
    encoding: "utf8",
    env: { ...process.env, RUNX_BIN: runx },
  });
  return { root, run };
}

describe("publish-runx-maintained-connectors", () => {
  it("publishes each lock entry once, and a rerun reports every entry unchanged", () => {
    const { root, run } = workspace([entry("apollo", "a"), entry("github", "b")]);
    const first = run("--registry", "https://registry.runx.test");
    expect(first.status, first.stderr).toBe(0);
    expect(first.stdout).toContain("apollo@sha-aaaaaaaaaaaa published");
    expect(first.stdout).toContain("2 runx-maintained connectors: 2 published, 0 unchanged");
    expect(JSON.parse(readFileSync(path.join(root, "registry.json"), "utf8"))).toEqual({
      "apollo@sha-aaaaaaaaaaaa": digest("a"),
      "github@sha-bbbbbbbbbbbb": digest("b"),
    });

    const again = run("--registry", "https://registry.runx.test");
    expect(again.status, again.stderr).toBe(0);
    expect(again.stdout).toContain("2 runx-maintained connectors: 0 published, 2 unchanged");
  });

  it("publishes nothing when any package differs from the lock", () => {
    const { root, run } = workspace([entry("apollo", "a"), entry("github", "b")]);
    // The github package on disk changed after the lock was generated.
    writeFileSync(path.join(root, "connectors/github/package.json"), JSON.stringify(entry("github", "c")));
    const result = run("--registry", "https://registry.runx.test");
    expect(result.status).toBe(1);
    expect(result.stderr).toContain("github: version sha-cccccccccccc (lock: sha-bbbbbbbbbbbb)");
    expect(existsSync(path.join(root, "registry.json"))).toBe(false);
  });

  it("fails when the registry records anything but the locked version", () => {
    const { root, run } = workspace([entry("apollo", "a")]);
    writeFileSync(path.join(root, "connectors/apollo/recorded.json"), JSON.stringify({ ...entry("apollo", "a"), content_digest: digest("f") }));
    const result = run("--registry", "https://registry.runx.test");
    expect(result.status).toBe(1);
    expect(result.stderr).toContain(`apollo: the registry recorded content_digest ${digest("f")}`);
  });

  it("refuses to run without a registry, or without the lock", () => {
    const { root, run } = workspace([entry("apollo", "a")]);
    expect(run().stderr).toContain("Usage: publish-runx-maintained-connectors --registry <url>");
    rmSync(path.join(root, "registry/runx-maintained-connectors.lock.json"));
    expect(run("--registry", "https://registry.runx.test").stderr).toContain("runx-maintained-connectors.lock.json is missing");
  });
});
