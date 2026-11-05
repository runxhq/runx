export function normalizeReport(inputs) {
  const supplied = object(inputs.provider_result);
  const request = object(inputs.request);
  const findings = [];
  const property = text(supplied.property || request.property);
  const reportType = text(request.report_type || supplied.request?.report_type || "standard");
  const sourceStatus = text(inputs.source_status) === "provider_readback"
    ? "provider_readback"
    : "supplied_result";

  if (!/^properties\/[1-9][0-9]*$/u.test(property)) {
    findings.push(finding("ga.property.invalid", "property must be a GA4 properties/{numeric-id} resource"));
  }
  if (!new Set(["standard", "realtime"]).has(reportType)) {
    findings.push(finding("ga.report_type.invalid", "report_type must be standard or realtime"));
  }
  if (text(request.property) && text(supplied.property) && text(request.property) !== text(supplied.property)) {
    findings.push(finding("ga.property.mismatch", "returned property does not match the request"));
  }

  const requestedDimensions = names(request.dimensions);
  const requestedMetrics = names(request.metrics);
  const dimensionHeaders = headers(supplied.dimension_headers);
  const metricHeaders = headers(supplied.metric_headers);

  if (requestedMetrics.length === 0) {
    findings.push(finding("ga.metrics.missing", "at least one requested metric is required"));
  }
  if (!same(requestedDimensions, dimensionHeaders.map((header) => header.name))) {
    findings.push(finding("ga.dimension_headers.mismatch", "returned dimension headers differ from the ordered request"));
  }
  if (!same(requestedMetrics, metricHeaders.map((header) => header.name))) {
    findings.push(finding("ga.metric_headers.mismatch", "returned metric headers differ from the ordered request"));
  }

  const rawRows = Array.isArray(supplied.rows) ? supplied.rows : [];
  if (rawRows.length > 100000) {
    findings.push(finding("ga.rows.too_many", "one evidence packet cannot contain more than 100000 rows"));
  }
  const rows = rawRows.slice(0, 100000).map((row, index) =>
    normalizeRow(row, dimensionHeaders, metricHeaders, index, findings)
  );

  const metadata = object(supplied.metadata);
  const sampling = Array.isArray(metadata.sampling_metadatas)
    ? metadata.sampling_metadatas.map((item) => object(item))
    : [];
  const subjectToThresholding = Boolean(metadata.subject_to_thresholding);
  const dataLossFromOtherRow = Boolean(metadata.data_loss_from_other_row);
  const schemaRestrictions = object(metadata.schema_restriction_response);
  const caveats = [];

  if (subjectToThresholding) {
    caveats.push("Google reports that privacy thresholding applies to this response.");
  }
  if (dataLossFromOtherRow) {
    caveats.push("Google reports data loss from an aggregated (other) row.");
  }
  if (sampling.length > 0) {
    caveats.push("Sampling metadata is present; treat the result as an estimate.");
  }
  if (Object.keys(schemaRestrictions).length > 0) {
    caveats.push("Schema restrictions are present in the provider response.");
  }
  if (reportType === "realtime") {
    caveats.push("Realtime data is provisional and is not settled period evidence.");
  }

  const paginationInput = object(supplied.pagination);
  const returnedRows = rows.length;
  const offset = nonNegativeInteger(request.offset, 0);
  const limit = nonNegativeInteger(request.limit, returnedRows);
  const knownTotal = Number.isSafeInteger(supplied.row_count) && supplied.row_count >= 0 ? supplied.row_count : null;
  const pageEnd = offset + returnedRows;
  const rowCount = knownTotal ?? pageEnd;
  if (paginationInput.returned_rows !== undefined && paginationInput.returned_rows !== returnedRows) {
    findings.push(finding("ga.pagination.row_count_mismatch", "returned_rows differs from the actual page"));
  }
  if (!Number.isSafeInteger(pageEnd) || (knownTotal !== null && returnedRows > 0 && pageEnd > knownTotal)) {
    findings.push(finding("ga.pagination.total_mismatch", "page position and rows exceed the reported total"));
  }
  const exhausted = knownTotal !== null ? pageEnd >= knownTotal
    : paginationInput.complete === true || (request.limit !== undefined && returnedRows < limit);
  const nextOffset = !exhausted && returnedRows > 0 ? pageEnd : null;
  if (!exhausted && returnedRows === 0) {
    findings.push(finding("ga.pagination.stalled", "no rows returned before the reported end; refresh the same query instead of advancing"));
  }
  if (paginationInput.next_offset != null && paginationInput.next_offset !== nextOffset) {
    findings.push(finding("ga.pagination.continuation_mismatch", "next_offset skips rows, repeats a page or continues beyond the end"));
  }
  if (knownTotal === null) {
    caveats.push("No total row count was supplied; row_count is only the observed lower bound, not a query total.");
  }
  if (knownTotal !== null && typeof paginationInput.complete === "boolean" && paginationInput.complete !== exhausted) {
    caveats.push("The connector completeness flag contradicts the reported total; coverage follows the total and actual page position.");
  }
  const paginationComplete = offset === 0 && exhausted && knownTotal !== null;
  if (!paginationComplete) {
    caveats.push("This packet does not contain the complete query result; retain the offset and any continuation before aggregating.");
  }

  const dateRanges = normalizeDateRanges(request.date_ranges, reportType, findings);
  const currencyCode = text(metadata.currency_code || request.currency_code);
  const timeZone = text(metadata.time_zone);

  const sourceQuery = object(supplied.request);
  if (Object.keys(sourceQuery).length === 0) {
    caveats.push("The original query was not echoed with this result; query identity relies on the caller or the current native provider invocation.");
  }
  const expectedQuery = queryIdentity(request, property, reportType);
  return {
    expected_query: expectedQuery,
    source_query: Object.keys(sourceQuery).length > 0
      ? queryIdentity(sourceQuery, property, "standard") : expectedQuery,
    report_draft: {
      schema: "runx/google-analytics.report_evidence.v1",
      decision: findings.length > 0
        ? "blocked"
        : caveats.length > 0
          ? "usable_with_caveats"
          : "ready",
      provider: "google-analytics",
      provider_status: sourceStatus === "provider_readback" ? "readback_verified" : "not_called",
      source_status: sourceStatus,
      property,
      report_type: reportType,
      request: {
        date_ranges: dateRanges,
        dimensions: requestedDimensions,
        metrics: requestedMetrics,
        dimension_filter: object(request.dimension_filter),
        metric_filter: object(request.metric_filter),
        order_bys: Array.isArray(request.order_bys) ? request.order_bys : [],
        minute_ranges: Array.isArray(request.minute_ranges) ? request.minute_ranges : [],
        limit,
        offset,
      },
      dimension_headers: dimensionHeaders,
      metric_headers: metricHeaders,
      rows,
      row_count: rowCount,
      pagination: {
        returned_rows: returnedRows,
        complete: paginationComplete,
        next_offset: nextOffset,
      },
      measurement: {
        time_zone: timeZone,
        currency_code: currencyCode,
        fetched_at: text(supplied.fetched_at),
      },
      privacy: {
        subject_to_thresholding: subjectToThresholding,
        data_loss_from_other_row: dataLossFromOtherRow,
        sampling_metadatas: sampling,
        schema_restriction_response: schemaRestrictions,
      },
      property_quota: object(supplied.property_quota),
      caveats,
      validation: {
        status: findings.length === 0 ? "pass" : "fail",
        findings,
      },
    },
  };
}

