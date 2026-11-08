---
name: vuln-triage
description: Prioritize actual exact-coordinate vulnerability findings, preserve unknown exposure and partial coverage, and return practical remediation or an unsent advisory draft.
---

# Vulnerability triage

Turn dependency findings into an operational decision: what deserves attention,
what exposure is established, what remains unknown, and which remediation is
justified. This is the judgment step after evidence acquisition. It does not
scan a repository, install packages, modify code or publish an advisory.

Use the default `scan` with `target`, actual `evidence` and any relevant
`scan_context`. Evidence can be a current `runx/cve-audit.audit.v1` packet or a
findings object from another source. Each finding needs `dependency`,
`exact_version` and `advisory_id`; retain ecosystem, source URL, affected ranges,
severity, paths and original provenance when available. A CVE finding's `dependency_locations` may be
a bounded preview; preserve its count, completeness flag and component artifact.
Use the producer's paged `locations` phase before treating that preview as the
complete affected-location set. An audit delivery digest
or `verified: true` is not a finding. Use `cve-audit` when the missing work is
actually reading a lockfile and querying OSV; use `sbom-generate` for inventory
and CycloneDX output.

## Keep facts, exposure and judgment separate

An exact-coordinate match says that the source associates an advisory with a
package version. It does not establish that the vulnerable function is reachable
in this deployment, that the package is installed at runtime, or that an exploit
has occurred. The review preserves the supplied source and binds its exact bytes
with a native digest. That binds the input; it does not independently verify a
provider, a caller's receipt id or the truth of a supplied deployment claim.

Describe relevant deployment observations in `scan_context.evidence`, with an
`id`, a `source` and an `excerpt` for each. A confirmed-exposure assessment must
reference those ids in `exposure_evidence`. Internet-facing status alone is not
proof that a vulnerable code path is used. If the necessary trace, call site,
configuration or runtime evidence is missing, retain `not_established` and name
what would settle it. Confidence describes the judgment, not cryptographic proof.

Prioritize from impact, reachability, deployment role, available mitigations and
the cost of delaying a fix. Do not repeat a severity label as the entire
rationale. Give each finding a concrete next action: validate an upgrade, inspect
a named call site, apply an evidenced mitigation, or obtain the missing trace.
An exact `fixed_version` must occur in the affected package's supplied advisory
events; otherwise describe version verification as pending. Do not invent a
release or assume that the highest suggested version is compatible.

## Use it directly or inside a graph

The default returns `vulnerability_review` (`runx/vuln-triage.scan.v1`), with
the original findings, one assessment and recommendation per coordinate/advisory
identity, escalation criteria, useful operator summary and source coverage.
Repeated identities keep their locations together. No arbitrary finding count
is silently discarded. Runtime byte and model-turn budgets still apply; for a
large audit, consume its bounded result parts and retain the overall scope.

A parent can pass a current CVE result directly while its graph is still open.
There is no requirement to fabricate a sealed parent receipt first. The final
parent receipt records the actual provider and triage steps. If an independent
consumer needs sealed evidence, verify the original receipt tree through native
receipt tools and retrieve the declared claim; a receipt proves recorded claim
integrity, not external truth.

Partial scans are useful: triage the known findings now and keep the scan's
continuation and previous result-part references. Inline findings may cover only
one part. An empty part is not a clean overall review. `no_matches_in_scope` is
available only for a complete selected scan with full inventory coverage, no
omissions and all observations present. Unknown exposure remains unknown even
when scan coverage is complete. The review never certifies a project secure.

## Advisory drafting and recovery

`advisory` takes the exact validated review and an audience, then prepares a
title, summary, body, affected advisory ids and disclosure checklist. It reuses
the findings and remediation rather than rescanning or reclassifying the source.
The result is `ready_for_review`, `not_published` and `not_called`; no repository,
email or other provider is invoked. Publication is a separate authorized action.

Missing identities, omitted assessments, unbound exposure references and invented
fix versions produce `needs_more_evidence` while retaining the known findings.
Correct the precise missing input or assessment. Continue the original audit or
read its earlier result parts when coverage is incomplete. Never replace missing
evidence with a receipt-shaped string, silently shorten the findings list, or
report an incomplete scan as a negative result.

## Agent task: vuln-triage-assess

Use `triage_context.findings` and the supplied deployment context. Return
`risk_assessment_draft` with `decision` (`remediate`, `monitor` or `needs_human`),
`operator_summary.summary`, and `advisory_assessments`. Each assessment contains
the unchanged `finding_key`, `priority` (`urgent`, `high`, `normal`, `low` or
`unknown`), `confidence` (`high`, `medium`, `low`), `exposure` (`confirmed` or
`not_established`), `exposure_evidence` ids, a substantive `rationale` and a
concrete `recommendation`. Add `fixed_version` only from the matching supplied
advisory's fixed events. Cover every finding once; do not invent findings,
deployment evidence, independent verification or completed remediation.

## Agent task: vuln-triage-advisory

Return `advisory_draft` with title, summary, body, affected_advisory_ids and
disclosure_checklist. Cite only admitted ids. Preserve partial coverage and
unknown exposure; do not turn a suggested upgrade into a completed deployment.
Do not claim publication or provider execution.
