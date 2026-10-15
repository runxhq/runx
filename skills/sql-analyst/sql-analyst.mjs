export function prepareSchema(inputs) {
  const question = text(inputs.question);
  const dialect = text(inputs.dialect) || "postgres";
  const asOfText = text(inputs.as_of);
  const asOf = Date.parse(asOfText);
  const maxSchemaAgeDays = Number(inputs.max_schema_age_days ?? 30);
  const schema = object(inputs.schema_summary);
  const sampleSnapshot = object(inputs.sample_rows);
  const constraints = object(inputs.constraints);
  const tablesObject = object(schema.tables);
  const blockers = [];
  let reason = "needs_schema";

  if (!question) blockers.push("question is missing");
  if (!Number.isFinite(asOf)) blockers.push("as_of is invalid");
  if (!Number.isFinite(maxSchemaAgeDays) || maxSchemaAgeDays <= 0 || maxSchemaAgeDays > 3650) {
    blockers.push("max_schema_age_days is invalid");
  }
  if (/\b(insert|update|delete|drop|alter|truncate|grant|revoke|create)\b/iu.test(question)) {
    blockers.push("question requests a write or schema mutation");
    reason = "unsafe_request";
  }
  if (!["postgres", "sqlite", "mysql"].includes(dialect)) blockers.push("dialect is unsupported");
  if (constraints.read_only === false) {
    blockers.push("read_only must not be false");
    reason = "unsafe_request";
  }

  const schemaSource = admitSource("schema_summary", schema, asOf, maxSchemaAgeDays, blockers);
  const allowed = Array.isArray(constraints.allowed_tables)
    ? constraints.allowed_tables.map(text).filter(Boolean)
    : Object.keys(tablesObject);
  if (allowed.some((name) => !Object.hasOwn(tablesObject, name))) {
    blockers.push("allowed_tables references an unknown table");
  }
  const tables = {};
  for (const [name, definition] of Object.entries(tablesObject)) {
    if (!allowed.includes(name)) continue;
    const fields = Array.isArray(definition?.fields) ? definition.fields.map(text).filter(Boolean) : [];
    if (!validIdentifier(name)
      || fields.length === 0
      || new Set(fields).size !== fields.length
      || fields.some((field) => !validIdentifier(field))) {
      blockers.push(`table ${name || "<empty>"} has invalid or duplicate identifiers`);
      continue;
    }
    tables[name] = fields;
  }
  if (Object.keys(tables).length === 0) blockers.push("no allowed tables with fields are available");

  const maxRows = Number(constraints.max_rows ?? 1000);
  if (!Number.isInteger(maxRows) || maxRows < 1 || maxRows > 10_000) {
    blockers.push("max_rows must be an integer from 1 to 10000");
  }
  let sampleRows = [];
  let sampleSource = {};
  if (Object.keys(sampleSnapshot).length > 0) {
    sampleSource = admitSource("sample_rows", sampleSnapshot, asOf, maxSchemaAgeDays, blockers);
    if (!Array.isArray(sampleSnapshot.rows) || sampleSnapshot.rows.length > 20) {
      blockers.push("sample_rows.rows must contain at most 20 rows");
    } else {
      sampleRows = sampleSnapshot.rows.map(object);
    }
  }
  const governedRead = admitGovernedRead(object(inputs.execution_context), blockers);

  return {
    analysis_context: {
      decision: blockers.length === 0 ? "ready" : "stop",
      stop_reason: blockers.length === 0 ? "" : reason,
      question,
      dialect,
      as_of: asOfText,
      max_schema_age_days: maxSchemaAgeDays,
      tables,
      allowed_tables: allowed,
      max_rows: maxRows,
      sample_rows: sampleRows,
      source_evidence: [schemaSource, sampleSource].filter((source) => Object.keys(source).length > 0),
      governed_read: governedRead,
      blockers,
    },
  };
}

