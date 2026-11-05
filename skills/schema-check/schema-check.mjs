// Domain inclusion proof only. Compilation and sample validation stay native.
const annotations = new Set([
  "$schema",
  "$id",
  "$comment",
  "$defs",
  "definitions",
  "title",
  "description",
  "default",
  "examples",
  "deprecated",
  "readOnly",
  "writeOnly",
  "format",
]);
const lowerBounds = ["minLength", "minItems", "minProperties"];
const upperBounds = ["maxLength", "maxItems", "maxProperties"];
const supported = new Set([
  ...annotations,
  "type",
  "const",
  "enum",
  "properties",
  "required",
  "additionalProperties",
  "items",
  "minimum",
  "exclusiveMinimum",
  "maximum",
  "exclusiveMaximum",
  ...lowerBounds,
  ...upperBounds,
  "multipleOf",
  "pattern",
  "uniqueItems",
]);
const allTypes = [
  "null",
  "boolean",
  "object",
  "array",
  "string",
  "integer",
  "fractional",
];
const cosmetic = new Set([
  "$comment",
  "title",
  "description",
  "default",
  "examples",
  "deprecated",
  "readOnly",
  "writeOnly",
  "format",
]);

export function checkSchemaChange(inputs) {
  const policy = inputs.compatibility_policy;
  const invalid = [];
  if (![inputs.current_schema, inputs.proposed_schema].every(isSchema)) {
    invalid.push({
      code: "schema.invalid",
      message: "Contracts must be JSON Schema objects or booleans.",
    });
  }
  if (
    !Array.isArray(inputs.sample_payloads) ||
    !inputs.sample_payloads.length
  ) {
    invalid.push({
      code: "samples.invalid",
      message: "Supply a non-empty array of real payloads.",
    });
  }
  if (
    !isRecord(policy) ||
    (policy.breaking_allowed !== undefined &&
      typeof policy.breaking_allowed !== "boolean")
  ) {
    invalid.push({
      code: "policy.invalid",
      message: "Supply an explicit compatibility policy.",
    });
  }
  if (invalid.length) return refusedInput(invalid);
  const samples = inputs.sample_payloads;
  for (const name of ["current", "proposed"]) {
    const result = inputs[`${name}_validation`];
    if (
      !isRecord(result) ||
      result.results?.length !== samples.length ||
      result.values_digest !== inputs.samples_digest ||
      result.schema_digest !== inputs[`${name}_digest`]
    ) {
      throw new Error(
        `native ${name} validation is missing or bound to different inputs`,
      );
    }
  }
  const comparison = compareSchemaAcceptance(
    inputs.current_schema,
    inputs.proposed_schema,
    policy,
  );
  const samplesValid =
    inputs.current_validation.valid && inputs.proposed_validation.valid;
  const policyAllows =
    !comparison.breaking.some(
      (change) => change.policy_rule === "policy_required_field_missing",
    ) &&
    (policy.breaking_allowed === true || comparison.breaking.length === 0);
  const compatible =
    policyAllows && samplesValid && comparison.unsupported.length === 0;
  const notes = [
    ...comparison.notes,
    ...coverageNotes(samples, comparison.paths),
  ];
  if (!samplesValid)
    notes.push({
      path: "$",
      kind: "sample_validation_failed",
      note: "One or more samples fail the current or proposed schema; inspect the native validation steps.",
    });
  if (compatible && comparison.breaking.length)
    notes.push({
      path: "$",
      kind: "policy_authorized_break",
      note: "The proposal is permitted by breaking_allowed; it is not an unchanged acceptance guarantee.",
    });
  return {
    schema_check: {
      schema: "runx/schema-check.check.v1",
      decision: compatible ? "compatible" : "refused",
      policy_result: policyAllows ? "allowed" : "blocked_by_policy",
      breaking_changes: bound(comparison.breaking, 500, (count) => ({
        path: "$",
        old_contract: "see exact current schema",
        new_contract: "see exact proposed schema",
        policy_rule: `${count}_additional_potential_breaks`,
      })),
      validation_results: inputs.proposed_validation.results.map((result) => ({
        sample_index: result.index,
        valid: result.valid,
        errors: result.errors.map(
          (issue) =>
            `${issue.instance_path || "/"}: violates ${issue.schema_path}`,
        ),
      })),
      migration_notes: bound(notes, 1000, (count) => ({
        path: "$",
        kind: "additional_notes",
        note: `${count} more notes; inspect the exact schema and validation artifacts.`,
      })),
      proposal: compatible
        ? {
            status: "proposed",
            gate: "schema-publisher-or-human-approver",
            live_write_performed: false,
            current_schema_digest: requiredDigest(inputs.current_digest),
            proposed_schema_digest: requiredDigest(inputs.proposed_digest),
            samples_digest: requiredDigest(inputs.samples_digest),
            changed_paths:
              comparison.changed.size > 500
                ? ["$"]
                : [...comparison.changed].sort(),
          }
        : null,
      validation: {
        status: comparison.unsupported.length ? "fail" : "pass",
        findings: bound(comparison.unsupported, 50, (count) => ({
          code: "compatibility.unsupported",
          message: `${count} additional unproven constraints; inspect the exact contracts.`,
        })),
      },
    },
  };
}

