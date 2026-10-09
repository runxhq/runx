import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { chmod, mkdir, mkdtemp, readFile, rm, symlink, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { createRunxClient, executionBindingFromInspection, RunxClientError } from "./index.js";
import { invoke } from "./process.js";
import { decodeSkill } from "./decode.js";

const roots: string[] = [];
afterEach(async () => { await Promise.all(roots.splice(0).map((root) => rm(root, { recursive: true, force: true }))); });

async function setup(source = "process.stdout.write('{}');") {
  const root = await mkdtemp(path.join(os.tmpdir(), "runx-client-test-")); roots.push(root);
  const cwd = path.join(root, "workspace"), projectDir = path.join(root, "project"), home = path.join(root, "home"), receiptDir = path.join(root, "receipts");
  await Promise.all([cwd, projectDir, home, receiptDir].map((dir) => mkdir(dir)));
  const command = path.join(root, "runx.mjs");
  await writeFile(command, `#!${process.execPath}\n${source}`); await chmod(command, 0o755);
  const config = { command, cwd, projectDir, home, environment: { PATH: process.env.PATH }, timeoutMs: 3000 };
  await storeReceipt(receiptDir);
  return { root, config, receiptDir };
}
async function artifact(projectDir: string, value: unknown, label = "request") {
  const bytes = Buffer.from(JSON.stringify(value));
  const digest = `sha256:${createHash("sha256").update(bytes).digest("hex")}`;
  const dir = path.join(projectDir, "artifacts", "skill-runs", "run_test", "requests"); await mkdir(dir, { recursive: true });
  const file = path.join(dir, `${label}-${digest.slice(7)}.json`); await writeFile(file, bytes);
  return { schema: "runx.project_artifact_ref.v1", ref: `runx:project-artifact:${digest}`, digest, bytes: bytes.byteLength, media_type: "application/json", path: file };
}
const receiptFixture = JSON.parse(await readFile(new URL("../../../fixtures/receipt-verify/valid-production/receipt.json", import.meta.url), "utf8"));
delete receiptFixture.lineage.sync;
async function storeReceipt(receiptDir: string) {
  await writeFile(path.join(receiptDir, receiptFixture.id.replace(":", "-") + ".json"), JSON.stringify(receiptFixture));
}
const inputRequest = { id: "input.message", kind: "input", questions: [{ id: "message", prompt: "Message", required: true, type: "string" }] };
const output = (status: string, outcome: string, fields = {}) => ({ schema: "runx.skill_run.v1", status, outcome, run_id: "run_test", ...(status === "sealed" ? { receipt_id: receiptFixture.id } : {}), ...fields });

describe("native invocation and decoding", () => {
  it("inspects a manual-only package without fabricating an execution binding", async () => {
    const { root, config } = await setup(`process.stdout.write(JSON.stringify({ schema: 'runx.skill.inspect.v1', status: 'ok', name: 'manual', package_digest: 'sha256:${"a".repeat(64)}', runners: [] }));`);
    const inspection = await createRunxClient(config).skills.inspect({ skillPath: root });
    expect(inspection.runner).toBeUndefined();
    expect(() => executionBindingFromInspection(inspection)).toThrow("selected runner");
  });
  it("passes an exact resume packet without changing approval actors", async () => {
    const { root, config, receiptDir } = await setup();
    const capture = path.join(root, "resume.json");
    await writeFile(config.command, `#!${process.execPath}\nimport { readFileSync, writeFileSync } from 'node:fs';
      writeFileSync(${JSON.stringify(capture)}, JSON.stringify({ args: process.argv.slice(2), packet: JSON.parse(readFileSync(process.argv[4], 'utf8')) }));
      process.stdout.write(JSON.stringify({ status: 'failure', error: { code: 'stale_resolution' } })); process.exitCode = 1;`);
    const packet = { answers: { task: { actor: "agent", value: "answer" } }, approvals: { gate: { actor: "human", approved: true } } };
    await expect(createRunxClient(config).runs.resume({ runId: "run_test", receiptDir, resolutions: packet })).resolves.toMatchObject({ status: "failure", error: { code: "stale_resolution" } });
    const captured = JSON.parse(await readFile(capture, "utf8"));
    expect(captured.packet).toEqual(packet);
    expect(captured.args.slice(0, 2)).toEqual(["resume", "run_test"]);
    await expect(readFile(captured.args[2])).rejects.toMatchObject({ code: "ENOENT" });
  });
  it("names the turn server an MCP client starts, with the trusted environment", async () => {
    const { root, config, receiptDir } = await setup();
    const server = createRunxClient(config).turns.server({ runId: "run_test", receiptDir, environment: { SKILL_FLAG: "1" } });
    expect(server).toMatchObject({ command: config.command, args: ["turn", "run_test", "--receipt-dir", receiptDir], cwd: config.cwd });
    expect(server.env).toMatchObject({ SKILL_FLAG: "1", RUNX_CWD: config.cwd, RUNX_PROJECT_DIR: config.projectDir, RUNX_HOME: config.home, NO_COLOR: "1" });
    expect(() => createRunxClient(config).turns.server({ runId: "--help", receiptDir })).toThrow("positional");
    expect(() => createRunxClient(config).turns.server({ runId: "run_test", receiptDir: "relative" })).toThrow("absolute");
    expect(() => createRunxClient(config).turns.server({ runId: "run_test", receiptDir, environment: { RUNX_HOME: path.join(root, "x") } })).toThrow("runtime-owned");
  });
  it("returns native failed harness and invalid verification verdicts as data", async () => {
    const { root, config, receiptDir } = await setup();
    const capture = path.join(root, "verify.json");
    await writeFile(config.command, `#!${process.execPath}\nimport { readFileSync, writeFileSync } from 'node:fs';
      if (process.argv[2] === 'harness') { process.stdout.write(JSON.stringify({ status: 'failed', case_count: 1, assertion_error_count: 1, assertion_errors: ['failed'], case_names: ['case'], receipt_ids: [], graph_case_count: 0 })); process.exitCode = 1; }
      else { writeFileSync(${JSON.stringify(capture)}, readFileSync(0)); process.stdout.write(JSON.stringify({ schema: 'runx.verify_verdict.v1', valid: false, receipt_id: null })); process.exitCode = 3; }`);
    const client = createRunxClient(config);
    await expect(client.harness.run({ skillPath: root, receiptDir })).resolves.toMatchObject({ status: "failed", assertion_errors: ["failed"] });
    await expect(client.receipts.verify({ receipt: '{"receipt":"exact"}' })).resolves.toMatchObject({ valid: false });
    expect(await readFile(capture, "utf8")).toBe('{"receipt":"exact"}');
  });
  it("keeps input keys out of argv and removes private payloads", async () => {
    const { root, config, receiptDir } = await setup();
    const capture = path.join(root, "capture.json");
    await writeFile(config.command, `#!${process.execPath}\nimport { readFileSync, writeFileSync } from 'node:fs';
      const file = process.argv[process.argv.indexOf('--inputs') + 1];
      writeFileSync(${JSON.stringify(capture)}, JSON.stringify({ args: process.argv.slice(2), file, inputs: JSON.parse(readFileSync(file, 'utf8')), home: process.env.HOME }));
      process.stdout.write(JSON.stringify(${JSON.stringify(output("sealed", "completed", { closure: { disposition: "closed" } }))}));`);
    await createRunxClient(config).skills.run({ skillPath: "/skill", receiptDir, inputs: { "receipt-dir": "untrusted", nested: { value: 1 } } });
    const captured = JSON.parse(await readFile(capture, "utf8"));
    expect(captured.args.filter((arg: string) => arg === "--receipt-dir")).toHaveLength(1);
    expect(captured.inputs).toEqual({ "receipt-dir": "untrusted", nested: { value: 1 } });
    expect(captured.home).toBe(config.home);
    await expect(readFile(captured.file)).rejects.toMatchObject({ code: "ENOENT" });
  });
  it("accepts sealed failure on exit 1 and rejects inconsistent exits", async () => {
    const { config, receiptDir } = await setup();
    const result = { exitCode: 1, stdout: JSON.stringify(output("sealed", "failed", { closure: { disposition: "failed" } })), stderr: "" };
    await expect(decodeSkill(result, { projectDir: config.projectDir, receiptDir, maxArtifactBytes: 10000 })).resolves.toMatchObject({ status: "sealed", outcome: "failed" });
    await expect(decodeSkill({ ...result, exitCode: 0 }, { projectDir: config.projectDir, receiptDir, maxArtifactBytes: 10000 })).rejects.toThrow("exit code disagrees");
  });
  it("hydrates spilled requests and rejects digest drift", async () => {
    const { config, receiptDir } = await setup();
    const ref = await artifact(config.projectDir, inputRequest);
    const set = await artifact(config.projectDir, [{ id: inputRequest.id, kind: "input", request_digest: ref.digest, artifact_ref: ref }], "request-set");
    const result = { exitCode: 2, stdout: JSON.stringify(output("needs_agent", "deferred", { requests: [], request_set_ref: set })), stderr: "" };
    const settings = { projectDir: config.projectDir, receiptDir, maxArtifactBytes: 10000 };
    await expect(decodeSkill(result, settings)).resolves.toMatchObject({ requests: [inputRequest] });
    await writeFile(ref.path, JSON.stringify({ ...inputRequest, id: "other" }));
    await expect(decodeSkill(result, settings)).rejects.toThrow("digest or byte count");
  });
  it("hydrates oversized results and checks the total artifact budget", async () => {
    const { config, receiptDir } = await setup();
    const value = { message: "value".repeat(8000) };
    const ref = await artifact(config.projectDir, value, "result");
    const result = { exitCode: 0, stdout: JSON.stringify(output("sealed", "completed", { closure: { disposition: "closed" }, result: { artifact_ref: ref } })), stderr: "" };
    await expect(decodeSkill(result, { projectDir: config.projectDir, receiptDir, maxArtifactBytes: 100000 })).resolves.toMatchObject({ result: value });
    await expect(decodeSkill(result, { projectDir: config.projectDir, receiptDir, maxArtifactBytes: 100 })).rejects.toThrow("byte budget");
  });
  it("preserves ordinary provider artifact references in skill results", async () => {
    const { config, receiptDir } = await setup();
    const value = { artifact_ref: "runx:artifact:sha256:provider", content_digest: "provider-digest" };
    const result = { exitCode: 0, stdout: JSON.stringify(output("sealed", "completed", { closure: { disposition: "closed" }, result: value })), stderr: "" };
    await expect(decodeSkill(result, { projectDir: config.projectDir, receiptDir, maxArtifactBytes: 10000 })).resolves.toMatchObject({ result: value });
  });
  it("recovers the full-output fallback and refuses another run's output", async () => {
    const { config, receiptDir } = await setup();
    const full = output("needs_agent", "deferred", { requests: [inputRequest], result: "large payload" });
    const ref = await artifact(config.projectDir, full, "full-output");
    const result = { exitCode: 2, stdout: JSON.stringify(output("needs_agent", "deferred", { requests: [], artifact_refs: [ref] })), stderr: "" };
    const settings = { projectDir: config.projectDir, receiptDir, maxArtifactBytes: 10000 };
    await expect(decodeSkill(result, settings)).resolves.toMatchObject({ requests: [inputRequest], result: "large payload" });
    const other = await artifact(config.projectDir, { ...full, run_id: "other" }, "full-output");
    await expect(decodeSkill({ ...result, stdout: JSON.stringify(output("needs_agent", "deferred", { artifact_refs: [other] })) }, settings)).rejects.toThrow("disagrees on run_id");
  });
  it("refuses artifacts outside the project and final-component symlinks", async () => {
    const { root, config, receiptDir } = await setup();
    const ref = await artifact(config.projectDir, inputRequest);
    const outside = path.join(root, "outside.json"); await writeFile(outside, JSON.stringify(inputRequest));
    const link = path.join(path.dirname(ref.path), "link.json"); await symlink(ref.path, link);
    for (const file of [outside, link]) {
      const result = { exitCode: 2, stdout: JSON.stringify(output("needs_agent", "deferred", { requests: [{ id: inputRequest.id, kind: "input", request_digest: ref.digest, artifact_ref: { ...ref, path: file } }] })), stderr: "" };
      await expect(decodeSkill(result, { projectDir: config.projectDir, receiptDir, maxArtifactBytes: 10000 })).rejects.toBeInstanceOf(RunxClientError);
    }
  });
  it.each(["RUNX_HOME", "RUNX_RECEIPT_SIGN_KID", "NODE_OPTIONS", "PATH", "LD_PRELOAD"])("refuses untrusted %s before spawning", async (name) => {
    const { config, receiptDir } = await setup();
    await expect(createRunxClient(config).skills.run({ skillPath: "/skill", receiptDir, environment: { [name]: "attack" } })).rejects.toThrow("runtime-owned");
  });
});

describe("process lifetime", () => {
  it("bounds combined byte output and stdin", async () => {
    const { config } = await setup();
    const base = { ...config, env: {}, args: [], maxOutputBytes: 100, maxInputBytes: 10, timeoutMs: 1000 };
    await writeFile(config.command, `#!${process.execPath}\nprocess.stdout.write('a'.repeat(60)); process.stderr.write('b'.repeat(60));`);
    await expect(invoke(base)).rejects.toMatchObject({ reason: "output_limit" });
    await expect(invoke({ ...base, input: "x".repeat(11) })).rejects.toMatchObject({ reason: "input_limit" });
  });
  it("handles spawn failure and both early and live abort", async () => {
    const { config } = await setup("setInterval(() => {}, 1000);");
    const base = { ...config, env: {}, args: [], maxOutputBytes: 1000, maxInputBytes: 100, timeoutMs: 1000 };
    await expect(invoke({ ...base, command: path.join(config.cwd, "absent") })).rejects.toMatchObject({ reason: "spawn" });
    await expect(invoke({ ...base, signal: AbortSignal.abort() })).rejects.toMatchObject({ reason: "aborted" });
    const controller = new AbortController();
    const pending = invoke({ ...base, signal: controller.signal }); setTimeout(() => controller.abort(), 50);
    await expect(pending).rejects.toMatchObject({ reason: "aborted" });
  });
  it.skipIf(process.platform === "win32")("kills a descendant that ignores SIGTERM after the direct child exits", async () => {
    const { root, config } = await setup();
    const pidFile = path.join(root, "descendant.pid");
    const childCode = `require('node:fs').writeFileSync(${JSON.stringify(pidFile)}, String(process.pid)); process.on('SIGTERM', () => {}); setInterval(() => {}, 1000);`;
    await writeFile(config.command, `#!${process.execPath}\nimport { spawn } from 'node:child_process'; spawn(${JSON.stringify(process.execPath)}, ['-e', ${JSON.stringify(childCode)}], { stdio: 'ignore' }); setInterval(() => {}, 1000);`);
    await expect(invoke({ ...config, env: {}, args: [], maxOutputBytes: 1000, maxInputBytes: 100, timeoutMs: 500 })).rejects.toMatchObject({ reason: "timeout" });
    const pid = Number(await readFile(pidFile, "utf8"));
    // The process may briefly remain a zombie until its new parent reaps it.
    const probe = spawn("ps", ["-o", "stat=", "-p", String(pid)]);
    let stat = ""; probe.stdout.on("data", (chunk) => { stat += chunk; });
    await new Promise((resolve) => probe.on("close", resolve));
    expect(stat.trim() === "" || stat.trim().startsWith("Z")).toBe(true);
  });
});
