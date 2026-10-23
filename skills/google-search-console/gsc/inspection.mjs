import {
  digest,
  finding,
  object,
  propertyCovers,
  stringArray,
  text,
  validProperty,
  webUrl,
} from "./shared.mjs";

export function normalizeInspection(inputs) {
  const supplied = object(inputs.provider_result);
  const expected = object(inputs.request);
  const findings = [];
  const property = text(supplied.property || expected.property);
  const inspectionUrl = text(supplied.inspection_url || expected.inspection_url);

  if (!validProperty(property)) {
    findings.push(finding("gsc.property.invalid", "property must be an HTTP(S) URL-prefix or sc-domain property"));
  }
  if (!webUrl(inspectionUrl)) {
    findings.push(finding("gsc.inspection_url.invalid", "inspection_url must be an absolute HTTP(S) URL"));
  }
  if (property && inspectionUrl && !propertyCovers(property, inspectionUrl)) {
    findings.push(finding("gsc.inspection_url.outside_property", "inspection_url is not covered by the property"));
  }
  for (const field of ["property", "inspection_url"]) {
    if (text(expected[field]) && text(supplied[field]) && text(expected[field]) !== text(supplied[field])) {
      findings.push(finding(`gsc.inspection.${field}_mismatch`, `supplied ${field} does not match the request`));
    }
  }

  return {
    inspection_draft: {
      schema: "runx/google-search-console.url_inspection_evidence.v1",
      decision: findings.length === 0 ? "ready" : "blocked",
      provider: "google-search-console",
      provider_status: "readback_verified",
      property,
      inspection_url: inspectionUrl,
      index_status: {
        verdict: text(supplied.verdict),
        coverage_state: text(supplied.coverage_state),
        robots_txt_state: text(supplied.robots_txt_state),
        indexing_state: text(supplied.indexing_state),
        page_fetch_state: text(supplied.page_fetch_state),
        crawled_as: text(supplied.crawled_as),
        last_crawl_time: text(supplied.last_crawl_time),
        referring_urls: stringArray(supplied.referring_urls),
        sitemap: stringArray(supplied.sitemap),
      },
      amp: object(supplied.amp),
      mobile_usability: object(supplied.mobile_usability),
      rich_results: object(supplied.rich_results),
      inspection_link: text(supplied.inspection_link),
      fetched_at: text(supplied.fetched_at),
      validation: {
        status: findings.length === 0 ? "pass" : "fail",
        findings,
      },
    },
  };
}

export function finalizeInspection(inputs) {
  const draft = object(inputs.inspection_draft);
  return {
    inspection_evidence: {
      ...draft,
      evidence_digest: digest(inputs.digest_result),
    },
  };
}