export function compareSchemaAcceptance(current, proposed, policy = {}) {
  const breaking = [],
    notes = [],
    unsupported = [],
    changed = new Set();
  const roots = { current, proposed };
  const sameRoots = schemaSignature(current) === schemaSignature(proposed);
  const nestedResource =
    hasNestedResource(current) || hasNestedResource(proposed);
  const active = [];
  const addBreak = (path, old, next, rule) => {
    changed.add(path);
    breaking.push({
      path,
      old_contract: describe(old),
      new_contract: describe(next),
      policy_rule: rule,
    });
  };
  const note = (path, kind, text) => {
    changed.add(path);
    notes.push({ path, kind, note: text });
  };
  const unproven = (path, message) =>
    unsupported.push({
      code: "compatibility.unsupported",
      message: `${path}: ${message}`,
    });
  // Root equality needs no inference, even for recursive or advanced schemas.
  if (!sameRoots) compare(current, proposed, "$");
  const paths = indexPaths(proposed);
  for (const value of policy.required_fields ?? []) {
    const path = value.startsWith("$")
      ? value
      : `$.${value.replace(/^\./u, "")}`;
    if (!paths.has(path))
      addBreak(
        path,
        "declared field required by policy",
        "absent",
        "policy_required_field_missing",
      );
  }
  return { breaking, notes, unsupported, changed, paths };

  function resolve(node, root, path) {
    const seen = new Set();
    while (isRecord(node) && typeof node.$ref === "string") {
      if (seen.has(node)) {
        unproven(path, "cyclic reference aliases require separate review");
        return null;
      }
      seen.add(node);
      if (
        Object.keys(node).some((key) => key !== "$ref" && !annotations.has(key))
      ) {
        unproven(
          path,
          "$ref with assertion siblings requires separate intersection review",
        );
        return null;
      }
      if (node.$ref !== "#" && !node.$ref.startsWith("#/")) {
        unproven(
          path,
          "only local JSON Pointer references are included in this proof",
        );
        return null;
      }
      let target = root;
      try {
        for (const key of node.$ref === "#"
          ? []
          : decodeURIComponent(node.$ref.slice(2))
              .split("/")
              .map((part) => part.replace(/~1/gu, "/").replace(/~0/gu, "~"))) {
          if (
            (!isRecord(target) && !Array.isArray(target)) ||
            !Object.hasOwn(target, key)
          )
            throw new Error("missing reference");
          target = target[key];
        }
      } catch {
        unproven(path, "local reference target is unavailable");
        return null;
      }
      node = target;
    }
    if (!isSchema(node)) {
      unproven(path, "reference target is not a schema");
      return null;
    }
    return node;
  }

  function compare(oldSource, nextSource, path) {
    const old = resolve(oldSource, roots.current, path),
      next = resolve(nextSource, roots.proposed, path);
    if (old === null || next === null) return;
    if (old === false || next === true) {
      if (old !== next)
        note(
          path,
          "acceptance_relaxed",
          "The proposed constraint accepts at least the prior instances.",
        );
      return;
    }
    if (next === false) {
      if (old !== false)
        addBreak(path, old, next, "schema_disallows_instances");
      return;
    }
    const a = old === true ? {} : old,
      b = next;
    // Definitions are document-relative: identical $ref spellings are not equal evidence.
    if (
      schemaSignature(a) === schemaSignature(b) &&
      (!containsRef(b) || sameRoots)
    )
      return;
    if (active.some((pair) => pair[0] === a && pair[1] === b)) {
      unproven(path, "changed recursive inclusion requires separate review");
      return;
    }
    active.push([a, b]);
    try {
      if (
        (path !== "$" &&
          (Object.hasOwn(a, "$id") || Object.hasOwn(b, "$id"))) ||
        nestedResource
      ) {
        unproven(path, "nested $id resource scopes require separate review");
        return;
      }
      for (const key of new Set([...Object.keys(a), ...Object.keys(b)])) {
        if (!supported.has(key))
          unproven(
            path,
            `${key} is outside the supported implication language`,
          );
      }
      for (const schema of [a, b])
        if (
          schema.$schema &&
          schema.$schema !== "https://json-schema.org/draft/2020-12/schema"
        ) {
          unproven(
            path,
            "changed non-2020-12 contracts require dialect-specific review",
          );
        }
      const oldTypes = types(a),
        nextTypes = types(b);
      if ([...oldTypes].some((type) => !nextTypes.has(type)))
        addBreak(path, a, b, "type_narrowed");
      else if (stable(a.type) !== stable(b.type))
        note(
          path,
          "type_relaxed",
          "The proposed type accepts the prior JSON types; integer is included in number.",
        );
      const oldEnum = finite(a),
        nextEnum = finite(b);
      if (
        nextEnum &&
        (!oldEnum ||
          oldEnum.some(
            (value) =>
              !nextEnum.some((other) => stable(value) === stable(other)),
          ))
      ) {
        addBreak(path, a, b, "enum_narrowed");
      } else if (stable(oldEnum) !== stable(nextEnum))
        note(
          path,
          "enum_relaxed",
          "The proposed finite-value constraint does not remove a prior allowed value.",
        );
      const relevant = (type) => oldTypes.has(type) && nextTypes.has(type);
      for (const [type, minimum, maximum] of [
        ["string", "minLength", "maxLength"],
        ["array", "minItems", "maxItems"],
        ["object", "minProperties", "maxProperties"],
      ]) {
        if (!relevant(type)) continue;
        if ((b[minimum] ?? 0) > (a[minimum] ?? 0))
          addBreak(path, a, b, `${minimum}_narrowed`);
        else if (a[minimum] !== b[minimum])
          note(
            path,
            "bound_relaxed",
            `${minimum} does not reject prior instances.`,
          );
        if ((b[maximum] ?? Infinity) < (a[maximum] ?? Infinity))
          addBreak(path, a, b, `${maximum}_narrowed`);
        else if (a[maximum] !== b[maximum])
          note(
            path,
            "bound_relaxed",
            `${maximum} does not reject prior instances.`,
          );
      }
      if (relevant("integer") || relevant("fractional")) {
        for (const lower of [true, false]) {
          const before = numericBound(a, lower),
            after = numericBound(b, lower);
          if (
            lower
              ? after.value > before.value ||
                (after.value === before.value &&
                  after.exclusive &&
                  !before.exclusive)
              : after.value < before.value ||
                (after.value === before.value &&
                  after.exclusive &&
                  !before.exclusive)
          ) {
            addBreak(
              path,
              a,
              b,
              lower ? "minimum_narrowed" : "maximum_narrowed",
            );
          } else if (stable(before) !== stable(after))
            note(
              path,
              "bound_relaxed",
              "The numeric bound does not reject prior instances.",
            );
        }
        if (
          b.multipleOf !== undefined &&
          b.multipleOf !== a.multipleOf &&
          !(
            Number.isSafeInteger(a.multipleOf) &&
            Number.isSafeInteger(b.multipleOf) &&
            a.multipleOf % b.multipleOf === 0
          )
        ) {
          addBreak(path, a, b, "multipleOf_narrowed");
        } else if (a.multipleOf !== b.multipleOf)
          note(
            path,
            "multiple_relaxed",
            "The proposed divisor includes the previous allowed multiples.",
          );
      }
      if (
        relevant("string") &&
        b.pattern !== undefined &&
        a.pattern !== b.pattern
      )
        unproven(
          path,
          "changed regular-expression inclusion is not inferred from matching samples",
        );
      if (relevant("array")) {
        if (b.uniqueItems && !a.uniqueItems && (a.maxItems ?? Infinity) > 1)
          addBreak(path, a, b, "uniqueItems_narrowed");
        compare(a.items ?? true, b.items ?? true, `${path}[]`);
      }
      if (relevant("object")) {
        for (const key of b.required ?? [])
          if (!(a.required ?? []).includes(key))
            addBreak(fieldPath(path, key), a, b, "field_became_required");
        const oldProps = a.properties ?? {},
          nextProps = b.properties ?? {};
        for (const key of new Set([
          ...Object.keys(oldProps),
          ...Object.keys(nextProps),
        ])) {
          const childPath = fieldPath(path, key);
          const before = Object.hasOwn(oldProps, key)
            ? oldProps[key]
            : (a.additionalProperties ?? true);
          const after = Object.hasOwn(nextProps, key)
            ? nextProps[key]
            : (b.additionalProperties ?? true);
          if (!Object.hasOwn(oldProps, key))
            note(
              childPath,
              "property_added",
              "Compare this field against the old additionalProperties constraint, including previously allowed values.",
            );
          if (!Object.hasOwn(nextProps, key))
            note(
              childPath,
              "property_removed",
              "The new additionalProperties constraint determines whether old values remain accepted.",
            );
          compare(before, after, childPath);
        }
        compare(
          a.additionalProperties ?? true,
          b.additionalProperties ?? true,
          `${path}.*`,
        );
      }
    } finally {
      active.pop();
    }
  }
}

