import { existsSync } from "node:fs";
import { readFile } from "node:fs/promises";
import path from "node:path";

import { describe, expect, it } from "vitest";

import { validateRunnerManifestYaml } from "../scripts/lib/native-parser.mjs";

const hostedPaymentSkills = [
  "charge",
  "refund",
  "settle-invoice",
  "spend",
  "stripe-pay",
  "x402-pay",
] as const;

const localPaymentSimulators = ["mock-charge", "mock-pay", "mock-refund"] as const;

const retiredPackages = [
  "mpp-charge",
  "mpp-pay",
  "mpp-refund",
  "stripe-charge",
  "stripe-refund",
] as const;

const retiredStages = [
  "charge/graph/charge-challenge",
  "charge/graph/charge-price",
  "charge/graph/charge-verify",
  "spend/graph/pay-fulfill-rail",
  "spend/graph/pay-quote",
  "spend/graph/pay-recover",
  "spend/graph/pay-reserve",
] as const;

describe("payment ownership boundary", () => {
  it("keeps the private payment runtime out of OSS", async () => {
    expect(existsSync(path.resolve("crates/runx-pay"))).toBe(false);
    for (const skill of retiredPackages) {
      expect(existsSync(path.resolve("skills", skill)), skill).toBe(false);
    }
    for (const stage of retiredStages) {
      expect(existsSync(path.resolve("skills", stage)), stage).toBe(false);
    }

    const runtime = await readFile(path.resolve("crates/runx-cli/src/runtime.rs"), "utf8");
    expect(runtime).not.toMatch(/PaymentEffect|PaymentFinality|EffectState|runx_pay/);
  });

  it("keeps real payment effects behind hosted provider contracts", async () => {
    for (const skill of hostedPaymentSkills) {
      const source = await readFile(path.resolve("skills", skill, "X.yaml"), "utf8");
      const manifest = validateRunnerManifestYaml(source).raw.document as {
        readonly catalog?: Record<string, unknown>;
        readonly runners: Record<string, {
          readonly type: string;
          readonly graph: { readonly steps: readonly {
            readonly tool?: string;
            readonly operation?: string;
            readonly idempotency_key?: unknown;
            readonly inputs?: { readonly approval?: unknown };
            readonly run?: { readonly type?: string };
          }[] };
        }>;
      };

      expect(manifest.catalog?.runtime_path, `${skill} runtime`).toBe("hosted");
      expect(manifest.catalog?.requires_adapter, `${skill} adapter`).toBe(true);
      expect(manifest.catalog?.approval, `${skill} approval`).toBe("required");
      expect(Object.keys(manifest.runners).length, `${skill} runners`).toBeGreaterThan(0);
      const operations: string[] = [];
      for (const [name, runner] of Object.entries(manifest.runners)) {
        expect(runner.type, `${skill}/${name} composition`).toBe("graph");
        for (const step of runner.graph.steps) {
          // Pure preparation/readback computation can accompany provider calls.
          // It must use the isolated module lane, never a host CLI process.
          if (step.run) expect(step.run.type, `${skill}/${name} local computation`).toBe("javascript");
          if (step.operation) operations.push(step.operation);
          // The payment itself is a connector write; its readback is the
          // matching `.read` operation.
          if (step.operation && !step.operation.endsWith(".read")) {
            expect(step.idempotency_key, `${skill}/${name} retry identity`).toBeTypeOf("string");
            expect(step.inputs?.approval, `${skill}/${name} exact-act approval`).toMatchObject({
              type: expect.any(String), reason: expect.any(String),
            });
          }
        }
      }
      if (skill === "stripe-pay") {
        expect(source, `${skill} canonical hosted delegate`).toContain("skill: ../spend");
      } else {
        expect(operations.some((operation) => !operation.endsWith(".read")), `${skill} hosted mutation`).toBe(true);
        expect(operations.some((operation) => operation.endsWith(".read")), `${skill} hosted readback`).toBe(true);
      }
      expect(source, `${skill} retired native planner`).not.toMatch(/tool: payment\.(?:quote|reserve|fulfill|recover|charge_|refund_plan|invoice_plan)/);
    }
  });

  it("keeps deterministic mocks local, explicit, and incapable of moving money", async () => {
    for (const skill of localPaymentSimulators) {
      const profile = await readFile(path.resolve("skills", skill, "X.yaml"), "utf8");
      const implementation = await readFile(path.resolve("skills", skill, `${skill}.mjs`), "utf8");
      const manifest = validateRunnerManifestYaml(profile).raw.document as {
        readonly catalog?: Record<string, unknown>;
      };

      expect(manifest.catalog?.visibility, `${skill} visibility`).toBe("internal");
      expect(manifest.catalog?.role, `${skill} role`).toBe("harness-fixture");
      expect(manifest.catalog?.requires_adapter, `${skill} adapter`).toBe(false);
      expect(profile, `${skill} local JavaScript`).toContain("type: javascript");
      expect(profile, `${skill} provider access`).not.toMatch(/provider\.(?:mutate|read)/);
      expect(implementation, `${skill} simulated result`).toContain('status: "simulated"');
      expect(implementation, `${skill} no money movement`).toContain("money_moved: false");
    }
  });
});
