import path from "node:path";

import { describe, expect, it } from "vitest";

import {
  collectKernelFixtureFiles,
  evaluateKernelFixtureInput,
  isRunnerKernelFixture,
  normalizeForFixture,
  readKernelFixture,
  validateKernelFixture,
  validateJsonSchemaValue,
} from "../scripts/generate-kernel-parity-fixtures.js";

describe("kernel parity fixtures", () => {
  it("match the current trusted-kernel behavior", async () => {
    const fixtureFiles = await collectKernelFixtureFiles();
    expect(fixtureFiles.length).toBeGreaterThan(0);

    for (const fixtureFile of fixtureFiles) {
      const fixture = await readKernelFixture(fixtureFile);
      expect(fixture.name, fixtureFile).toBe(path.basename(fixtureFile, ".json"));
      const relativeFixturePath = path.relative(path.join(process.cwd(), "fixtures", "kernel"), fixtureFile);
      expect(relativeFixturePath.startsWith(`runner${path.sep}`), fixture.name).toBe(isRunnerKernelFixture(fixture));

      const validation = await validateKernelFixture(fixture);
      expect(validation.errors, fixture.name).toEqual([]);

      if (fixture.expected.kind === "output") {
        expect(normalizeForFixture(evaluateKernelFixtureInput(fixture.input)), fixture.name).toEqual(fixture.expected.value);
      } else {
        let threw = false;
        try {
          evaluateKernelFixtureInput(fixture.input);
        } catch (error) {
          threw = true;
          expect(error, fixture.name).toMatchObject({
            code: fixture.expected.code,
            message: fixture.expected.message ?? expect.any(String),
          });
        }
        expect(threw, fixture.name).toBe(true);
      }
    }
  });

  it("fails closed when fixture schemas use unsupported JSON Schema keywords", () => {
    expect(validateJsonSchemaValue({ enum: ["allowed"] }, "allowed", "")).toEqual([
      {
        path: "/enum",
        message: "unsupported JSON Schema keyword 'enum'",
      },
    ]);
  });

  it("reports the failing oneOf schema branches", () => {
    expect(
      validateJsonSchemaValue(
        {
          oneOf: [
            { properties: { kind: { const: "alpha" } }, required: ["kind"], type: "object" },
            { properties: { count: { type: "number" } }, required: ["count"], type: "object" },
          ],
        },
        { kind: "beta" },
        "/input",
      ),
    ).toEqual([
      {
        path: "/input",
        message:
          "value matched 0 schema branches; expected exactly one (branch 0: /input/kind value must equal \"alpha\"; branch 1: /input/count required property is missing)",
      },
    ]);
  });

  it("applies sibling constraints after a oneOf branch matches", () => {
    expect(
      validateJsonSchemaValue(
        {
          oneOf: [{ const: "ok" }],
          type: "object",
        },
        "ok",
        "",
      ),
    ).toEqual([
      {
        path: "/",
        message: "value must be object",
      },
    ]);
  });

  it("rejects non-canonical fixture schema references", async () => {
    const validation = await validateKernelFixture({
      $schema: "../../../schema/state-machine.schema.json" as "../schema/state-machine.schema.json",
      name: "invalid-schema-ref",
      input: { kind: "state-machine.createSingleStepState", stepId: "lint" },
      expected: { kind: "output", value: {} },
    });

    expect(validation.errors).toEqual([
      "fixture.$schema: unsupported kernel fixture schema ref '../../../schema/state-machine.schema.json'",
    ]);
  });

  it("rejects fixture schema references that inherit from Object.prototype", async () => {
    const validation = await validateKernelFixture({
      $schema: "toString" as "../schema/state-machine.schema.json",
      name: "prototype-schema-ref",
      input: { kind: "state-machine.createSingleStepState", stepId: "lint" },
      expected: { kind: "output", value: {} },
    });

    expect(validation.errors).toEqual([
      "fixture.$schema: unsupported kernel fixture schema ref 'toString'",
    ]);
  });

  it("requires runner ingestion fixtures to use the runner prefix", async () => {
    const validation = await validateKernelFixture({
      $schema: "../schema/policy.schema.json",
      name: "missing-grant-runner-error",
      input: {
        kind: "policy.admitGraphStepScopes",
        request: { stepId: "read", requestedScopes: ["repo:read"] },
      },
      expected: {
        kind: "error",
        code: "kernel.fixture.evaluation_failed",
        message: "kernel fixture evaluation failed",
      },
    });

    expect(validation.errors).toContain("fixture.name: runner ingestion fixtures must use the 'runner-' prefix");
  });

  it("reserves the runner prefix for fixture-runner ingestion errors", async () => {
    const validation = await validateKernelFixture({
      $schema: "../schema/policy.schema.json",
      name: "runner-graph-scope-denies-widening",
      input: {
        kind: "policy.admitGraphStepScopes",
        request: {
          stepId: "deploy",
          requestedScopes: ["deployments:write"],
          grant: { scopes: ["checks:read"] },
        },
      },
      expected: {
        kind: "output",
        value: {
          status: "deny",
        },
      },
    });

    expect(validation.errors).toContain(
      "fixture.name: only kernel.fixture.evaluation_failed error fixtures may use the 'runner-' prefix",
    );
  });

  it("reports Object.prototype property collisions as additional properties", () => {
    expect(
      validateJsonSchemaValue(
        {
          additionalProperties: false,
          properties: {},
          type: "object",
        },
        JSON.parse('{"toString":"not allowed"}') as unknown,
        "",
      ),
    ).toEqual([
      {
        path: "/toString",
        message: "additional property is not allowed",
      },
    ]);
  });

  it("preserves source error details on fixture oracle failures", () => {
    try {
      evaluateKernelFixtureInput({
        kind: "policy.admitGraphStepScopes",
        request: { stepId: "read", requestedScopes: ["repo:read"] },
      });
      throw new Error("expected fixture oracle failure");
    } catch (error) {
      expect(error).toMatchObject({
        code: "kernel.fixture.evaluation_failed",
        sourceErrorMessage: expect.any(String),
        sourceErrorName: "RustKernelEvalError",
      });
    }
  });
});
