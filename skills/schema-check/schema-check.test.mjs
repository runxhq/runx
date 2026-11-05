import test from "node:test";
import assert from "node:assert/strict";
import { compareSchemaAcceptance, checkSchemaChange } from "./schema-check.mjs";

function proves(a, b) {
  const result = compareSchemaAcceptance(a, b);
  assert.deepEqual(result.breaking, []);
  assert.deepEqual(result.unsupported, []);
  return result;
}
function narrows(a, b) {
  const result = compareSchemaAcceptance(a, b);
  assert.ok(result.breaking.length > 0);
  return result;
}
test("an optional typed field narrows an open object despite passing examples", () => {
  const result = narrows(
    { type: "object" },
    { type: "object", properties: { note: { type: "string" } } },
  );
  assert.equal(result.breaking[0].path, "$.note");
  proves(
    { type: "object", additionalProperties: false },
    {
      type: "object",
      properties: { note: { type: "string" } },
      additionalProperties: false,
    },
  );
});
test("property removal follows additionalProperties rather than field presence", () => {
  const a = {
    type: "object",
    properties: { old: { type: "integer" } },
    additionalProperties: true,
  };
  const b = { type: "object", additionalProperties: true };
  assert.ok(proves(a, b).changed.has("$.old"));
  narrows(a, { ...b, additionalProperties: false });
  proves(
    { type: "object", additionalProperties: { type: "string" } },
    {
      type: "object",
      properties: { note: { type: "string" } },
      additionalProperties: { type: "string" },
    },
  );
});
test("numeric type widening and union membership follow JSON Schema integer semantics", () => {
  proves({ type: "integer" }, { type: "number" });
  proves({ type: ["null", "integer"] }, { type: ["number", "null"] });
  narrows({ type: "number" }, { type: "integer" });
  narrows({ type: ["string", "null"] }, { type: "string" });
  proves({ const: "a" }, { type: "string", enum: ["a", "b"] });
});
test("homogeneous arrays compare item constraints and collection bounds", () => {
  proves(
    {
      type: "array",
      items: { type: "integer", minimum: 1 },
      minItems: 2,
      maxItems: 5,
    },
    {
      type: "array",
      items: { type: "number", minimum: 0 },
      minItems: 1,
      maxItems: 10,
    },
  );
  narrows(
    { type: "array", items: { type: "number" } },
    { type: "array", items: { type: "number", minimum: 0 } },
  );
  narrows({ type: "array" }, { type: "array", uniqueItems: true });
  proves(
    { type: "array", maxItems: 1 },
    { type: "array", maxItems: 1, uniqueItems: true },
  );
});
test("inclusive and exclusive bounds preserve endpoint semantics", () => {
  proves(
    { type: "number", exclusiveMinimum: 0, maximum: 10 },
    { type: "number", minimum: 0, exclusiveMaximum: 11 },
  );
  narrows(
    { type: "number", minimum: 0 },
    { type: "number", exclusiveMinimum: 0 },
  );
  narrows(
    { type: "number", maximum: 10 },
    { type: "number", exclusiveMaximum: 10 },
  );
  proves(
    { type: "integer", multipleOf: 6 },
    { type: "integer", multipleOf: 3 },
  );
  narrows(
    { type: "integer", multipleOf: 3 },
    { type: "integer", multipleOf: 6 },
  );
});
test("string and object counts may relax without pretending samples prove implication", () => {
  proves(
    { type: "string", minLength: 3, maxLength: 10, pattern: "^[a-z]+$" },
    { type: "string", minLength: 1, maxLength: 20, pattern: "^[a-z]+$" },
  );
  narrows(
    { type: "object", maxProperties: 10 },
    { type: "object", maxProperties: 9 },
  );
  assert.ok(
    compareSchemaAcceptance(
      { type: "string", pattern: "a" },
      { type: "string", pattern: "b" },
    ).unsupported.length,
  );
});
test("local refs compare resolved definitions, not identical ref spellings", () => {
  const make = (definition) => ({
    type: "object",
    $defs: { id: definition },
    properties: { id: { $ref: "#/$defs/id" } },
  });
  proves(make({ type: "integer" }), make({ type: "number" }));
  narrows(make({ type: "string" }), make({ type: "number" }));
  proves(
    { $defs: { "a/b": { type: "integer" } }, $ref: "#/$defs/a~1b" },
    { $defs: { "a/b": { type: "number" } }, $ref: "#/$defs/a~1b" },
  );
});
test("changed recursive refs, assertion siblings and nested resource scopes remain unproven", () => {
  const recursive = { type: "object", properties: { next: { $ref: "#" } } };
  assert.ok(
    compareSchemaAcceptance(recursive, { ...recursive, maxProperties: 3 })
      .unsupported.length,
  );
  assert.ok(
    compareSchemaAcceptance(
      { $defs: { n: { type: "number" } }, $ref: "#/$defs/n", minimum: 0 },
      { type: "number" },
    ).unsupported.length,
  );
  assert.ok(
    compareSchemaAcceptance(
      { type: "object", properties: { x: { $id: "child", type: "integer" } } },
      { type: "object", properties: { x: { $id: "child", type: "number" } } },
    ).unsupported.length,
  );
  proves(recursive, structuredClone(recursive));
});
test("removing unsupported applicators cannot silently change evaluation coverage", () => {
  assert.ok(
    compareSchemaAcceptance(
      {
        type: "array",
        prefixItems: [{ type: "string" }],
        items: { type: "number" },
      },
      { type: "array", items: { type: "number" } },
    ).unsupported.length,
  );
  assert.ok(
    compareSchemaAcceptance(
      {
        type: "object",
        patternProperties: { "^x": { type: "string" } },
        additionalProperties: false,
      },
      { type: "object", additionalProperties: false },
    ).unsupported.length,
  );
});
test("data containing ref/id keys is not mistaken for schema resource metadata", () => {
  proves(
    { enum: [{ $id: "literal", $ref: "literal" }] },
    { enum: [{ $id: "literal", $ref: "literal" }, { id: 2 }] },
  );
  proves(
    { type: "object", properties: { $id: { type: "integer" } } },
    { type: "object", properties: { $id: { type: "number" } } },
  );
});
test("annotation-only edits do not require inference, including advanced unchanged assertions", () => {
  proves(
    { title: "old", allOf: [{ type: "string" }] },
    { title: "new", allOf: [{ type: "string" }] },
  );
  narrows({ enum: [{ title: "old" }] }, { enum: [{ title: "new" }] });
});
test("booleans, nested required fields and source-path escaping remain explicit", () => {
  proves(false, { type: "string" });
  proves({ type: "string" }, true);
  narrows(true, false);
  assert.equal(
    narrows({ type: "object" }, { type: "object", required: ["a.b"] })
      .breaking[0].path,
    '$["a.b"]',
  );
  const result = compareSchemaAcceptance(
    { type: "object" },
    { type: "object" },
    { required_fields: ["id"] },
  );
  assert.equal(result.breaking[0].policy_rule, "policy_required_field_missing");
});

