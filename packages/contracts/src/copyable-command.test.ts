import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";

import { posixCommand, renderPosixCommand } from "./copyable-command.js";

const fixture = JSON.parse(
  readFileSync(new URL("../../../fixtures/contracts/copyable-command/copyable-command-v1.vectors.json", import.meta.url), "utf8"),
) as { readonly cases: readonly { readonly name: string; readonly argv: readonly string[]; readonly copy_text?: string }[] };

describe("copyable commands", () => {
  it("render as the vectors the Rust contract holds to", () => {
    expect(fixture.cases.length).toBeGreaterThan(10);
    for (const testCase of fixture.cases) {
      if (testCase.copy_text === undefined) {
        expect(() => renderPosixCommand(testCase.argv), testCase.name).toThrow();
      } else {
        expect(posixCommand(testCase.argv), testCase.name).toEqual({
          shell: "posix",
          argv: testCase.argv,
          copy_text: testCase.copy_text,
        });
      }
    }
  });
});
