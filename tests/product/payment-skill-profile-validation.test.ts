import { existsSync } from "node:fs";
import { readFile } from "node:fs/promises";
import path from "node:path";

import { describe, expect, it } from "vitest";

import { validateRunnerManifestYaml } from "../../scripts/lib/native-parser.mjs";

const hostedPaymentSkills = [
  "charge",
  "pay",
  "refund",
  "invoice-pay",
] as const;

const localPaymentSimulators = ["mock-charge", "mock-pay", "mock-refund"] as const;

describe("payment ownership boundary", () => {
  it("keeps the private payment runtime out of OSS", async () => {
    expect(existsSync(path.resolve("crates/runx-pay"))).toBe(false);

    const runtime = await readFile(path.resolve("crates/runx-cli/src/runtime.rs"), "utf8");
    expect(runtime).not.toMatch(/PaymentEffect|PaymentFinality|EffectState|runx_pay/);
  });

  it("keeps real payment effects behind hosted provider contracts", async () => {
    for (const skill of hostedPaymentSkills) {
      const source = await readFile(path.resolve("skills", skill, "run.yaml"), "utf8");
      const manifest = validateRunnerManifestYaml(source).raw.document as {
        readonly catalog?: Record<string, unknown>;
        readonly runners: Record<string, {
          readonly steps: readonly {
            readonly tool?: string | { readonly module?: string; readonly command?: string };
            readonly operation?: string;
            readonly idempotency_key?: unknown;
            readonly approval?: unknown;
            readonly expected_result?: unknown;
          }[];
        }>;
      };

      expect(manifest.catalog?.approval, `${skill} approval`).toBe("required");
      expect(Object.keys(manifest.runners).length, `${skill} runners`).toBeGreaterThan(0);
      let approved = 0;
      let readbacks = 0;
      for (const [name, runner] of Object.entries(manifest.runners)) {
        for (const step of runner.steps) {
          // Pure preparation/readback computation can accompany provider calls.
          // It must use the isolated worker lane, never a host process.
          if (typeof step.tool === "object") {
            expect(step.tool.command, `${skill}/${name} local computation`).toBeUndefined();
            expect(step.tool.module, `${skill}/${name} local computation`).toBeTypeOf("string");
          }
          if (!step.operation) continue;
          // The payment itself is approved as the exact act and carries its
          // retry identity; its readback states what it must read back. The
          // kernel refuses a write without a key and a read with an approval.
          if (step.approval !== undefined) {
            approved += 1;
            expect(step.idempotency_key, `${skill}/${name} retry identity`).toBeTypeOf("string");
            expect(step.approval, `${skill}/${name} exact-act approval`).toMatchObject({
              type: expect.any(String), reason: expect.any(String),
            });
          }
          if (step.expected_result !== undefined) readbacks += 1;
        }
      }
      if (skill === "invoice-pay") {
        // It checks the invoice, then pays it through `pay`.
        expect(source, `${skill} pays through pay`).toContain("skill: ../pay");
      } else {
        expect(approved, `${skill} approved hosted mutation`).toBeGreaterThan(0);
        expect(readbacks, `${skill} hosted readback`).toBeGreaterThan(0);
      }
      expect(source, `${skill} retired native planner`).not.toMatch(/tool: payment\.(?:quote|reserve|fulfill|recover|charge_|refund_plan|invoice_plan)/);
    }
  });

  it("keeps deterministic mocks local, explicit, and incapable of moving money", async () => {
    for (const skill of localPaymentSimulators) {
      const profile = await readFile(path.resolve("fixtures/skills", skill, "run.yaml"), "utf8");
      const implementation = await readFile(path.resolve("fixtures/skills", skill, `${skill}.mjs`), "utf8");
      const manifest = validateRunnerManifestYaml(profile).raw.document as {
        readonly catalog?: Record<string, unknown>;
      };

      expect(manifest.catalog?.visibility, `${skill} visibility`).toBe("internal");
      expect(manifest.catalog?.role, `${skill} role`).toBe("harness-fixture");
      expect(profile, `${skill} local JavaScript`).toMatch(/tool: \{ module: /);
      expect(profile, `${skill} provider access`).not.toMatch(/provider\.(?:mutate|read)/);
      expect(implementation, `${skill} simulated result`).toContain('status: "simulated"');
      expect(implementation, `${skill} no money movement`).toContain("money_moved: false");
    }
  });
});
