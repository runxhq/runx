import { Ajv2020 } from "ajv/dist/2020.js";
import { beforeEach, describe, expect, it, vi } from "vitest";

const stringValue = {
  type: "object",
  properties: { value: { type: "string" } },
  required: ["value"],
};

describe("contract validator cache", () => {
  beforeEach(() => {
    vi.resetModules();
    vi.restoreAllMocks();
  });

  it("compiles one immutable schema once across repeated validation", async () => {
    const compile = vi.spyOn(Ajv2020.prototype, "compile");
    const { validateContractSchema } = await import("./internal.js");
    const schema = { ...stringValue };

    expect(validateContractSchema(schema, { value: "first" }, "cache")).toEqual({ value: "first" });
    expect(validateContractSchema(schema, { value: "second" }, "cache")).toEqual({ value: "second" });
    expect(() => validateContractSchema(schema, { value: 3 }, "cache")).toThrow();
    expect(compile).toHaveBeenCalledTimes(1);
  });

  it("isolates validators compiled with different reference sets", async () => {
    const compile = vi.spyOn(Ajv2020.prototype, "compile");
    const { validateContractSchema } = await import("./internal.js");
    const reference = { ...stringValue, $id: "https://schemas.runx.ai/test/cache-reference.json" };
    const otherReference = {
      type: "object",
      properties: { value: { type: "number" } },
      required: ["value"],
      $id: "https://schemas.runx.ai/test/cache-other-reference.json",
    };
    const schema = { $ref: reference.$id };

    expect(validateContractSchema(schema, { value: "ok" }, "cache", [reference])).toEqual({ value: "ok" });
    expect(validateContractSchema(schema, { value: "again" }, "cache", [reference])).toEqual({ value: "again" });
    expect(() => validateContractSchema(schema, { value: "no" }, "cache", [otherReference]))
      .toThrow();
    expect(compile).toHaveBeenCalledTimes(2);
  });
});
