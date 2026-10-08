import { mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";

const workspace = path.resolve(process.env.RUNX_CWD || process.cwd());
const root = path.join(workspace, ".runx", "sourcey-journey");
const project = path.join(root, "project");
const bin = path.join(root, "bin");
mkdirSync(project, { recursive: true });
mkdirSync(bin, { recursive: true });

writeFileSync(path.join(project, "sourcey.config.ts"), `export default {
  name: "Runx Sourcey Journey",
  navigation: {
    tabs: [{ tab: "Guides", groups: [{ group: "Start", pages: ["introduction"] }] }],
  },
};
`);
writeFileSync(path.join(project, "introduction.md"), `---
title: Runx Sourcey Journey
---

# Runx Sourcey Journey

A deterministic documentation project used to prove the complete governed workflow.
`);
writeFileSync(path.join(bin, "sourcey.mjs"), `import { mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";

const outputIndex = process.argv.indexOf("-o");
const outputDir = outputIndex >= 0
  ? process.argv[outputIndex + 1]
  : path.resolve(process.cwd(), ".sourcey/runx-docs");
mkdirSync(outputDir, { recursive: true });
writeFileSync(path.join(outputDir, "index.html"), \`<!doctype html>
<html><head><title>Runx Sourcey Journey</title></head>
<body><main id="content-area"><h1>Runx Sourcey Journey</h1>
<p>Governed documentation build verified.</p></main></body></html>\`);
`);

process.stdout.write(`${JSON.stringify({
  fixture_workspace: {
    data: {
      root: path.relative(workspace, root),
      file_count: 3,
    },
  },
})}\n`);
