import { readFile, readdir } from "node:fs/promises";

import { describe, expect, it } from "vitest";

import { scopeGrantAllows, type ScopeGrantPolicy } from "./scope-policy.js";

interface ScopeGrantFixture {
  readonly expected: { readonly kind: "output"; readonly value: boolean };
  readonly input: {
    readonly kind: "policy.scopeGrantAllows";
    readonly grantedScope: string;
    readonly requestedScope: string;
    readonly policy: ScopeGrantPolicy;
  };
  readonly name: string;
}

const policyFixtureRoot = new URL("../../../fixtures/kernel/policy/", import.meta.url);

describe("runx scope grant policy", () => {
  it("matches the Rust-owned kernel fixtures", async () => {
    const fixtureNames = (await readdir(policyFixtureRoot))
      .filter((name) => name.startsWith("scope-grant-") && name.endsWith(".json"))
      .sort();
    expect(fixtureNames.length).toBeGreaterThan(0);

    for (const fixtureName of fixtureNames) {
      const fixture = JSON.parse(
        await readFile(new URL(fixtureName, policyFixtureRoot), "utf8"),
      ) as ScopeGrantFixture;
      expect(fixture.expected.kind, fixture.name).toBe("output");
      expect(
        scopeGrantAllows(
          fixture.input.grantedScope,
          fixture.input.requestedScope,
          fixture.input.policy,
        ),
        fixture.name,
      ).toBe(fixture.expected.value);
    }
  });
});