function check(a, b, policy = { breaking_allowed: false }) {
  const digest = "sha256:" + "a".repeat(64),
    samples = [{}];
  const validation = {
    valid: true,
    values_digest: digest,
    schema_digest: digest,
    results: [{ index: 0, valid: true, errors: [] }],
  };
  return checkSchemaChange({
    current_schema: a,
    proposed_schema: b,
    sample_payloads: samples,
    compatibility_policy: policy,
    current_digest: digest,
    proposed_digest: digest,
    samples_digest: digest,
    current_validation: validation,
    proposed_validation: validation,
  }).schema_check;
}
test("a policy exception cannot approve unknown implication semantics", () => {
  const result = check(
    { type: "array", prefixItems: [{ type: "string" }] },
    { type: "array", items: { type: "number" } },
    { breaking_allowed: true },
  );
  assert.equal(result.decision, "refused");
  assert.equal(result.validation.status, "fail");
  assert.equal(result.proposal, null);
  const intentional = check(
    { type: "string" },
    { type: "number" },
    { breaking_allowed: true },
  );
  assert.equal(intentional.decision, "compatible");
  assert.ok(
    intentional.migration_notes.some(
      (note) => note.kind === "policy_authorized_break",
    ),
  );
});
test("boolean false contracts still honor mandatory declared fields", () => {
  assert.equal(
    check(false, false, { required_fields: ["id"] }).decision,
    "refused",
  );
});

test("a dynamic reference into changed definitions is not proved compatible", () => {
  const schema = (x) => ({
    type: "object",
    properties: { x: { $dynamicRef: "#/$defs/x" } },
    $defs: { x },
  });
  const result = compareSchemaAcceptance(
    schema({ type: "string" }),
    schema({ type: "string", maxLength: 3 }),
  );
  assert.ok(result.unsupported.length > 0, JSON.stringify(result));
});
