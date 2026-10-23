import { finding, text, webUrl } from "./shared.mjs";

export function classifyIndexingRequest(inputs) {
  const url = text(inputs.url);
  const resourceType = text(inputs.resource_type).toLowerCase().replaceAll("-", "_");
  const eligible = new Set(["job_posting", "broadcast_event"]);
  const findings = [];

  if (!webUrl(url)) {
    findings.push(finding("gsc.indexing.url_invalid", "url must be an absolute HTTP(S) URL"));
  }

  const specialist = findings.length === 0 && eligible.has(resourceType);
  return {
    indexing_admission: {
      schema: "runx/google-search-console.indexing_admission.v1",
      decision: findings.length > 0 ? "blocked" : specialist ? "specialist_required" : "refused",
      reason_code: findings.length > 0
        ? "invalid_request"
        : specialist
          ? "restricted_api_specialist_review"
          : "unsupported_resource_type",
      url,
      resource_type: resourceType,
      operator_reason: text(inputs.reason),
      provider_status: "not_called",
      external_status: "not_requested",
      downstream_handoff: specialist
        ? {
            state: "specialist_review_required",
            expected_outcome: "confirm Google Indexing API eligibility and use a separately governed implementation",
          }
        : {},
      validation: {
        status: findings.length === 0 ? "pass" : "fail",
        findings,
      },
    },
  };
}