function numericBound(schema, lower) {
  const inclusive =
    schema[lower ? "minimum" : "maximum"] ?? (lower ? -Infinity : Infinity);
  const exclusive = schema[lower ? "exclusiveMinimum" : "exclusiveMaximum"];
  if (
    exclusive !== undefined &&
    (lower ? exclusive >= inclusive : exclusive <= inclusive)
  )
    return { value: exclusive, exclusive: true };
  return { value: inclusive, exclusive: false };
}
function types(schema) {
  const declared =
    schema.type === undefined
      ? allTypes
      : (Array.isArray(schema.type) ? schema.type : [schema.type]).flatMap(
          (type) => (type === "number" ? ["integer", "fractional"] : [type]),
        );
  const values = finite(schema);
  const valueTypes = values?.map((value) =>
    value === null
      ? "null"
      : Array.isArray(value)
        ? "array"
        : typeof value === "number"
          ? Number.isInteger(value)
            ? "integer"
            : "fractional"
          : typeof value,
  );
  return new Set(
    values ? declared.filter((type) => valueTypes.includes(type)) : declared,
  );
}
function finite(schema) {
  return Object.hasOwn(schema, "const")
    ? [schema.const]
    : (schema.enum ?? null);
}
function schemaChildren(node) {
  if (!isRecord(node)) return [];
  return [
    ...[
      "$defs",
      "definitions",
      "properties",
      "patternProperties",
      "dependentSchemas",
    ].flatMap((key) => (isRecord(node[key]) ? Object.values(node[key]) : [])),
    ...[
      "items",
      "additionalProperties",
      "contains",
      "not",
      "if",
      "then",
      "else",
      "propertyNames",
      "unevaluatedProperties",
      "unevaluatedItems",
    ].flatMap((key) => (isSchema(node[key]) ? [node[key]] : [])),
    ...["allOf", "anyOf", "oneOf", "prefixItems"].flatMap((key) =>
      Array.isArray(node[key]) ? node[key] : [],
    ),
  ];
}
function containsRef(node) {
  return (
    isRecord(node) &&
    (["$ref", "$dynamicRef", "$recursiveRef"].some(
      (key) => typeof node[key] === "string",
    ) ||
      schemaChildren(node).some(containsRef))
  );
}
function schemaSignature(node) {
  if (!isRecord(node)) return stable(node);
  const mapped = Object.fromEntries(
    Object.entries(node)
      .filter(([key]) => !cosmetic.has(key))
      .map(([key, value]) => {
        if (
          [
            "$defs",
            "definitions",
            "properties",
            "patternProperties",
            "dependentSchemas",
          ].includes(key) &&
          isRecord(value)
        )
          return [
            key,
            Object.fromEntries(
              Object.entries(value).map(([name, child]) => [
                name,
                schemaSignature(child),
              ]),
            ),
          ];
        if (
          [
            "items",
            "additionalProperties",
            "contains",
            "not",
            "if",
            "then",
            "else",
            "propertyNames",
            "unevaluatedProperties",
            "unevaluatedItems",
          ].includes(key)
        )
          return [key, schemaSignature(value)];
        if (
          ["allOf", "anyOf", "oneOf", "prefixItems"].includes(key) &&
          Array.isArray(value)
        )
          return [key, value.map(schemaSignature)];
        return [key, value];
      }),
  );
  return stable(mapped);
}
function hasNestedResource(root) {
  const visit = (node) =>
    isRecord(node) &&
    (Object.hasOwn(node, "$id") || schemaChildren(node).some(visit));
  return schemaChildren(root).some(visit);
}
function indexPaths(root) {
  const paths = new Set(["$"]),
    active = new Set();
  function walk(source, path) {
    if (!isRecord(source) || active.has(source)) return;
    active.add(source);
    try {
      if (source.$ref?.startsWith("#/")) {
        let node = root;
        try {
          for (const key of decodeURIComponent(source.$ref.slice(2))
            .split("/")
            .map((part) => part.replace(/~1/gu, "/").replace(/~0/gu, "~")))
            node = node[key];
        } catch {
          return;
        }
        walk(node, path);
      }
      for (const [key, child] of Object.entries(source.properties ?? {})) {
        const next = fieldPath(path, key);
        paths.add(next);
        walk(child, next);
      }
      if (isSchema(source.items)) {
        paths.add(`${path}[]`);
        walk(source.items, `${path}[]`);
      }
    } finally {
      active.delete(source);
    }
  }
  walk(root, "$");
  return paths;
}
function coverageNotes(samples, paths) {
  const covered = new Set();
  function walk(value, path) {
    if (Array.isArray(value)) {
      if (value.length) covered.add(`${path}[]`);
      for (const child of value) walk(child, `${path}[]`);
    } else if (isRecord(value))
      for (const [key, child] of Object.entries(value)) {
        const next = fieldPath(path, key);
        covered.add(next);
        walk(child, next);
      }
  }
  samples.forEach((value) => walk(value, "$"));
  return [...paths]
    .filter((path) => path !== "$")
    .sort()
    .map((path) => ({
      path,
      kind: covered.has(path) ? "sample_covered" : "sample_not_covered",
      note: covered.has(path)
        ? "At least one supplied sample includes this path."
        : "No supplied sample covers this path; coverage was not invented.",
    }));
}
function fieldPath(base, key) {
  return /^[A-Za-z_][A-Za-z0-9_]*$/u.test(key)
    ? `${base}.${key}`
    : `${base}[${JSON.stringify(key)}]`;
}
function describe(value) {
  const text = typeof value === "string" ? value : stable(value);
  return text.length > 1900
    ? text.slice(0, 1900) + "… (see exact schema)"
    : text;
}
function bound(values, maximum, summary) {
  return values.length > maximum
    ? [...values.slice(0, maximum - 1), summary(values.length - maximum + 1)]
    : values;
}
function requiredDigest(value) {
  if (typeof value !== "string" || !/^sha256:[0-9a-f]{64}$/u.test(value))
    throw new Error("native digest evidence is missing");
  return value;
}
function refusedInput(findings) {
  return {
    schema_check: {
      schema: "runx/schema-check.check.v1",
      decision: "refused",
      policy_result: "invalid_input",
      breaking_changes: [],
      validation_results: [],
      migration_notes: [],
      proposal: null,
      validation: { status: "fail", findings },
    },
  };
}
function isSchema(value) {
  return typeof value === "boolean" || isRecord(value);
}
function isRecord(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}
function stable(value) {
  if (Array.isArray(value)) return `[${value.map(stable).join(",")}]`;
  if (isRecord(value))
    return `{${Object.keys(value)
      .sort()
      .map((key) => `${JSON.stringify(key)}:${stable(value[key])}`)
      .join(",")}}`;
  return JSON.stringify(value);
}
