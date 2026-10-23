import {
  digest,
  finding,
  nonNegativeIntegerOrNull,
  object,
  stringArray,
  text,
  validProperty,
} from "./shared.mjs";

const ALLOWED_DIMENSIONS = new Set([
  "query",
  "page",
  "country",
  "device",
  "date",
  "hour",
  "searchAppearance",
]);

const ALLOWED_SEARCH_TYPES = new Set([
  "web",
  "image",
  "video",
  "news",
  "googleNews",
  "discover",
]);

const ALLOWED_DATA_STATES = new Set(["final", "all", "hourly_all"]);

export function normalizePerformance(inputs) {
  const supplied = object(inputs.provider_result);
  const expected = object(inputs.request);
  const findings = [];

  const property = text(supplied.property || expected.property);
  const startDate = text(supplied.start_date || expected.start_date);
  const endDate = text(supplied.end_date || expected.end_date);
  const dimensions = stringArray(
    Array.isArray(supplied.dimensions) ? supplied.dimensions : expected.dimensions,
  );
  const searchType = text(supplied.search_type || expected.search_type || "web");
  const dataState = text(supplied.data_state || expected.data_state || "final");
  const sourceStatus = text(inputs.source_status) === "provider_readback"
    ? "provider_readback"
    : "supplied_result";

  if (!validProperty(property)) {
    findings.push(finding("gsc.property.invalid", "property must be an HTTP(S) URL-prefix or sc-domain property"));
  }
  if (!date(startDate) || !date(endDate) || startDate > endDate) {
    findings.push(finding("gsc.date_range.invalid", "start_date and end_date must form an ordered YYYY-MM-DD range"));
  }
  if (dimensions.length === 0 || dimensions.some((dimension) => !ALLOWED_DIMENSIONS.has(dimension))) {
    findings.push(finding("gsc.dimensions.invalid", "dimensions must be a non-empty supported ordered subset"));
  }
  if (new Set(dimensions).size !== dimensions.length) {
    findings.push(finding("gsc.dimensions.duplicate", "dimensions cannot contain duplicates"));
  }
  if (!ALLOWED_SEARCH_TYPES.has(searchType)) {
    findings.push(finding("gsc.search_type.invalid", "search_type is not supported"));
  }
  if (!ALLOWED_DATA_STATES.has(dataState)) {
    findings.push(finding("gsc.data_state.invalid", "data_state must be final, all, or hourly_all"));
  }
  if (dataState === "hourly_all" && !dimensions.includes("hour")) {
    findings.push(finding(
      "gsc.hourly_all.hour_dimension_missing",
      "hourly_all data requires the hour dimension",
    ));
  }
  if (dimensions.includes("hour") && dataState !== "hourly_all") {
    findings.push(finding(
      "gsc.hour.data_state_mismatch",
      "the hour dimension requires hourly_all data_state",
    ));
  }

  for (const field of ["property", "start_date", "end_date", "search_type", "data_state"]) {
    const expectedValue = text(expected[field]);
    const suppliedValue = text(supplied[field]);
    if (expectedValue && suppliedValue && expectedValue !== suppliedValue) {
      findings.push(finding(`gsc.request.${field}_mismatch`, `supplied ${field} does not match the request`));
    }
  }
  const expectedDimensions = stringArray(expected.dimensions);
  if (
    expectedDimensions.length > 0
    && dimensions.join("\u0000") !== expectedDimensions.join("\u0000")
  ) {
    findings.push(finding("gsc.request.dimensions_mismatch", "supplied dimensions do not match the request"));
  }

  const rawRows = Array.isArray(supplied.rows) ? supplied.rows : [];
  if (rawRows.length > 25000) {
    findings.push(finding("gsc.rows.too_many", "one evidence packet cannot contain more than 25000 rows"));
  }
  const rows = rawRows.slice(0, 25000).map((row, index) =>
    normalizePerformanceRow(row, dimensions, index, findings)
  );

  const metadata = object(supplied.metadata);
  const firstIncompleteDate = text(metadata.first_incomplete_date);
  const firstIncompleteHour = text(metadata.first_incomplete_hour);
  if (firstIncompleteDate && !date(firstIncompleteDate)) {
    findings.push(finding("gsc.metadata.first_incomplete_date_invalid", "first_incomplete_date must be YYYY-MM-DD"));
  }
  if (firstIncompleteHour && !offsetHour(firstIncompleteHour)) {
    findings.push(finding(
      "gsc.metadata.first_incomplete_hour_invalid",
      "first_incomplete_hour must be an ISO-8601 offset hour",
    ));
  }
  const complete = !firstIncompleteDate && !firstIncompleteHour;
  const caveats = [];
  if (!complete) {
    caveats.push("Provider metadata marks part of this period as incomplete.");
  }
  if (dataState !== "final") {
    caveats.push("The request admitted non-final Search Console data.");
  }

  const paginationInput = object(supplied.pagination);
  const returnedRows = integerOr(paginationInput.returned_rows, rows.length);
  const reportedRowCount = integerOr(supplied.row_count, rows.length);
  const paginationComplete = typeof paginationInput.complete === "boolean"
    ? paginationInput.complete
    : returnedRows === 0 || returnedRows < integerOr(expected.row_limit, returnedRows);
  if (!paginationComplete) {
    caveats.push("The packet is a bounded page and does not claim complete property coverage.");
  }

  return {
    performance_draft: {
      schema: "runx/google-search-console.performance_evidence.v1",
      decision: findings.length > 0
        ? "blocked"
        : caveats.length > 0
          ? "usable_with_caveats"
          : "ready",
      provider: "google-search-console",
      provider_status: sourceStatus === "provider_readback" ? "readback_verified" : "not_called",
      source_status: sourceStatus,
      property,
      request: {
        start_date: startDate,
        end_date: endDate,
        dimensions,
        search_type: searchType,
        data_state: dataState,
      },
      rows,
      row_count: reportedRowCount,
      pagination: {
        returned_rows: returnedRows,
        complete: paginationComplete,
        next_start_row: nonNegativeIntegerOrNull(paginationInput.next_start_row),
      },
      aggregation_type: text(supplied.aggregation_type),
      freshness: {
        complete,
        data_state: dataState,
        first_incomplete_date: firstIncompleteDate,
        first_incomplete_hour: firstIncompleteHour,
        fetched_at: text(supplied.fetched_at),
      },
      caveats,
      validation: {
        status: findings.length === 0 ? "pass" : "fail",
        findings,
      },
    },
  };
}

