export function checkSchemaChange(inputs) {
  const policy = isRecord(inputs.compatibility_policy) ? inputs.compatibility_policy : {};
  const findings = validateRequest(inputs, policy);
  if (findings.length > 0) {
    return {
      schema_check: {
        schema: "runx.schema_check.v1",
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

  const samples = inputs.sample_payloads;
  for (const [name, result] of [["current", inputs.current_validation], ["proposed", inputs.proposed_validation]]) {
    if (!isRecord(result) || result.results?.length !== samples.length || result.values_digest !== inputs.samples_digest || result.schema_digest !== inputs[`${name}_digest`]) {
      throw new Error(`native ${name} validation is missing or bound to different inputs`);
    }
  }
  const { proposed, breaking, notes } = compareSchemas(inputs.current_schema, inputs.proposed_schema, policy);
  const validationResults = inputs.proposed_validation.results.map((result) => ({
    sample_index: result.index,
    valid: result.valid,
    errors: result.errors.map((issue) => `${issue.instance_path || "/"}: violates ${issue.schema_path}`),
  }));
  const samplesValid = inputs.current_validation.valid && inputs.proposed_validation.valid;
  const unsupported = unsupportedComparison(inputs.current_schema, inputs.proposed_schema);
  const policyAllows = !breaking.some((change) => change.policy_rule === "policy_required_field_missing") && (policy.breaking_allowed === true || breaking.length === 0);
  const compatible = policyAllows && samplesValid && unsupported.length === 0;
  const migrationNotes = [
    ...notes,
    ...coverageNotes(samples, proposed),
    ...(samplesValid
      ? []
      : [{ path: "$", kind: "sample_validation_failed", note: "One or more samples failed proposed schema validation." }]),
  ];
  return {
    schema_check: {
      schema: "runx.schema_check.v1",
      decision: compatible ? "compatible" : "refused",
      policy_result: policyAllows ? "allowed" : "blocked_by_policy",
      breaking_changes: breaking,
      validation_results: validationResults,
      migration_notes: migrationNotes,
      proposal: compatible
        ? {
            status: "proposed",
            gate: "schema-publisher-or-human-approver",
            live_write_performed: false,
            current_schema_digest: requiredDigest(inputs.current_digest),
            proposed_schema_digest: requiredDigest(inputs.proposed_digest),
            samples_digest: requiredDigest(inputs.samples_digest),
            changed_paths: [...new Set([...notes, ...breaking].map((item) => item.path))].sort(),
          }
        : null,
      validation: { status: unsupported.length ? "fail" : "pass", findings: unsupported },
    },
  };
}

function validateRequest(inputs, policy) {
  const findings = [];
  if (!isRecord(inputs.current_schema)) {
    findings.push({ code: "schema.invalid", message: "current_schema must be a JSON schema object." });
  }
  if (!isRecord(inputs.proposed_schema)) {
    findings.push({ code: "schema.invalid", message: "proposed_schema must be a JSON schema object." });
  }
  if (!Array.isArray(inputs.sample_payloads) || inputs.sample_payloads.length === 0) {
    findings.push({ code: "samples.invalid", message: "sample_payloads must be a non-empty array of payloads." });
  }
  if (!isRecord(inputs.compatibility_policy)) {
    findings.push({ code: "policy.invalid", message: "compatibility_policy must be an object." });
  } else if (policy.breaking_allowed !== undefined && typeof policy.breaking_allowed !== "boolean") {
    findings.push({ code: "policy.invalid", message: "compatibility_policy.breaking_allowed must be a boolean." });
  }
  return findings;
}

function requiredDigest(value) {
  if (typeof value !== "string" || !value.startsWith("sha256:")) {
    throw new Error("native digest evidence is missing");
  }
  return value;
}

function compareSchemas(currentSchema, proposedSchema, policy) {
  const current = normalizeSchema(currentSchema);
  const proposed = normalizeSchema(proposedSchema);
  const breaking = [];
  const notes = [];
  const paths = new Set([...current.keys(), ...proposed.keys()]);

  for (const path of [...paths].sort()) {
    const oldContract = current.get(path);
    const newContract = proposed.get(path);
    if (oldContract && !newContract) {
      breaking.push(change(path, oldContract, newContract, "field_removed"));
      continue;
    }
    if (!oldContract && newContract) {
      notes.push({
        path,
        kind: newContract.required ? "new_required_field" : "new_optional_field",
        note: newContract.required
          ? "New required field can break existing callers."
          : "Additive optional field.",
      });
      if (newContract.required) {
        breaking.push(change(path, oldContract, newContract, "new_required_field"));
      }
      continue;
    }
    for (const required of newContract.required_fields) {
      if (!oldContract.required_fields.includes(required)) breaking.push(change(`${path}.${required}`, oldContract, newContract, "field_became_required"));
    }
    if (oldContract.type !== newContract.type) {
      breaking.push(change(path, oldContract, newContract, "type_changed"));
    }
    if (!oldContract.required && newContract.required) {
      breaking.push(change(path, oldContract, newContract, "field_became_required"));
    }
    if (oldContract.additional_properties !== false && newContract.additional_properties === false) {
      breaking.push(change(path, oldContract, newContract, "additional_properties_narrowed"));
    }
    if (enumNarrowed(oldContract.enum, newContract.enum)) {
      breaking.push(change(path, oldContract, newContract, "enum_narrowed"));
    } else if (enumExpanded(oldContract.enum, newContract.enum)) {
      notes.push({ path, kind: "enum_expanded", note: "Enum was expanded without removing existing values." });
    }
  }

  for (const requiredPath of policyPaths(policy.required_fields)) {
    if (!proposed.has(requiredPath)) {
      breaking.push({
        path: requiredPath,
        old_contract: describeContract(current.get(requiredPath)),
        new_contract: "absent",
        policy_rule: "policy_required_field_missing",
      });
    }
  }

  return { proposed, breaking, notes };
}

function normalizeSchema(schema, basePath = "$") {
  if (!isRecord(schema)) return new Map();
  const fields = new Map([[basePath, contractFor(schema, false)]]);
  const properties = isRecord(schema.properties) ? schema.properties : {};
  const required = new Set(Array.isArray(schema.required) ? schema.required : []);
  for (const [name, child] of Object.entries(properties)) {
    const path = basePath === "$" ? `$.${name}` : `${basePath}.${name}`;
    fields.set(path, contractFor(child, required.has(name)));
    if (isRecord(child)) {
      for (const [nestedPath, nestedContract] of normalizeSchema(child, path).entries()) {
        if (nestedPath !== path) fields.set(nestedPath, nestedContract);
      }
    }
  }
  return fields;
}

function contractFor(node, required) {
  const schema = isRecord(node) ? node : {};
  return {
    type: Array.isArray(schema.type) ? schema.type.join("|") : schema.type || "any",
    required,
    enum: Array.isArray(schema.enum) ? [...schema.enum] : null,
    required_fields: Array.isArray(schema.required) ? schema.required : [],
    additional_properties: schema.additionalProperties === undefined ? null : schema.additionalProperties,
  };
}

function describeContract(contract) {
  if (!contract) return "absent";
  const parts = [`type=${contract.type}`, `required=${contract.required}`];
  if (contract.enum) parts.push(`enum=[${contract.enum.map(String).join(",")}]`);
  if (contract.additional_properties !== null) parts.push(`additionalProperties=${contract.additional_properties}`);
  return parts.join("; ");
}

function change(path, oldContract, newContract, policyRule) {
  return {
    path,
    old_contract: describeContract(oldContract),
    new_contract: describeContract(newContract),
    policy_rule: policyRule,
  };
}

function policyPaths(value) {
  if (!Array.isArray(value)) return [];
  return value
    .map((path) => String(path || "").trim())
    .filter(Boolean)
    .map((path) => (path.startsWith("$.") ? path : `$.${path.replace(/^\./u, "")}`));
}

function enumNarrowed(oldEnum, newEnum) {
  if (!newEnum) return false;
  if (!oldEnum) return true;
  return oldEnum.some((value) => !newEnum.some((candidate) => stable(candidate) === stable(value)));
}

function enumExpanded(oldEnum, newEnum) {
  if (!oldEnum || !newEnum) return false;
  return newEnum.some((value) => !oldEnum.some((candidate) => stable(candidate) === stable(value))) && oldEnum.every((value) => newEnum.some((candidate) => stable(candidate) === stable(value)));
}

// The native engine validates arbitrary self-contained 2020-12 schemas. This
// separate inclusion proof intentionally supports a smaller contract language.
// Unknown constraints never disappear behind a passing sample or policy override.
function unsupportedComparison(current, proposed) {
  if (stable(current) === stable(proposed)) return [];
  const supported = new Set(["type", "properties", "required", "enum", "additionalProperties", "$schema", "title", "description", "default", "examples"]);
  const findings = [];
  function visit(node, path) {
    if (!isRecord(node)) {
      findings.push({ code: "compatibility.unsupported", message: `${path}: boolean schemas require separate compatibility review.` });
      return;
    }
    for (const key of Object.keys(node)) {
      if (!supported.has(key) || (key === "additionalProperties" && typeof node[key] !== "boolean")) {
        findings.push({ code: "compatibility.unsupported", message: `${path}/${key}: sample validation is available, but compatibility is not proven for this constraint.` });
      }
    }
    for (const [key, child] of Object.entries(node.properties || {})) visit(child, `${path}/properties/${key}`);
  }
  visit(current, "current"); visit(proposed, "proposed");
  return findings;
}

function stable(value) {
  if (Array.isArray(value)) return JSON.stringify(value.map((item) => JSON.parse(stable(item))));
  if (isRecord(value)) return JSON.stringify(Object.fromEntries(Object.keys(value).sort().map((key) => [key, JSON.parse(stable(value[key]))])));
  return JSON.stringify(value);
}

function coverageNotes(samples, proposedFields) {
  const covered = new Set();
  for (const sample of samples) {
    for (const path of coveredPaths(sample)) covered.add(path);
  }
  return [...proposedFields.keys()].filter((path) => path !== "$").sort().map((path) => ({
    path,
    kind: covered.has(path) ? "sample_covered" : "sample_not_covered",
    note: covered.has(path)
      ? "At least one supplied sample includes this path."
      : "No supplied sample covers this path; coverage was not invented.",
  }));
}

function coveredPaths(sample, basePath = "$") {
  if (!isRecord(sample)) return new Set();
  const paths = new Set();
  for (const [field, value] of Object.entries(sample)) {
    const path = `${basePath}.${field}`;
    paths.add(path);
    for (const nested of coveredPaths(value, path)) paths.add(nested);
  }
  return paths;
}

function isRecord(value) {
  return Boolean(value) && typeof value === "object" && !Array.isArray(value);
}
