---
name: vuln-triage
description: Turn verified exact-version vulnerability evidence into bounded remediation and advisory packets without inventing exposure or claiming publication.
---

# Vulnerability Triage

Translate verified vulnerability identities into an operator decision: what is
affected, what exposure is established or unknown, how urgent the response is,
what remediation is justified, and whether an advisory should be prepared. This
is the judgment layer of the security chain, not another scanner.

Use `cve-audit` first to prove the exact dependency versions and OSV advisory
identities. `vuln-triage` then reasons about operational exposure and priority
without changing those facts. Its advisory packet is the end of the chain:
publishing an advisory is the operator's step, outside runx.

## Runners

`scan` accepts the exact CVE audit result, its independent verification packet,
and the sealed receipt id. Every assessment remains bound to dependency,
installed version, and advisory id. Judgment may classify priority, distinguish
established from unknown exposure, and propose remediation, but confidence must
describe only that judgment—the CVE identity is already verified.

`advisory` accepts only a validated triage packet. It prepares precise wording
for the admitted advisory ids and returns a review packet with
`publication_status: not_published`. It does not call a repository, advisory,
email, or social provider.

## Operating standard

- Keep verified identity, exposure judgment, and remediation confidence
  separate.
- State affected versions, preconditions, impact, and mitigations precisely.
- Treat unknown exposure as a reason to investigate or escalate, not as proof of
  exploitation or safety.
- Avoid alarmism and vague severity labels. Explain the operational reason for
  priority and the evidence still missing.
- Preserve every verified finding. Adding a new CVE or silently omitting one
  invalidates the packet.

The finalized triage packet adds deterministic escalation criteria for high
priority, unknown exposure, and low-confidence judgments. A clean audit returns
`no_verified_findings`; it does not certify the target secure.

## Inputs and result

The scan runner consumes the target-bound audit result, verification result, and
receipt id plus any bounded operational context needed to assess
exposure. It returns one assessment per verified finding, a remediation plan,
operator summary, evidence binding, confidence, and escalation criteria.

The advisory runner consumes that validated triage packet and returns a title,
summary, body, exact affected advisory ids, and disclosure checklist. That
artifact is a draft for review, not external publication.

## Stop conditions

- Return `needs_verified_evidence` when the audit, independent verification, or
  receipt id is missing or invalid.
- Return `needs_more_evidence` when an assessment adds or omits a finding,
  changes a version or advisory identity, or lacks bounded confidence.
- Reject advisory wording that cites an unknown id or states exposure the
  triage packet did not establish.
- Never mutate the target, query another provider, repair dependencies, or
  claim disclosure.

## Example

An audit proves that exact version `x.y.z` matches one OSV advisory. Triage may
find that the vulnerable code path is reachable, assign high priority, and
recommend a tested upgrade; or it may record exposure as unknown and request a
named runtime trace. It cannot add a related CVE from memory. The advisory draft
may describe only the admitted finding and the remediation evidence present in
the packet.

## Agent task contracts

### `vuln-triage-assess`

Assess only the verified finding identities. Return one assessment per exact
dependency, version, and advisory id plus the remediation plan and operator
summary. State high, medium, or low confidence for exposure and priority
judgment. Do not add CVEs, versions, provider claims, or publication claims.

### `vuln-triage-advisory`

Draft a precise advisory using only the admitted advisory ids, target,
remediation, and evidence binding. Return the affected ids and disclosure
checklist with the draft. Do not claim publication, provider execution, or
exposure that the triage packet does not establish.