export function finalizePlan(inputs) {
  const context = object(inputs.analysis_context);
  const draft = object(inputs.sql_plan_draft);
  const findings = strings(context.blockers).map((message) => ({ code: "sql.context.invalid", message }));
  let decision = text(context.stop_reason) || "needs_schema";

  if (context.decision === "ready") {
    decision = "needs_schema";
    if (text(draft.decision) !== "ready") {
      findings.push({ code: "sql.plan.not_ready", message: "draft decision is not ready" });
    }
    const plan = object(draft.query_plan);
    if (text(plan.dialect) !== text(context.dialect)) {
      findings.push({ code: "sql.dialect.changed", message: "plan dialect does not match input" });
    }
    const tables = strings(plan.tables);
    const allowedTables = new Set(Object.keys(object(context.tables)));
    if (tables.length === 0 || tables.some((table) => !allowedTables.has(table))) {
      findings.push({ code: "sql.table.unknown", message: "plan references an unknown or disallowed table" });
    }
    const fields = strings(plan.fields);
    if (fields.length === 0 || fields.some((field) => !knownField(field, context.tables))) {
      findings.push({ code: "sql.field.unknown", message: "plan references an unknown qualified field" });
    }
    const joins = Array.isArray(plan.joins) ? plan.joins : [];
    if (joins.some((join) => !knownField(join?.left, context.tables)
      || !knownField(join?.right, context.tables)
      || !["inner", "left"].includes(text(join?.type)))) {
      findings.push({ code: "sql.join.invalid", message: "join requires known fields and an inner or left type" });
    }
    const filters = Array.isArray(plan.filters) ? plan.filters : [];
    const operators = new Set(["eq", "neq", "gt", "gte", "lt", "lte", "between", "in", "is_null", "is_not_null", "like"]);
    if (filters.some((filter) => !knownField(filter?.field, context.tables)
      || !operators.has(text(filter?.operator))
      || (!["is_null", "is_not_null"].includes(text(filter?.operator)) && !text(filter?.value_ref)))) {
      findings.push({ code: "sql.filter.invalid", message: "filters require a known field, bounded operator, and non-literal value_ref" });
    }
    const limit = Number(plan.limit);
    if (!Number.isInteger(limit) || limit < 1 || limit > Number(context.max_rows)) {
      findings.push({ code: "sql.limit.invalid", message: "plan limit exceeds the admitted bound" });
    }
    if (/\b(insert|update|delete|drop|alter|truncate|grant|revoke|create)\b/iu.test(JSON.stringify(plan))) {
      findings.push({ code: "sql.write_token.detected", message: "plan contains a write or schema mutation token" });
      decision = "unsafe_request";
    }
    if (!Array.isArray(draft.validation_checks)
      || draft.validation_checks.length === 0
      || !text(draft.interpretation?.summary)) {
      findings.push({ code: "sql.analysis.incomplete", message: "plan requires validation checks and an interpretation summary" });
    }
    if (findings.length === 0) decision = "ready";
  }

  const ready = decision === "ready";
  const governedRead = object(context.governed_read);
  const handoffReady = ready && text(governedRead.operation) && Object.keys(object(governedRead.inputs)).length > 0;
  return {
    sql_analysis_plan: {
      decision,
      query_plan: ready ? object(draft.query_plan) : {},
      validation_checks: ready && Array.isArray(draft.validation_checks) ? draft.validation_checks : [],
      interpretation: ready ? object(draft.interpretation) : {},
      residual_risks: ready && Array.isArray(draft.residual_risks) ? draft.residual_risks : [],
      schema_binding: {
        dialect: text(context.dialect),
        allowed_tables: Array.isArray(context.allowed_tables) ? context.allowed_tables : [],
        max_rows: Number(context.max_rows || 0),
        source_evidence: Array.isArray(context.source_evidence) ? context.source_evidence : [],
      },
      execution: {
        status: !ready ? "not_ready" : handoffReady ? "prepared_for_governed_read" : "planned_only",
        executed: false,
        provider_receipt_ref: "",
        handoff: handoffReady
          ? { connector: "store", operation: text(governedRead.operation), inputs: object(governedRead.inputs) }
          : {},
      },
      validation: { status: ready ? "pass" : "fail", findings },
    },
  };
}

function admitSource(label, source, asOf, maxAgeDays, blockers) {
  const sourceRef = text(source.source_ref);
  const sourceDigest = text(source.source_digest);
  const observedText = text(source.observed_at);
  const observedAt = Date.parse(observedText);
  const ageDays = (asOf - observedAt) / 86_400_000;
  if (!sourceRef || !/^sha256:[0-9a-f]{64}$/u.test(sourceDigest) || !Number.isFinite(observedAt)) {
    blockers.push(`${label} requires source_ref, source_digest, and observed_at`);
    return {};
  }
  if (!Number.isFinite(asOf) || !Number.isFinite(maxAgeDays) || ageDays < 0 || ageDays > maxAgeDays) {
    blockers.push(`${label} is stale or future-dated`);
    return {};
  }
  return {
    source_ref: sourceRef,
    source_digest: sourceDigest,
    observed_at: observedText,
    provenance: "caller_supplied_source_digest",
  };
}

function admitGovernedRead(context, blockers) {
  if (Object.keys(context).length === 0) return {};
  const operation = text(context.operation);
  const dataSourceRef = text(context.data_source_ref);
  const resource = text(context.resource);
  const aggregateId = text(context.aggregate_id);
  const limit = Number(context.limit ?? 50);
  if (!["projection.read", "events.read", "heads.list"].includes(operation)) {
    blockers.push("execution_context operation is not a store read");
  }
  if (!dataSourceRef || !resource) blockers.push("execution_context requires data_source_ref and resource");
  if (["projection.read", "events.read"].includes(operation) && !aggregateId) {
    blockers.push("execution_context operation requires aggregate_id");
  }
  if (["events.read", "heads.list"].includes(operation)
    && (!Number.isInteger(limit) || limit < 1 || limit > 100)) {
    blockers.push("execution_context limit must be from 1 to 100");
  }
  return {
    operation,
    inputs: {
      data_source_ref: dataSourceRef,
      resource,
      ...(aggregateId ? { aggregate_id: aggregateId } : {}),
      ...(["events.read", "heads.list"].includes(operation) ? { limit } : {}),
    },
  };
}

function knownField(value, tables) {
  const parts = text(value).split(".");
  return parts.length === 2 && Array.isArray(tables?.[parts[0]]) && tables[parts[0]].includes(parts[1]);
}

function validIdentifier(value) {
  return /^[A-Za-z_][A-Za-z0-9_$]*$/u.test(text(value));
}

function object(value) {
  return value && typeof value === "object" && !Array.isArray(value) ? value : {};
}

function text(value) {
  return typeof value === "string" ? value.trim() : "";
}

function strings(value) {
  return Array.isArray(value)
    ? value.filter((entry) => typeof entry === "string").map((entry) => entry.trim()).filter(Boolean)
    : [];
}
