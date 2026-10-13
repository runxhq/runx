import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { beforeAll, describe, expect, it } from "vitest";

import { RUNX_SCOPE_GRANT_POLICY, scopeGrantAllows } from "@runxhq/contracts";
import { rustExampleBinary } from "../scripts/lib/rust-example.mjs";
import { evaluateRustKernelInputSync } from "../scripts/rust-kernel-eval.js";

const policies = Object.values(RUNX_SCOPE_GRANT_POLICY);
const workspaceRoot = fileURLToPath(new URL("..", import.meta.url));
const namespaces = ["", "repo", "repository", "repo:admin", "α", "🧭", "repo*", " ", "\0"];
const segments = ["", "read", "write", "*", "read:child", "🛠️", "\n"];
const grants = [...new Set([
  "*", "**", ":", ":*", "repo:**", "repo:read*",
  ...namespaces.flatMap((namespace) => [namespace, `${namespace}:*`, `${namespace}*`, `${namespace}:read`]),
])];
const requests = [...new Set([
  "*", "repo", ":read", ":*", "repo:*", "repo:read*",
  ...namespaces.flatMap((namespace) => segments.map((segment) => `${namespace}:${segment}`)),
])];
const cases = policies.flatMap((policy) => grants.flatMap((grantedScope) => requests.map((requestedScope) => ({
  kind: "policy.scopeGrantAllows",
  grantedScope,
  requestedScope,
  policy,
}))));

let nativeResults: readonly { status: "success"; result: { kind: "output"; value: boolean } }[];

beforeAll(() => {
  const result = spawnSync(rustExampleBinary("runx-core", "kernel_eval_batch"), [], {
    cwd: workspaceRoot,
    encoding: "utf8",
    input: JSON.stringify(cases),
    maxBuffer: 8 * 1024 * 1024,
    timeout: 30_000,
  });
  expect(result.error).toBeUndefined();
  expect(result.status, result.stderr).toBe(0);
  nativeResults = JSON.parse(result.stdout);
  expect(nativeResults).toHaveLength(cases.length);
});

function rustAllows(grantedScope: string, requestedScope: string, policy: typeof policies[number]): unknown {
  return evaluateRustKernelInputSync({ kind: "policy.scopeGrantAllows", grantedScope, requestedScope, policy });
}

describe("Rust/TypeScript scope policy differential", () => {
  it("refuses expansion of an empty namespace while preserving opaque exact equality", () => {
    for (const policy of policies) {
      expect(scopeGrantAllows(":*", ":read", policy)).toBe(false);
      expect(rustAllows(":*", ":read", policy)).toBe(false);
      expect(scopeGrantAllows(":*", ":*", policy)).toBe(true);
      expect(rustAllows(":*", ":*", policy)).toBe(true);
    }
    expect(rustAllows("repo:*", "repo:read", "delegated")).toBe(true);
    expect(rustAllows("repo:*", "repo:admin:keys", "delegated")).toBe(false);
    expect(rustAllows("*", "repo:read", "delegated")).toBe(false);
    expect(rustAllows("*", "repo:read", "trusted")).toBe(true);
  });

  for (const [policyIndex, policy] of policies.entries()) {
    it(`matches the generated lexical corpus under ${policy}`, () => {
      expect(grants.length * requests.length).toBeGreaterThan(1_000);
      const offset = policyIndex * grants.length * requests.length;
      for (let index = offset; index < offset + grants.length * requests.length; index += 1) {
        const { grantedScope, requestedScope } = cases[index]!;
        const expected = scopeGrantAllows(grantedScope, requestedScope, policy);
        expect(nativeResults[index], JSON.stringify({ grantedScope, requestedScope, policy })).toEqual({ status: "success", result: { kind: "output", value: expected } });
      }
    });
  }
});
