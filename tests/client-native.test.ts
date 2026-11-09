import { cp, mkdir, mkdtemp, rm } from "node:fs/promises";
import { createServer } from "node:http";
import type { AddressInfo } from "node:net";
import os from "node:os";
import path from "node:path";

import { afterEach, describe, expect, it } from "vitest";

import { createRunxClient, executionClosureFromInspection } from "../packages/client/src/index.js";
import { runxBinary } from "./native-test-environment.js";

// The client's commands against the native CLI itself, so what the client
// sends is held to the CLI's grammar, not to a fake's.
describe("the client against the native CLI", () => {
  const roots: string[] = [];
  afterEach(async () => {
    await Promise.all(roots.splice(0).map((root) => rm(root, { recursive: true, force: true })));
  });

  it("names a method as <ref>#<method>, and the CLI inspects that method", async () => {
    const root = await mkdtemp(path.join(os.tmpdir(), "runx-client-native-"));
    roots.push(root);
    const [cwd, projectDir, home] = ["workspace", "project", "home"].map((name) => path.join(root, name)) as [string, string, string];
    await Promise.all([cwd, projectDir, home].map((dir) => mkdir(dir)));
    const client = createRunxClient({ command: runxBinary, cwd, projectDir, home, environment: { PATH: process.env.PATH } });
    const inspection = await client.skills.inspect({ skillPath: path.resolve("skills/agent"), method: "write" });
    expect(inspection.method?.name).toBe("write");
  });

  // Cloud's layout: a bare workspace, project and home, none of them a
  // workspace by its markers. A host states its run's authority, its hosted
  // grant by connector, for each invocation; it needs no workspace and saves
  // nothing.
  it("inspects and runs under a host's hosted grant in Cloud's bare layout", async () => {
    const root = await mkdtemp(path.join(os.tmpdir(), "runx-client-boundary-"));
    roots.push(root);
    const [cwd, projectDir, home] = ["workspace", "project", "home"].map((name) => path.join(root, name)) as [string, string, string];
    await Promise.all([cwd, projectDir, home].map((dir) => mkdir(dir)));
    const plugin = path.join(root, "plugin");
    await cp(path.resolve("fixtures/journeys/hosted-records"), plugin, { recursive: true });
    const skillPath = path.join(plugin, "skills/records");

    // A loopback runx Connect: it authenticates the operator, lists the one
    // grant, bound to the connector revision the skill binds, and answers
    // each provider call, recording the grant it was sent under.
    const sent: Array<Record<string, unknown>> = [];
    let grants: unknown[] = [];
    const connect = createServer((request, response) => {
      let body = "";
      request.on("data", (chunk) => { body += String(chunk); });
      request.on("end", () => {
        const answer = (value: unknown, status = 200) => {
          response.writeHead(status, { "content-type": "application/json" });
          response.end(JSON.stringify(value));
        };
        if (request.method === "GET" && request.url === "/v1/me") return answer({ status: "success", principal: { principal_id: "operator-1" } });
        if (request.method === "GET" && request.url === "/v1/grants") return answer({ status: "success", grants });
        if (request.method === "POST" && request.url === "/v1/provider-operations") {
          const call = JSON.parse(body) as Record<string, unknown>;
          sent.push(call);
          return answer({ status: "success", access: call.access, provider: "fixture/records", operation: call.operation, target: call.target, result: { records: [] }, readback_uri: "runx:provider-readback:records-1" });
        }
        return answer({ status: "error", error: { code: "not_found" } }, 404);
      });
    });
    await new Promise<void>((listening) => connect.listen(0, "127.0.0.1", listening));
    try {
      const url = `http://127.0.0.1:${(connect.address() as AddressInfo).port}`;
      const client = createRunxClient({
        command: runxBinary, cwd, projectDir, home,
        environment: { PATH: process.env.PATH, RUNX_PUBLIC_API_BASE_URL: url, RUNX_PUBLIC_API_TOKEN: "rxk_operator", RUNX_PUBLIC_API_ALLOW_PRIVATE_NETWORK: "1" },
      });
      const unbound = await client.skills.inspect({ skillPath, method: "list" });
      const binding = executionClosureFromInspection(unbound).connectorBindings.find((bound) => bound.connector === "fixture/records");
      expect(binding).toBeDefined();
      grants = [{ grant_id: "grant_records_read", provider: "fixture/records", connector_binding: binding, status: "active", credential_material_bound: true, scopes: ["records.read", "records.summary.read"] }];
      const runAuthority = { grants: { "fixture/records": "grant_records_read" } };

      const inspected = await client.skills.inspect({ skillPath, method: "list", runAuthority });
      expect(inspected.readiness?.status, JSON.stringify(inspected)).toBe("ready");

      const ran = await client.skills.run({ skillPath, method: "list", runAuthority, receiptDir: path.join(root, "receipts") });
      expect(ran.status, JSON.stringify(ran)).toBe("sealed");
      expect(ran.outcome, JSON.stringify(ran)).toBe("completed");
      expect(sent.length).toBeGreaterThan(0);
      expect(sent.every((call) => call.grant_id === "grant_records_read"), JSON.stringify(sent)).toBe(true);
      // The project's store records the run.
      const read = await client.runs.read({ runId: ran.run_id as string });
      expect(read.run.run_id).toBe(ran.run_id);
      expect(read.run.outcome).toBe("completed");
    } finally {
      await new Promise<void>((closed) => connect.close(() => closed()));
    }
  });
});
