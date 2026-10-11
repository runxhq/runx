import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";

import { isRegistryVersion, REGISTRY_VERSION_PATTERN } from "./registry-version.js";

const fixture = JSON.parse(
  readFileSync(new URL("../../../fixtures/contracts/registry-version/registry-version-v1.vectors.json", import.meta.url), "utf8"),
) as { readonly pattern: string; readonly cases: readonly { readonly name: string; readonly input: string; readonly expectation: "valid" | "invalid" }[] };

describe("registry version grammar", () => {
  it("matches the vectors the Rust contract holds to", () => {
    expect(REGISTRY_VERSION_PATTERN).toBe(fixture.pattern);
    for (const testCase of fixture.cases) {
      expect(isRegistryVersion(testCase.input), testCase.name).toBe(testCase.expectation === "valid");
    }
  });
});
