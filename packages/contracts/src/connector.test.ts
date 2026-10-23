import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";

import {
  type HttpImplementation,
  type ConnectorManifest,
  type JsonObject,
  type JsonValue,
  referenceKind,
  connectorPays,
  operationAmount,
  renderDeclaredRequest,
  renderOAuth2TokenEndpoint,
  renderTemplate,
} from "./index.js";

const payments = JSON.parse(
  readFileSync(new URL("../../../fixtures/contracts/connector-payments.json", import.meta.url), "utf8"),
) as {
  readonly pays: ConnectorManifest["pays"];
  readonly cases: readonly {
    readonly name: string;
    readonly terms: Parameters<typeof connectorPays>[1];
    readonly expected: boolean;
  }[];
};

describe("connector payment compatibility", () => {
  for (const testCase of payments.cases) {
    it(testCase.name, () => {
      expect(connectorPays(payments, testCase.terms)).toBe(testCase.expected);
    });
  }
  it("requires a declared family", () => {
    expect(connectorPays({}, { family: "x402" })).toBe(false);
    expect(connectorPays({ pays: {} }, { family: "constructor" })).toBe(false);
    expect(connectorPays({ pays: {} }, { family: "toString" })).toBe(false);
  });
});

describe("bound financial amounts", () => {
  const amounts = JSON.parse(readFileSync(new URL("../../../fixtures/contracts/connector-amount.json", import.meta.url), "utf8")) as {
    binding: ConnectorManifest["operations"][string]["amount"];
    decimal_binding: ConnectorManifest["operations"][string]["amount"];
    cases: Array<{ name: string; arguments: JsonObject; expected?: unknown; error?: boolean; decimal?: boolean }>;
  };
  for (const entry of amounts.cases) it(entry.name, () => {
    const resolve = () => operationAmount({ amount: entry.decimal ? amounts.decimal_binding : amounts.binding } as ConnectorManifest["operations"][string], entry.arguments);
    if (entry.error) expect(resolve).toThrow();
    else expect(resolve()).toEqual(entry.expected);
  });
});

interface Case {
  readonly name: string;
  readonly request: string;
  readonly arguments: JsonObject;
  readonly parameters?: JsonObject;
  readonly item?: number;
  readonly responses?: { readonly [request: string]: JsonObject };
  readonly purchase_id?: string;
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
          ...(testCase.purchase_id === undefined ? {} : { purchaseId: testCase.purchase_id }),
          request: testCase.request,
          ...(testCase.item === undefined ? {} : { item: testCase.item }),
          ...(testCase.responses ? { responses: testCase.responses } : {}),
        });
      if (testCase.error) {
        expect(render).toThrow();
        return;
      }
      const result = render();
      const body = result.body !== undefined && result.headers["content-type"] === "application/json" ? JSON.parse(result.body) as unknown : result.body;
      expect({ ...result, ...(body === undefined ? {} : { body }) }).toEqual(testCase.expected);
    });
  }
});

// The kernel classifies the same locators the same way
// (crates/runx-runtime/src/kernel/connectors/references.rs).
describe("reference kinds", () => {
  // The declarations the github and slack connector manifests carry.
  const github = {
    references: [
      { kind: "thread", locator: "github://[^/]+/[^/]+/issues/[1-9][0-9]*" },
      { kind: "change_request", locator: "github://[^/]+/[^/]+/pulls/[1-9][0-9]*" },
      { kind: "comment", locator: "github://[^/]+/[^/]+/issues/comments/[1-9][0-9]*" },
      { kind: "repository", locator: "github://[^/]+/[^/]+" },
    ],
  } as const;
  const slack = { references: [{ kind: "thread", locator: "slack://[^/]+/[^/]+/[0-9]+\\.[0-9]+" }] } as const;

  it("classifies a locator by what its own connector declares", () => {
    expect(referenceKind(github, "github://acme/app/issues/7")).toBe("thread");
    expect(referenceKind(github, "github://acme/app/pulls/7")).toBe("change_request");
    expect(referenceKind(github, "github://acme/app")).toBe("repository");
    expect(referenceKind(slack, "slack://T123/C456/1750000000.000001")).toBe("thread");
    // Another connector's locator, or one with no declaration, is an external URL.
    expect(referenceKind(slack, "github://acme/app/issues/7")).toBe("external_url");
    expect(referenceKind({}, "https://example.com/report")).toBe("external_url");
  });
});


// The kernel renders an effect with the same bindings
// (crates/runx-contracts/src/connector/tests.rs).
describe("effect locators", () => {
  it("read the call's result as $result and a repeated element as $item", () => {
    const locator: JsonValue = {
      $concat: ["github://", { $argument: "/repository" }, "/issues/", { $item: "/number" }],
    };
    expect(renderTemplate(locator, { arguments: { repository: "acme/app" }, item: { number: 7 } }))
      .toBe("github://acme/app/issues/7");
    expect(renderTemplate({ $result: "/post/id" }, { result: { post: { id: "19" } } })).toBe("19");
    expect(() => renderTemplate({ $result: "/post/id" }, {})).toThrow();
  });
});

// The kernel binds the same runtime values
// (crates/runx-contracts/src/connector/tests.rs).
describe("runtime values", () => {
  it("render the purchase only where it is supplied, never in place of the key", () => {
    const template: JsonValue = { purchase: { $runx: "purchase" }, key: { $runx: "idempotency_key" } };
    expect(renderTemplate(template, { idempotencyKey: "runx:sha256:abc", purchaseId: "purchase_1" }))
      .toEqual({ purchase: "purchase_1", key: "runx:sha256:abc" });
    expect(() => renderTemplate(template, { idempotencyKey: "runx:sha256:abc" })).toThrow(/purchase/);
    expect(() => renderTemplate({ $runx: "secret" }, { idempotencyKey: "runx:sha256:abc" })).toThrow();
  });
});


describe("OAuth2 endpoint rendering", () => {
  const corpus = JSON.parse(readFileSync(new URL("../../../fixtures/contracts/connector-oauth2.json", import.meta.url), "utf8"));
  for (const test of corpus.endpoint_cases) {
    it(test.name, () => {
      const render = () => renderOAuth2TokenEndpoint(test.template,
        { ...(test.arguments ? { arguments: test.arguments } : {}), ...(test.parameters ? { parameters: test.parameters } : {}) }, test.allowed_hosts);
      if (test.expected) expect(render()).toBe(test.expected);
      else expect(render).toThrow();
    });
  }
});
