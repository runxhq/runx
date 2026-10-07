import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";

import {
  type HttpImplementation,
  type JsonObject,
  type JsonValue,
  referenceKind,
  renderDeclaredRequest,
} from "./index.js";

interface Case {
  readonly name: string;
  readonly request: string;
  readonly arguments: JsonObject;
  readonly parameters?: JsonObject;
  readonly item?: number;
  readonly responses?: { readonly [request: string]: JsonObject };
  readonly expected?: unknown;
  readonly error?: boolean;
}

const corpus = JSON.parse(
  readFileSync(new URL("../../../fixtures/contracts/connector-request/cases.json", import.meta.url), "utf8"),
) as { readonly idempotency_key: string; readonly http: HttpImplementation; readonly cases: readonly Case[] };

// The kernel's http driver answers the same cases
// (crates/runx-runtime/src/drivers/http.rs).
describe("declared connector requests", () => {
  for (const testCase of corpus.cases) {
    it(testCase.name, () => {
      const render = () =>
        renderDeclaredRequest(corpus.http, {
          arguments: testCase.arguments,
          ...(testCase.parameters ? { parameters: testCase.parameters } : {}),
          idempotencyKey: corpus.idempotency_key,
          request: testCase.request,
          ...(testCase.item === undefined ? {} : { item: testCase.item }),
          ...(testCase.responses ? { responses: testCase.responses } : {}),
        });
      if (testCase.error) {
        expect(render).toThrow();
        return;
      }
      expect(render()).toEqual(testCase.expected);
    });
  }
});

// The kernel classifies the same locators the same way
// (crates/runx-runtime/src/kernel/connectors/references.rs).
describe("reference kinds", () => {
  it("classifies locators by what their connectors declare", () => {
    expect(referenceKind("github", "github://acme/app/issues/7")).toBe("thread");
    expect(referenceKind(undefined, "github://acme/app/pulls/7")).toBe("change_request");
    expect(referenceKind("slack", "slack://T123/C456/1750000000.000001")).toBe("thread");
    expect(referenceKind("slack", "github://acme/app/issues/7")).toBe("external_url");
    expect(referenceKind(undefined, "https://example.com/report")).toBe("external_url");
  });
});

