#!/usr/bin/env node

// Distribution selects packages; native package preparation owns their closure.
import { spawnSync } from "node:child_process";
import { readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { resolveNativeRunxBinary } from "./lib/native-parser.mjs";

const root = path.resolve(fileURLToPath(new URL("..", import.meta.url)));
const output = path.join(root, "crates/runx-runtime/bundled-skills.json");
const names = [
  "skill-build", "skill-review", "skill-adopt", "diagnose-skill-run",
  "run-history", "receipt-query", "receipt-audit", "least-privilege",
  "policy-draft", "sign-receipt",
];
const binary = resolveNativeRunxBinary();
const skills = names.map((name) => {
  const result = spawnSync(binary, ["registry", "package", `skills/${name}`, "--json"], {
    cwd: root,
    encoding: "utf8",
    maxBuffer: 32 * 1024 * 1024,
  });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(`Cannot bundle ${name}: ${result.stderr || result.stdout}`);
  const response = JSON.parse(result.stdout);
  const prepared = response.registry?.package;
  if (response.status !== "success" || prepared?.metadata?.name !== name) {
    throw new Error(`Native package preparation returned the wrong identity for ${name}`);
  }
  return { skill_id: `runx/${name}`, package: prepared };
});
const contents = `${JSON.stringify({
  orientation: await readFile(path.join(root, "SKILL.md"), "utf8"),
  skills,
}, null, 2)}\n`;

if (process.argv.includes("--check")) {
  if (await readFile(output, "utf8").catch(() => null) !== contents) {
    throw new Error("Bundled core skills are stale; run pnpm bundled-skills:generate");
  }
} else {
  await writeFile(output, contents, "utf8");
}
console.log(`${process.argv.includes("--check") ? "checked" : "generated"} ${skills.length} bundled core skills`);
