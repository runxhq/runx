import { spawnSync } from "node:child_process";
import { mkdtemp, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";

import { beforeAll, describe, expect, it } from "vitest";

import {
  loadCliFeatureParityContract,
  type CliFeatureParityContract,
  type OracleCase,
} from "./cli-feature-parity-contract.js";
import { ensureRunxBinary, kernelTestEnv, runxBinary } from "./host-protocol-test-utils.js";
import { appendLedgerEntries, createRunEventEntry } from "./ledger-fixtures.js";

describe("CLI feature parity matrix", () => {
  let contract: CliFeatureParityContract;

  beforeAll(() => {
    ensureRunxBinary();
    contract = loadCliFeatureParityContract(runxBinary);
  });

  it("covers every command with at least one oracle case", () => {
    const casesByCommand = new Map<string, OracleCase[]>();
    const caseIds = new Set<string>();

    for (const testCase of contract.cases) {
      expect(caseIds.has(testCase.id), testCase.id).toBe(false);
      caseIds.add(testCase.id);
      const cases = casesByCommand.get(testCase.commandId) ?? [];
      cases.push(testCase);
      casesByCommand.set(testCase.commandId, cases);
    }

    for (const command of contract.commands) {
      expect(command.parity.surfaces.length).toBeGreaterThan(0);
      expect(casesByCommand.get(command.name)?.length ?? 0).toBeGreaterThan(0);
    }
  });

  it("connects every runtime surface to a command and oracle case", () => {
    const commandIds = new Set(contract.commands.map((command) => command.name));
    const provenSurfaces = new Set(contract.cases.flatMap((testCase) => testCase.proves));

    for (const surface of contract.surfaces) {
      expect(surface.coveredBy.length, surface.id).toBeGreaterThan(0);
      for (const commandId of surface.coveredBy) {
        expect(commandIds.has(commandId), `${surface.id}:${commandId}`).toBe(true);
      }
      expect(provenSurfaces.has(surface.id), surface.id).toBe(true);
    }
  });

  it("executes deterministic oracle cases against the native CLI", async () => {
    const executableCases = contract.cases.filter((testCase) => testCase.mode === "execute");

    for (const testCase of executableCases) {
      const tempDir = await mkdtemp(path.join(os.tmpdir(), `runx-cli-parity-${testCase.id}-`));

      try {
        const receiptDir = path.join(tempDir, "receipts");
        await prepareOracleFixtures(testCase, receiptDir);
        const argv = (testCase.argv ?? []).map((arg) =>
          arg === "$FIXTURE_RECEIPTS" ? receiptDir : arg,
        );
        const result = spawnSync(runxBinary, argv, {
          cwd: process.cwd(),
          encoding: "utf8",
          env: {
            ...kernelTestEnv(process.env),
            RUNX_CWD: process.cwd(),
            RUNX_HOME: path.join(tempDir, "home"),
            RUNX_RECEIPT_DIR: receiptDir,
            RUNX_BANNER: "0",
            ...testCase.env,
          },
        });
        if (result.error) {
          throw result.error;
        }
        const stdout = result.stdout ?? "";
        const stderr = result.stderr ?? "";
        const exitCode = result.status ?? 1;

        expect(exitCode, testCase.id).toBe(testCase.expectedExitCode);
        for (const expected of testCase.stdoutIncludes ?? []) {
          expect(stdout, testCase.id).toContain(expected);
        }
        for (const expected of testCase.stderrIncludes ?? []) {
          expect(stderr, testCase.id).toContain(expected);
        }
        if (testCase.expectJson) {
          expect(() => JSON.parse(stdout), testCase.id).not.toThrow();
        }
      } finally {
        await rm(tempDir, { recursive: true, force: true });
      }
    }
  }, 20_000);
});

async function prepareOracleFixtures(testCase: OracleCase, receiptDir: string): Promise<void> {
  if (!testCase.argv?.includes("$FIXTURE_RECEIPTS")) {
    return;
  }
  if (testCase.id === "history.execute") {
    await appendLedgerEntries({
      receiptDir,
      runId: "gx_needs_agent_oracle",
      entries: [
        createRunEventEntry({
          runId: "gx_needs_agent_oracle",
          producer: { skill: "sourcey", runner: "graph" },
          kind: "run_started",
          status: "started",
          createdAt: "2026-04-28T01:00:00.000Z",
        }),
        createRunEventEntry({
          runId: "gx_needs_agent_oracle",
          stepId: "discover",
          producer: { skill: "sourcey", runner: "graph" },
          kind: "step_waiting_resolution",
          status: "waiting",
          detail: {
            request_ids: ["agent_task.test-step.output"],
            resolution_kinds: ["agent_act"],
            step_ids: ["discover"],
            step_labels: ["inspect repo"],
            inputs: {},
            selected_runner: "agent-task",
          },
          createdAt: "2026-04-28T01:00:00.000Z",
        }),
      ],
    });
  }
}
