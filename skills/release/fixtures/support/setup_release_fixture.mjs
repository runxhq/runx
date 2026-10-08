#!/usr/bin/env node

import fs from "node:fs";
import path from "node:path";

const inputs = process.env.RUNX_INPUTS_PATH
  ? JSON.parse(fs.readFileSync(process.env.RUNX_INPUTS_PATH, "utf8"))
  : JSON.parse(process.env.RUNX_INPUTS_JSON || "{}");
const prepareStatus = inputs.prepare_status === "blocked" ? "blocked" : "ready";
const root = path.resolve(process.env.RUNX_CWD || process.cwd());
const project = path.join(root, ".runx", "cache", "harness", "release-fixture-project");
fs.rmSync(project, { recursive: true, force: true });
fs.mkdirSync(project, { recursive: true });
const provider = [
  "#!/usr/bin/env node",
  "import fs from 'node:fs';",
  "import path from 'node:path';",
  `const prepareStatus = ${JSON.stringify(prepareStatus)};`,
  "const phase = process.argv[2];",
  "const version = process.env.RUNX_RELEASE_VERSION;",
  "const channel = process.env.RUNX_RELEASE_CHANNEL;",
  "const statePath = path.join(process.cwd(), 'release-state.json');",
  "if (phase === 'prepare') process.stdout.write(JSON.stringify({status:prepareStatus,version,channel,commit_ref:'fixture-commit',checks:{tests:prepareStatus==='ready'?'pass':'fail',pack:prepareStatus==='ready'?'pass':'blocked'}}));",
  "else if (phase === 'publish') { fs.writeFileSync(statePath, JSON.stringify({version,channel,release_id:'fixture-release'})); process.stdout.write(JSON.stringify({status:'submitted',version,channel,release_id:'fixture-release',locators:['fixture://release/' + version]})); }",
  "else if (phase === 'verify') { const state = fs.existsSync(statePath) ? JSON.parse(fs.readFileSync(statePath, 'utf8')) : {}; const ok = state.version === version && state.channel === channel; process.stdout.write(JSON.stringify({status:ok?'verified':'missing',version:state.version || '',channel:state.channel || '',release_id:state.release_id || '',locators:ok?['fixture://release/' + version]:[],checks:{readback:ok?'pass':'fail'}})); }",
  "else process.exit(2);",
].join("\n");
fs.writeFileSync(path.join(project, "provider.mjs"), provider + "\n");
const profile = {
  schema: "runx.release.profile.v1",
  id: "fixture/local-release",
  channel: "fixture",
  commands: {
    prepare: { argv: ["node", "./provider.mjs", "prepare"], cwd: ".", timeout_ms: 10000 },
    publish: { argv: ["node", "./provider.mjs", "publish"], cwd: ".", timeout_ms: 10000 },
    verify: { argv: ["node", "./provider.mjs", "verify"], cwd: ".", timeout_ms: 3600000 }
  }
};
const profilePath = path.join(project, "release-profile.json");
fs.writeFileSync(profilePath, JSON.stringify(profile, null, 2) + "\n");
process.stdout.write(JSON.stringify({ project_root: path.relative(root, project), profile_ref: path.basename(profilePath) }) + "\n");
