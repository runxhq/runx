import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { chmod, mkdir, mkdtemp, readFile, rm, symlink, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { createRunxClient, executionBindingFromInspection, executionClosureFromInspection, RunxClientError } from "./index.js";
import { invoke } from "./process.js";
import { decodeSkill } from "./decode.js";
import { receiptCommitments } from "./receipt-tree.js";
import type { ProjectArtifactRefContract } from "@runxhq/contracts";

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
async function storeReceipt(receiptDir: string) {
  await writeFile(path.join(receiptDir, receiptFixture.id.replace(":", "-") + ".json"), JSON.stringify(receiptFixture));
}
const inputRequest = { id: "input.message", kind: "input", questions: [{ id: "message", prompt: "Message", required: true, type: "string" }] };
const output = (status: string, outcome: string, fields = {}) => ({ schema: "runx.skill_run.v1", status, outcome, run_id: "run_test", ...(status === "sealed" ? { receipt_id: receiptFixture.id } : {}), ...fields });

describe("native invocation and decoding", () => {
  it("inspects a manual-only package without fabricating an execution binding", async () => {
    const { root, config } = await setup(`process.stdout.write(JSON.stringify({ schema: 'runx.skill.inspect.v1', status: 'ok', name: 'manual', skill_path: '/skill', manual_digest: 'sha256:${"e".repeat(64)}', package_digest: 'sha256:${"a".repeat(64)}', runners: [], operator_journeys: [] }));`);
    const inspection = await createRunxClient(config).skills.inspect({ skillPath: root });
    expect(inspection.runner).toBeUndefined();
    expect(() => executionBindingFromInspection(inspection)).toThrow("selected runner");
  });
  it("reads the connector revisions an execution closure runs, and refuses one it cannot read", () => {
    const binding = { connector: "ausca/inbox", manifest_digest: `sha256:${"a".repeat(64)}`, connector_digest: `sha256:${"b".repeat(64)}` };
    const closure = (connectorBindings?: unknown) => ({
      schema: "runx.skill.inspect.v1", status: "ok", name: "agent-inbox", package_digest: `sha256:${"c".repeat(64)}`,
      execution_closure: {
        closure_digest: `sha256:${"d".repeat(64)}`, fully_bound: true,
        package_bindings: [{ skill: "agent-inbox", runner: "create", package_digest: `sha256:${"c".repeat(64)}`, source_kind: "source_root", source_path: "/skill", source_files: ["SKILL.md"] }],
        ...(connectorBindings === undefined ? {} : { connector_bindings: connectorBindings }),
      },
    }) as never;
    expect(executionClosureFromInspection(closure([binding])).connectorBindings).toEqual([binding]);
    expect(executionClosureFromInspection(closure()).connectorBindings).toEqual([]);
    expect(() => executionClosureFromInspection(closure([{ connector: "ausca/inbox" }]))).toThrow("connector_bindings[0]");
  });
  it("passes an exact resume packet without changing approval actors", async () => {
    const { root, config, receiptDir } = await setup();
    const capture = path.join(root, "resume.json");
    await writeFile(config.command, `#!${process.execPath}\nimport { readFileSync, writeFileSync } from 'node:fs';
      writeFileSync(${JSON.stringify(capture)}, JSON.stringify({ args: process.argv.slice(2), packet: JSON.parse(readFileSync(0, 'utf8')) }));
      process.stdout.write(JSON.stringify({ status: 'failure', error: { code: 'stale_resolution' } })); process.exitCode = 1;`);
    const packet = { answers: { task: { request_digest: "sha256:task", value: "answer" } }, approvals: { gate: { request_digest: "sha256:gate", approved: true } } };
    await expect(createRunxClient(config).runs.resume({ runId: "run_test", receiptDir, resolutions: packet })).resolves.toMatchObject({ status: "failure", error: { code: "stale_resolution" } });
    const captured = JSON.parse(await readFile(capture, "utf8"));
    expect(captured.packet).toEqual(packet);
    expect(captured.args.slice(0, 3)).toEqual(["resume", "run_test", "-"]);
  });
  it("reads one run after its cursor, and refuses a read that is not one", async () => {
    const { root, config } = await setup();
    const capture = path.join(root, "history.json");
    const read = {
      schema: "runx.run_read.v1",
      run: { run_id: "run_test", skill: "reply", runner: "reply", state: "paused", package_digest: "sha256:p", execution_closure_digest: "sha256:c", driver: "attached", driven: false, created_at: "t0", updated_at: "t1" },
      waiting: [{ address: "send.approval", request_digest: "sha256:r", request: { kind: "approval", id: "send.approval", gate: { id: "send.approval", reason: "send it" } } }],
      events: [{ stream: "run_test", version: 2, committed_at: "t1", event: { kind: "suspended", step_id: "send", attempt: 1, hold: { kind: "request", address: "send.approval", request_digest: "sha256:r" } } }],
      cursor: "-cursor_1",
      more: false,
    };
    await writeFile(config.command, `#!${process.execPath}\nimport { writeFileSync } from 'node:fs';
      writeFileSync(${JSON.stringify(capture)}, JSON.stringify(process.argv.slice(2)));
      if (process.argv.includes('--after=stale')) { process.stdout.write(JSON.stringify({ status: 'failure', error: { code: 'invalid_cursor', message: 'the cursor is damaged' } })); process.exitCode = 1; }
      else process.stdout.write(JSON.stringify(process.argv.includes('--after=bad') ? { runs: [] } : ${JSON.stringify(read)}));`);
    const client = createRunxClient(config);
    await expect(client.runs.read({ runId: "run_test", after: "-cursor_0" })).resolves.toEqual(read);
    expect(JSON.parse(await readFile(capture, "utf8"))).toEqual(["history", "run_test", "--after=-cursor_0", "--json"]);
    await expect(client.runs.read({ runId: "run_test", after: "bad" })).rejects.toMatchObject({ reason: "contract" });
    await expect(client.runs.read({ runId: "run_test", after: "a b" })).rejects.toThrow("cursor");
    await expect(client.runs.read({ runId: "run_test", after: "stale" })).rejects.toMatchObject({ reason: "exit", code: "invalid_cursor" });
  });
  it("lists the project's skills and reads an artifact by its digest-bound reference", async () => {
    const { config } = await setup(`process.stdout.write(JSON.stringify(process.argv[2] === 'list' ? { schema: 'runx.list.v1', root: '/w', requested_kind: 'skills', items: [{ kind: 'skill', name: 'runx/run-history', source: 'built-in', path: 'runx/run-history', status: 'ok', emits: [] }] } : {}));`);
    const client = createRunxClient(config);
    await expect(client.skills.list()).resolves.toMatchObject({ schema: "runx.list.v1", items: [{ name: "runx/run-history" }] });
    const reference = await artifact(config.projectDir, { verdict: "ok" }) as ProjectArtifactRefContract;
    await expect(client.artifacts.read(reference)).resolves.toEqual({ verdict: "ok" });
    await expect(client.artifacts.read(reference, { maxBytes: 4 })).rejects.toMatchObject({ reason: "artifact" });
    await expect(client.artifacts.read({ ...reference, digest: `sha256:${"0".repeat(64)}`, ref: `runx:project-artifact:sha256:${"0".repeat(64)}` })).rejects.toMatchObject({ reason: "artifact" });
  });
  it("names the turn server an MCP client starts, with the trusted environment", async () => {
    const { root, config, receiptDir } = await setup();
    const server = createRunxClient(config).turns.server({ runId: "run_test", receiptDir, environment: { SKILL_FLAG: "1" } });
    expect(server).toMatchObject({ command: config.command, args: ["mcp", "--run", "run_test", "--receipt-dir", receiptDir], cwd: config.cwd });
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
      else { writeFileSync(${JSON.stringify(capture)}, readFileSync(0)); writeFileSync(${JSON.stringify(capture)} + '.args', JSON.stringify(process.argv.slice(2))); process.stdout.write(JSON.stringify({ schema: 'runx.verify_verdict.v1', valid: false, receipt_id: null })); process.exitCode = 3; }`);
    const client = createRunxClient(config);
    await expect(client.harness.run({ skillPath: root, receiptDir })).resolves.toMatchObject({ status: "failed", assertion_errors: ["failed"] });
    await expect(client.receipts.verify({ receipt: '{"receipt":"exact"}' })).resolves.toMatchObject({ valid: false });
    expect(await readFile(capture, "utf8")).toBe('{"receipt":"exact"}');
    expect(JSON.parse(await readFile(`${capture}.args`, "utf8"))).toEqual(["verify", "--receipt", "-", "--trust", "home", "--json"]);
    await client.receipts.verify({ receipt: '{"receipt":"exact"}', trust: "configured" });
    expect(JSON.parse(await readFile(`${capture}.args`, "utf8"))).toEqual(["verify", "--receipt", "-", "--trust", "configured", "--json"]);
  });
  it("reads a secret's value by its name, and passes runx's refusal on in its words", async () => {
    const { root, config } = await setup();
    const capture = path.join(root, "secret.args");
    await writeFile(config.command, `#!${process.execPath}\nimport { writeFileSync } from 'node:fs';
      writeFileSync(${JSON.stringify(capture)}, JSON.stringify(process.argv.slice(2)));
      if (process.argv[4] === 'gemini') process.stdout.write('the-model-key\\n');
      else { process.stderr.write('runx secret: no secret is named ' + process.argv[4] + '\\n'); process.exitCode = 1; }`);
    const client = createRunxClient(config);
    await expect(client.secrets.read({ name: "gemini" })).resolves.toBe("the-model-key");
    expect(JSON.parse(await readFile(capture, "utf8"))).toEqual(["secret", "read", "gemini"]);
    await expect(client.secrets.read({ name: "unset" })).rejects.toThrow("no secret is named unset");
    await expect(client.secrets.read({ name: "--help" })).rejects.toThrow("positional");
  });
  it("reads a connector's exact registry package and refuses another shape", async () => {
    const { root, config } = await setup();
    const capture = path.join(root, "connector.args");
    const connector = { ref: "acme/ops-api", version: "sha-0123456789ab", content_digest: `sha256:${"a".repeat(64)}`, manifest_digest: `sha256:${"b".repeat(64)}`, connector_digest: `sha256:${"c".repeat(64)}`, manifest: { id: "ops-api" }, digests: {}, files: [{ path: "connector.yaml", content: "id: ops-api\n" }] };
    await writeFile(config.command, `#!${process.execPath}\nimport { writeFileSync } from 'node:fs';
      writeFileSync(${JSON.stringify(capture)}, JSON.stringify(process.argv.slice(2)));
      process.stdout.write(JSON.stringify({ status: 'success', registry: { action: 'connector_package', connector: ${JSON.stringify(connector)} } }));`);
    const client = createRunxClient(config);
    await expect(client.packages.materializeConnector({ connectorPath: "/connectors/ops-api" })).resolves.toMatchObject({ registry: { connector: { ref: "acme/ops-api" } } });
    expect(JSON.parse(await readFile(capture, "utf8"))).toEqual(["registry", "package", "/connectors/ops-api", "--json"]);
    await expect(client.packages.materializeConnector({ connectorPath: "--help" })).rejects.toThrow("positional");
    await writeFile(config.command, `#!${process.execPath}\nprocess.stdout.write(JSON.stringify({ status: 'success', registry: { action: 'connector_package', connector: { ref: 'acme/ops-api' } } }));`);
    await expect(client.packages.materializeConnector({ connectorPath: "/connectors/ops-api" })).rejects.toThrow("Invalid native connector package");
  });
  it("names why runx refused, from its failure envelope", async () => {
    const { config } = await setup();
    await writeFile(config.command, `#!${process.execPath}\nprocess.stdout.write(JSON.stringify({ status: 'failure', error: { code: 'registry_error', message: 'SKILL.md.runx is not supported' } })); process.stderr.write('ignored'); process.exitCode = 1;`);
    const client = createRunxClient(config);
    await expect(client.packages.materialize({ skillPath: "/skills/old" })).rejects.toThrow("Runx exited with code 1: SKILL.md.runx is not supported");
  });
  it("sends inputs on stdin, never in argv or a file", async () => {
    const { root, config, receiptDir } = await setup();
    const capture = path.join(root, "capture.json");
    await writeFile(config.command, `#!${process.execPath}\nimport { readFileSync, writeFileSync } from 'node:fs';
      writeFileSync(${JSON.stringify(capture)}, JSON.stringify({ args: process.argv.slice(2), inputs: JSON.parse(readFileSync(0, 'utf8')) }));
      process.stdout.write(JSON.stringify(${JSON.stringify(output("sealed", "completed", { closure: { disposition: "closed" } }))}));`);
    await createRunxClient(config).skills.run({ skillPath: "/skill", receiptDir, inputs: { "receipt-dir": "untrusted", nested: { value: 1 } } });
    const captured = JSON.parse(await readFile(capture, "utf8"));
    expect(captured.args.filter((arg: string) => arg === "--receipt-dir")).toHaveLength(1);
    expect(captured.args[captured.args.indexOf("--inputs") + 1]).toBe("-");
    expect(captured.inputs).toEqual({ "receipt-dir": "untrusted", nested: { value: 1 } });
  });
  it("gives runx its home as RUNX_HOME and keeps the host's HOME for the commands it runs", async () => {
    const { root, config, receiptDir } = await setup();
    const capture = path.join(root, "capture.json");
    await writeFile(config.command, `#!${process.execPath}\nimport { writeFileSync } from 'node:fs';
      writeFileSync(${JSON.stringify(capture)}, JSON.stringify({ home: process.env.HOME, runxHome: process.env.RUNX_HOME }));
      process.stdout.write(JSON.stringify(${JSON.stringify(output("sealed", "completed", { closure: { disposition: "closed" } }))}));`);
    const person = path.join(root, "person");
    await createRunxClient({ ...config, environment: { ...config.environment, HOME: person } }).skills.run({ skillPath: "/skill", receiptDir });
    expect(JSON.parse(await readFile(capture, "utf8"))).toEqual({ home: person, runxHome: config.home });
  });
  it("gives a run the host's authority, act and context as arguments", async () => {
    const { root, config, receiptDir } = await setup();
    const capture = path.join(root, "capture.json");
    await writeFile(config.command, `#!${process.execPath}\nimport { writeFileSync } from 'node:fs';
      writeFileSync(${JSON.stringify(capture)}, JSON.stringify(process.argv.slice(2)));
      process.stdout.write(JSON.stringify(${JSON.stringify(output("sealed", "completed", { closure: { disposition: "closed" } }))}));`);
    const runAuthority = { grants: { "ausca/inbox": "grant_ausca_1" }, actor_ref: { type: "principal", uri: "runx:principal:user_1" }, max_child_depth: 2 } as const;
    const act = { purpose: "reply to the thread" };
    const context = { current_context: [] };
    await createRunxClient(config).skills.run({ skillPath: "/skill", receiptDir, runAuthority, act, context });
    const args = JSON.parse(await readFile(capture, "utf8")) as string[];
    expect(JSON.parse(args[args.indexOf("--run-authority") + 1]!)).toEqual(runAuthority);
    expect(JSON.parse(args[args.indexOf("--act") + 1]!)).toEqual(act);
    expect(JSON.parse(args[args.indexOf("--context") + 1]!)).toEqual(context);
  });
  it("projects inspection readiness under the host's stated grants", async () => {
    const { root, config } = await setup();
    const capture = path.join(root, "capture.json");
    await writeFile(config.command, `#!${process.execPath}\nimport { readFileSync, writeFileSync } from 'node:fs';
      writeFileSync(${JSON.stringify(capture)}, JSON.stringify({args: process.argv.slice(2), input: JSON.parse(readFileSync(0, "utf8"))}));
      process.stdout.write(JSON.stringify({ schema: 'runx.skill.inspect.v1', status: 'ok', name: 'agent-inbox', skill_path: '/skill', manual_digest: 'sha256:${"e".repeat(64)}', package_digest: 'sha256:${"a".repeat(64)}', runners: [], operator_journeys: [] }));`);
    const runAuthority = { grants: { "ausca/inbox": "grant_ausca_1" } } as const;
    await createRunxClient(config).skills.inspect({ skillPath: "/skill", runner: "create", runAuthority, inputs: {target: "resource-1"} });
    const {args, input} = JSON.parse(await readFile(capture, "utf8")) as {args: string[]; input: unknown};
    expect(input).toEqual({target: "resource-1"});
    expect(args[args.indexOf("--inputs") + 1]).toBe("-");
    expect(args.slice(0, 4)).toEqual(["skill", "inspect", "/skill", "create"]);
    expect(JSON.parse(args[args.indexOf("--run-authority") + 1]!)).toEqual(runAuthority);
  });
  it("accepts sealed failure on exit 1 and rejects inconsistent exits", async () => {
    const { config, receiptDir } = await setup();
    const result = { exitCode: 1, stdout: JSON.stringify(output("sealed", "failed", { closure: { disposition: "failed" } })), stderr: "" };
    await expect(decodeSkill(result, { projectDir: config.projectDir, receiptDir, maxArtifactBytes: 10000 })).resolves.toMatchObject({ status: "sealed", outcome: "failed" });
    await expect(decodeSkill({ ...result, exitCode: 0 }, { projectDir: config.projectDir, receiptDir, maxArtifactBytes: 10000 })).rejects.toThrow("exit code disagrees");
  });
  it("reads the sealed run's receipt tree, each child the one its parent signed", async () => {
    const { config, receiptDir } = await setup();
    // As a runtime seals: the id addresses the body, then the digest commits the body with its id.
    const seal = (receipt: Record<string, unknown>) => {
      const addressed = { ...receipt, id: receiptCommitments(receipt as never).id };
      return { ...addressed, digest: receiptCommitments(addressed as never).digest };
    };
    const child = seal({ ...receiptFixture, created_at: "2026-02-02T00:00:00Z", lineage: { children: [] } });
    const tree = (children: readonly unknown[]) => seal({ ...receiptFixture, lineage: { children } });
    const link = { type: "receipt", uri: `runx:receipt:${child.id}`, locator: child.digest };
    const store = async (receipt: Record<string, unknown> & { id: string }) => writeFile(path.join(receiptDir, `${receipt.id.replace(":", "-")}.json`), JSON.stringify(receipt));
    const settings = { projectDir: config.projectDir, receiptDir, maxArtifactBytes: 100000 };
    const run = (root: { id: string }) => decodeSkill({ exitCode: 0, stdout: JSON.stringify(output("sealed", "completed", { closure: { disposition: "closed" }, result: {}, receipt_id: root.id })), stderr: "" }, settings);

    const root = tree([link]);
    await store(root);
    await expect(run(root)).rejects.toThrow(/ENOENT|no such file/u);
    await store(child);
    await expect(run(root)).resolves.toMatchObject({
      receiptTree: { root: { id: root.id }, descendants: [{ id: child.id }] },
    });
    const repointed = tree([{ ...link, locator: root.digest }]);
    await store(repointed);
    await expect(run(repointed)).rejects.toThrow("differs from the one");
    const twice = tree([link, link]);
    await store(twice);
    await expect(run(twice)).rejects.toThrow("appears twice");
    // A body changed under its id and digest is not the receipt they commit.
    await store({ ...root, created_at: "2026-01-01T00:00:00Z" });
    await expect(run(root)).rejects.toThrow("not the content address of its body");
  });
  it("hydrates spilled requests and rejects digest drift", async () => {
    const { config, receiptDir } = await setup();
    const ref = await artifact(config.projectDir, inputRequest);
    const set = await artifact(config.projectDir, [{ id: inputRequest.id, kind: "input", request_digest: ref.digest, artifact_ref: ref }], "request-set");
    const result = { exitCode: 2, stdout: JSON.stringify(output("needs_agent", "deferred", { requests: [], request_set_ref: set })), stderr: "" };
    const settings = { projectDir: config.projectDir, receiptDir, maxArtifactBytes: 10000 };
    await expect(decodeSkill(result, settings)).resolves.toMatchObject({ requests: [{ address: inputRequest.id, request_digest: ref.digest, request: inputRequest }] });
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
    // As the CLI writes it: the request set's summaries carry each digest.
    const requestRef = await artifact(config.projectDir, inputRequest);
    const set = await artifact(config.projectDir, [{ id: inputRequest.id, kind: "input", request_digest: requestRef.digest, artifact_ref: requestRef }], "request-set");
    const result = { exitCode: 2, stdout: JSON.stringify(output("needs_agent", "deferred", { requests: [], request_set_ref: set, artifact_refs: [ref] })), stderr: "" };
    const settings = { projectDir: config.projectDir, receiptDir, maxArtifactBytes: 10000 };
    await expect(decodeSkill(result, settings)).resolves.toMatchObject({ requests: [{ address: inputRequest.id, request_digest: requestRef.digest, request: inputRequest }], result: "large payload" });
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
