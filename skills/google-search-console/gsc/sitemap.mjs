import {
  digest,
  finding,
  nonNegativeIntegerOrNull,
  object,
  propertyCovers,
  text,
  validProperty,
  webUrl,
} from "./shared.mjs";

export function prepareSitemapPlan(inputs) {
  const property = text(inputs.property);
  const sitemapUrl = text(inputs.sitemap_url);
  const findings = [];

  if (!validProperty(property)) {
    findings.push(finding("gsc.property.invalid", "property must be an HTTP(S) URL-prefix or sc-domain property"));
  }
  if (!webUrl(sitemapUrl)) {
    findings.push(finding("gsc.sitemap_url.invalid", "sitemap_url must be an absolute HTTP(S) URL"));
  }
  if (property && sitemapUrl && !propertyCovers(property, sitemapUrl)) {
    findings.push(finding("gsc.sitemap.outside_property", "sitemap_url is not covered by the property"));
  }

  const digestSubject = {
    provider: "google-search-console",
    operation: "sitemaps.submit",
    property,
    sitemap_url: sitemapUrl,
  };
  return {
    sitemap_plan_draft: {
      schema: "runx/google-search-console.sitemap_plan.v1",
      decision: findings.length === 0 ? "ready_for_execution" : "blocked",
      ...digestSubject,
      provider_status: "not_called",
      external_status: "not_submitted",
      validation: {
        status: findings.length === 0 ? "pass" : "fail",
        findings,
      },
    },
    digest_subject: digestSubject,
  };
}

export function bindSitemapPlan(inputs) {
  return {
    sitemap_plan: {
      ...object(inputs.sitemap_plan_draft),
      plan_digest: digest(inputs.digest_result),
    },
  };
}

export function admitSitemapSubmission(inputs) {
  const plan = object(inputs.sitemap_plan);
  const findings = [];
  const computedDigest = digest(inputs.digest_result);

  if (text(plan.schema) !== "runx/google-search-console.sitemap_plan.v1") {
    findings.push(finding("gsc.sitemap_plan.schema_invalid", "sitemap plan schema is not supported"));
  }
  if (text(plan.decision) !== "ready_for_execution") {
    findings.push(finding("gsc.sitemap_plan.not_ready", "sitemap plan is not ready for execution"));
  }
  if (text(plan.provider) !== "google-search-console" || text(plan.operation) !== "sitemaps.submit") {
    findings.push(finding("gsc.sitemap_plan.operation_mismatch", "sitemap plan does not bind the Search Console submit operation"));
  }
  if (text(plan.provider_status) !== "not_called" || text(plan.external_status) !== "not_submitted") {
    findings.push(finding("gsc.sitemap_plan.already_advanced", "sitemap plan claims provider activity"));
  }
  if (text(plan.plan_digest) !== computedDigest) {
    findings.push(finding("gsc.sitemap_plan.digest_mismatch", "sitemap plan fields do not match its native digest"));
  }

  return {
    submission_admission: {
      decision: findings.length === 0 ? "ready" : "blocked",
      property: text(plan.property),
      sitemap_url: text(plan.sitemap_url),
      plan_digest: text(plan.plan_digest),
      validation: {
        status: findings.length === 0 ? "pass" : "fail",
        findings,
      },
    },
  };
}

export function refuseSitemapSubmission(inputs) {
  const admission = object(inputs.submission_admission);
  return {
    sitemap_submission: {
      schema: "runx/google-search-console.sitemap_submission.v1",
      decision: "blocked",
      provider: "google-search-console",
      operation: "sitemaps.submit",
      property: text(admission.property),
      sitemap_url: text(admission.sitemap_url),
      plan_digest: text(admission.plan_digest),
      idempotency_key: text(inputs.idempotency_key),
      provider_status: "not_called",
      external_status: "not_submitted",
      mutation: { accepted_at: "" },
      readback: {
        status: "",
        last_submitted: "",
        last_downloaded: "",
        error_count: null,
        warning_count: null,
      },
      validation: object(admission.validation),
    },
  };
}

export function finalizeSitemapSubmission(inputs) {
  const plan = object(inputs.sitemap_plan);
  const mutation = object(inputs.mutation_result);
  const readback = object(inputs.readback_result);
  const findings = [];
  const property = text(plan.property);
  const sitemapUrl = text(plan.sitemap_url);

  for (const [label, result] of [["mutation", mutation], ["readback", readback]]) {
    if (text(result.property) !== property || text(result.sitemap_url) !== sitemapUrl) {
      findings.push(finding(
        `gsc.sitemap_submission.${label}_identity_mismatch`,
        `${label} does not bind the exact property and sitemap URL`,
      ));
    }
  }

  return {
    sitemap_submission: {
      schema: "runx/google-search-console.sitemap_submission.v1",
      decision: findings.length === 0 ? "completed" : "blocked",
      provider: "google-search-console",
      operation: "sitemaps.submit",
      property,
      sitemap_url: sitemapUrl,
      plan_digest: text(plan.plan_digest),
      idempotency_key: text(inputs.idempotency_key),
      provider_status: findings.length === 0 ? "readback_verified" : "readback_mismatch",
      external_status: findings.length === 0 ? "submitted" : "unverified",
      mutation: {
        accepted_at: text(mutation.accepted_at),
      },
      readback: {
        status: text(readback.status),
        last_submitted: text(readback.last_submitted),
        last_downloaded: text(readback.last_downloaded),
        error_count: nonNegativeIntegerOrNull(readback.error_count),
        warning_count: nonNegativeIntegerOrNull(readback.warning_count),
      },
      validation: {
        status: findings.length === 0 ? "pass" : "fail",
        findings,
      },
    },
  };
}