export function bindQuery(inputs) {
  const draft = inputs.report_draft;
  if (inputs.comparison.equal) return { report_draft: draft };
  return { report_draft: { ...draft, decision: "blocked", validation: {
    status: "fail", findings: [...draft.validation.findings,
      finding("ga.request.mismatch", "the source query differs from the requested report; do not relabel or combine its rows")],
  } } };
}

function queryIdentity(request, property, reportType) {
  return {
    property: request.property ?? property,
    report_type: request.report_type ?? reportType,
    date_ranges: (request.date_ranges ?? []).map(range => ({ ...range, name: range.name ?? "" })),
    dimensions: request.dimensions ?? [], metrics: request.metrics ?? [],
    dimension_filter: request.dimension_filter ?? {}, metric_filter: request.metric_filter ?? {},
    order_bys: request.order_bys ?? [], minute_ranges: request.minute_ranges ?? [],
    currency_code: request.currency_code ?? "", limit: request.limit ?? 1000, offset: request.offset ?? 0,
  };
}

export function finalizeReport(inputs) {
  return {
    report_evidence: {
      ...object(inputs.report_draft),
      evidence_digest: digest(inputs.digest_result),
    },
  };
}

function normalizeRow(value, dimensionHeaders, metricHeaders, index, findings) {
  const row = object(value);
  const dimensionValues = values(row.dimension_values);
  const metricValues = values(row.metric_values);
  const dimensions = {};
  const metrics = {};

  if (dimensionValues.length !== dimensionHeaders.length) {
    findings.push(finding(
      "ga.row.dimension_count_mismatch",
      `row ${index} has ${dimensionValues.length} dimension values for ${dimensionHeaders.length} headers`,
    ));
  }
  if (metricValues.length !== metricHeaders.length) {
    findings.push(finding(
      "ga.row.metric_count_mismatch",
      `row ${index} has ${metricValues.length} metric values for ${metricHeaders.length} headers`,
    ));
  }

  dimensionHeaders.forEach((header, headerIndex) => {
    dimensions[header.name] = dimensionValues[headerIndex] ?? "";
  });
  metricHeaders.forEach((header, headerIndex) => {
    const raw = metricValues[headerIndex] ?? "";
    const numeric = typeof raw === "number" ? raw : Number(raw);
    if (raw === "" || !Number.isFinite(numeric)) {
      findings.push(finding("ga.row.metric_invalid", `row ${index} has a non-numeric value for ${header.name}`));
      metrics[header.name] = null;
    } else {
      metrics[header.name] = numeric;
    }
  });

  return { dimensions, metrics };
}

