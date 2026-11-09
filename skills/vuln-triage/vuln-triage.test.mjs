import assert from "node:assert/strict";
import test from "node:test";
import {
  admitScan,
  finalizeScan,
  admitAdvisory,
  finalizeAdvisory,
} from "./vuln-triage.mjs";
const D = "sha256:" + "a".repeat(64);
const f = {
  ecosystem: "npm",
  dependency: "example-lib",
  exact_version: "1.2.3",
  advisory_id: "OSV-1",
  summary: "Fixture vulnerability",
  dependency_paths: ["a"],
  affected: [
    {
      package: { name: "example-lib", ecosystem: "npm" },
      ranges: [{ events: [{ introduced: "0" }, { fixed: "1.2.4" }] }],
    },
  ],
};
function context(evidence = { findings: [f] }, scan_context = {}) {
  return admitScan({
    target: "service",
    evidence,
    evidence_digest: D,
    scan_context,
  }).triage_context;
}
const draft = (c) => ({
  decision: "remediate",
  operator_summary: {
    summary:
      "Test the supplied fix and investigate actual code-path reachability.",
  },
  advisory_assessments: c.findings.map((x) => ({
    finding_key: x.finding_key,
    priority: "high",
    confidence: "medium",
    exposure: "not_established",
    exposure_evidence: [],
    rationale:
      "The advisory affects a declared exact dependency; deployment reachability is unknown.",
    recommendation: "Test and deploy the advisory-declared fixed version.",
    fixed_version: "1.2.4",
  })),
});
test("actual findings work without fabricated verification or a sealed parent receipt", () => {
  const c = context();
  const r = finalizeScan({
    triage_context: c,
    risk_assessment_draft: draft(c),
  }).vulnerability_review;
  assert.equal(r.decision, "remediate");
  assert.equal(r.validation.status, "pass");
  assert.equal(r.advisories[0].assessment.evidence_status, "source_bound");
  assert.equal(r.evidence_binding.kind, "provided_findings");
});
test("confirmed exposure requires supplied source evidence", () => {
  const c = context();
  const d = draft(c);
  d.advisory_assessments[0].exposure = "confirmed";
  const r = finalizeScan({
    triage_context: c,
    risk_assessment_draft: d,
  }).vulnerability_review;
  assert.equal(r.decision, "needs_more_evidence");
  assert.equal(r.advisories[0].assessment.exposure, "not_established");
});
test("real context references and advisory fixed versions are preserved", () => {
  const c = context(undefined, {
    evidence: [
      {
        id: "trace-1",
        source: "local trace",
        excerpt: "Synthetic test-only vulnerable call path.",
      },
    ],
  });
  const d = draft(c);
  Object.assign(d.advisory_assessments[0], {
    exposure: "confirmed",
    exposure_evidence: ["trace-1"],
  });
  const r = finalizeScan({
    triage_context: c,
    risk_assessment_draft: d,
  }).vulnerability_review;
  assert.equal(r.validation.status, "pass");
  assert.equal(r.advisories[0].assessment.exposure, "confirmed");
  d.advisory_assessments[0].fixed_version = "999.0.0";
  assert.equal(
    finalizeScan({ triage_context: c, risk_assessment_draft: d })
      .vulnerability_review.validation.status,
    "fail",
  );
});
test("missing assessments preserve every finding instead of slicing at 500", () => {
  const c = context({
    findings: Array.from({ length: 601 }, (_, i) => ({
      ...f,
      advisory_id: "OSV-" + i,
    })),
  });
  const d = draft(c);
  d.advisory_assessments.pop();
  const r = finalizeScan({
    triage_context: c,
    risk_assessment_draft: d,
  }).vulnerability_review;
  assert.equal(r.advisories.length, 601);
  assert.equal(r.validation.status, "fail");
});
test("empty partial and later parts never become a clean overall scan", () => {
  const source = {
    schema: "runx/cve-audit.audit.v1",
    status: "partial",
    findings: [],
    findings_scope: "current_part",
    coverage: {
      complete: false,
      inventory_complete: true,
      omitted_components: 0,
    },
    previous_result_part: { digest: D },
    continuation: { digest: D },
  };
  const r = finalizeScan({
    triage_context: context(source),
  }).vulnerability_review;
  assert.equal(r.decision, "needs_more_evidence");
  assert.deepEqual(r.evidence_binding.continuation, source.continuation);
  source.status = "complete";
  source.coverage.complete = true;
  assert.equal(
    finalizeScan({ triage_context: context(source) }).vulnerability_review
      .decision,
    "needs_more_evidence",
  );
  source.findings_scope = "all_observed";
  source.previous_result_part = null;
  assert.equal(
    finalizeScan({ triage_context: context(source) }).vulnerability_review
      .decision,
    "no_matches_in_scope",
  );
});
test("unknown advisory ids cannot enter an unsent advisory", () => {
  const c = context();
  const review = finalizeScan({
    triage_context: c,
    risk_assessment_draft: draft(c),
  }).vulnerability_review;
  const ac = admitAdvisory({ triage_packet: review }).advisory_context;
  const result = finalizeAdvisory({
    advisory_context: ac,
    advisory_draft: {
      title: "Review",
      body: "Draft only.",
      affected_advisory_ids: ["OSV-other"],
    },
  }).vulnerability_advisory_packet;
  assert.equal(result.validation.status, "fail");
  assert.equal(result.publication_status, "not_published");
  assert.equal(result.provider_status, "not_called");
});
test("duplicate identities preserve alternate source observations and conflicting versions", () => {
  const c = context({
    findings: [
      { ...f, modified: "before" },
      {
        ...f,
        modified: "after",
        dependency_paths: ["b"],
        references: [{ url: "https://example.test/source" }],
      },
    ],
  });
  assert.equal(c.findings.length, 1);
  assert.deepEqual(c.findings[0].dependency_paths, ["a", "b"]);
  assert.equal(
    c.findings[0].additional_observations[0].references[0].url,
    "https://example.test/source",
  );
  assert.equal(c.path, "stop");
});
test("validation failures keep the operator verdict consistent with the review", () => {
  const c = context();
  const d = draft(c);
  d.operator_summary = {};
  const r = finalizeScan({
    triage_context: c,
    risk_assessment_draft: d,
  }).vulnerability_review;
  assert.equal(r.decision, "needs_more_evidence");
  assert.equal(r.operator_summary.verdict, r.decision);
});

test("paged locations remain references rather than invented empty path arrays", () => {
  const ref = {
    schema: "runx.artifact.v1",
    digest: D,
    ref: "runx:project-artifact:" + D,
    size_bytes: 10,
    media_type: "application/json",
    path: "/synthetic",
    labels: { sharing: "configured_model" },
  };
  const finding = {
    ...f,
    dependency_locations: {
      count: 2500,
      preview: ["a"],
      complete: false,
      components_artifact: ref,
    },
  };
  delete finding.dependency_paths;
  const c = context({
    schema: "runx/cve-audit.audit.v1",
    status: "partial",
    findings: [finding],
    components_artifact: ref,
    inventory_artifact: ref,
  });
  assert.deepEqual(
    c.findings[0].dependency_locations,
    finding.dependency_locations,
  );
  assert.equal("dependency_paths" in c.findings[0], false);
  assert.deepEqual(c.source.components_artifact, ref);
  assert.deepEqual(c.source.inventory_artifact, ref);
});
