import { mkdtempSync, rmSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import { describe, expect, it } from "vitest";

import { checkExecutionSource, checkExecutionSplit } from "../scripts/runtime-architecture/phases.mjs";

describe("execution ownership check", () => {

  it("fails when ownership files disappear instead of silently accepting a stale target", () => {
    const root = mkdtempSync(path.join(os.tmpdir(), "runx-execution-owner-"));
    try {
      const findings: string[] = [];
      checkExecutionSplit(findings, root);
      expect(findings).toHaveLength(3);
      expect(findings.every(finding => finding.startsWith("Missing current execution owner"))).toBe(true);
    } finally {
      rmSync(root, { recursive: true });
    }
  });

  it.each([
    ["step_handlers.rs", "mod output; mod host_resolution; host.request_approval();"],
    ["step_handlers/output.rs", "use crate::adapter::SkillAdapter as Executor;"],
    ["step_handlers/host_resolution.rs", "seal_step(receipt);"],
  ])("rejects responsibility crossing in %s", (owner, source) => {
    expect(checkExecutionSource(owner, source)).toEqual([expect.stringContaining("responsibility boundary")]);
  });

  it("preserves the concrete admit/invoke/seal owner and typed projection helpers", () => {
    expect(checkExecutionSource("step_handlers.rs", "mod output; mod host_resolution; prepare_effect_execution(); SkillAdapter::invoke(); seal_step();")).toEqual([]);
    expect(checkExecutionSource("step_handlers/output.rs", "use crate::adapter::InvocationOutput; use crate::execution::output_projection::StepOutputProjection;")).toEqual([]);
    expect(checkExecutionSource("step_handlers/host_resolution.rs", "use crate::host::Host; resolver.request_approval(host);")).toEqual([]);
    expect(checkExecutionSource("step_handlers.rs", "")).toHaveLength(2);
  });
});