function normalizeDateRanges(value, reportType, findings) {
  const ranges = Array.isArray(value) ? value.map((item) => object(item)) : [];
  if (reportType === "realtime") return [];
  if (ranges.length === 0) {
    findings.push(finding("ga.date_ranges.missing", "standard reports require at least one date range"));
    return [];
  }
  return ranges.map((range, index) => {
    const startDate = text(range.start_date);
    const endDate = text(range.end_date);
    if (!dateExpression(startDate) || !dateExpression(endDate)) {
      findings.push(finding("ga.date_range.invalid", `date range ${index} is not a supported GA4 date expression`));
    }
    return {
      start_date: startDate,
      end_date: endDate,
      name: text(range.name),
    };
  });
}

function headers(value) {
  if (!Array.isArray(value)) return [];
  return value.map((entry) => {
    if (typeof entry === "string") return { name: text(entry), type: "" };
    const item = object(entry);
    return { name: text(item.name), type: text(item.type) };
  }).filter((entry) => entry.name);
}

function names(value) {
  if (!Array.isArray(value)) return [];
  return value.map((entry) => typeof entry === "string" ? text(entry) : text(object(entry).name)).filter(Boolean);
}

function values(value) {
  if (!Array.isArray(value)) return [];
  return value.map((entry) => {
    if (entry && typeof entry === "object" && !Array.isArray(entry)) return entry.value ?? "";
    return entry ?? "";
  });
}

function same(left, right) {
  return left.length === right.length && left.every((item, index) => item === right[index]);
}

function dateExpression(value) {
  return /^\d{4}-\d{2}-\d{2}$/u.test(value)
    || /^(today|yesterday|[0-9]+daysAgo)$/u.test(value);
}

function digest(value) {
  const candidate = text(object(value).digest);
  return /^sha256:[0-9a-f]{64}$/u.test(candidate) ? candidate : "";
}

function finding(code, message) {
  return { code, message };
}

function object(value) {
  return value && typeof value === "object" && !Array.isArray(value) ? value : {};
}

function text(value) {
  return typeof value === "string" ? value.trim() : "";
}

function nonNegativeInteger(value, fallback) {
  return Number.isInteger(value) && value >= 0 ? value : fallback;
}
