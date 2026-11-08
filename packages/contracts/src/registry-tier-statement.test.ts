import { createPublicKey, verify } from "node:crypto";
import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";

import { registryTierStatementPayload, validateRegistryTierStatement } from "./registry-tier-statement.js";

const fixture = JSON.parse(readFileSync(
  new URL("../../../fixtures/contracts/registry-tier-statement/registry-tier-statement-v1.vectors.json", import.meta.url),
  "utf8",
)) as {
  readonly public_key_base64: string;
  readonly statements: readonly { readonly name: string; readonly statement: unknown; readonly signing_payload: string }[];
  readonly invalid_statements: readonly { readonly name: string; readonly statement: unknown }[];
};

describe("registry tier statement", () => {
  it("signs the canonical statement, byte for byte as the Rust contract does", () => {
    const key = createPublicKey({
      key: Buffer.concat([Buffer.from("302a300506032b6570032100", "hex"), Buffer.from(fixture.public_key_base64, "base64")]),
      format: "der",
      type: "spki",
    });
    for (const testCase of fixture.statements) {
      const statement = validateRegistryTierStatement(testCase.statement);
      const payload = registryTierStatementPayload(statement);
      expect(payload, testCase.name).toBe(testCase.signing_payload);
      const signature = Buffer.from(statement.signature.value.slice("base64:".length), "base64url");
      expect(verify(null, Buffer.from(payload, "utf8"), key, signature), testCase.name).toBe(true);
      const { signature: _signature, ...unsigned } = statement;
      expect(registryTierStatementPayload(unsigned), testCase.name).toBe(testCase.signing_payload);
    }
  });

  it("refuses every malformed term and any field the contract does not name", () => {
    for (const testCase of fixture.invalid_statements) {
      expect(() => validateRegistryTierStatement(testCase.statement), testCase.name).toThrow();
    }
    const unknownField = fixture.invalid_statements.find((testCase) => testCase.name === "a field the contract does not name");
    expect(() => validateRegistryTierStatement(unknownField?.statement)).toThrow("no field withdrawn");
  });
});