export function finalizePerformance(inputs) {
  const draft = object(inputs.performance_draft);
  return {
    performance_evidence: {
      ...draft,
      evidence_digest: digest(inputs.digest_result),
    },
  };
}

function normalizePerformanceRow(value, dimensions, index, findings) {
  const row = object(value);
  const keys = Array.isArray(row.keys) ? row.keys.map((item) => text(item)) : [];
  const named = object(row.dimensions);
  const mapped = {};

  if (keys.length > 0 && keys.length !== dimensions.length) {
    findings.push(finding(
      "gsc.row.dimension_count_mismatch",
      `row ${index} has ${keys.length} keys for ${dimensions.length} dimensions`,
    ));
  }
  dimensions.forEach((dimension, dimensionIndex) => {
    mapped[dimension] = keys.length > 0 ? text(keys[dimensionIndex]) : text(named[dimension]);
    if (!mapped[dimension]) {
      findings.push(finding("gsc.row.dimension_missing", `row ${index} is missing dimension ${dimension}`));
    }
  });

  const metrics = {};
  for (const field of ["clicks", "impressions", "ctr", "position"]) {
    const numeric = numberOrNull(row[field]);
    if (numeric === null || numeric < 0) {
      findings.push(finding("gsc.row.metric_invalid", `row ${index} has invalid ${field}`));
      metrics[field] = 0;
    } else {
      metrics[field] = numeric;
    }
  }
  if (metrics.ctr > 1) {
    findings.push(finding("gsc.row.ctr_invalid", `row ${index} CTR must be between 0 and 1`));
  }

  return { dimensions: mapped, metrics };
}

function date(value) {
  return /^\d{4}-\d{2}-\d{2}$/u.test(value);
}

function offsetHour(value) {
  return /^\d{4}-\d{2}-\d{2}T\d{2}:00:00(?:Z|[+-]\d{2}:\d{2})$/u.test(value);
}

function numberOrNull(value) {
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

function integerOr(value, fallback) {
  return Number.isInteger(value) && value >= 0 ? value : fallback;
}
