---
name: vuln-disclosure
description: Prepare an evidence-bound vulnerability publication and publish the exact approved advisory through any compatible provider binding with independent readback.
---

# Vulnerability Disclosure

Publish a reviewed security advisory through a configured provider without
pretending an attempted publication succeeded. The default prepares the exact
payload, gates the live publication, and requires independent readback;
`vuln-disclosure` remains the explicit preparation runner and `publish` accepts
its typed packet in composed use. Where `cve-audit` proves
exact vulnerability identities and `vuln-triage` decides exposure,
remediation, and wording, this skill checks whether one exact advisory is ready
to enter a consequential provider lane.

Disclosure deserves a distinct boundary because a technically correct finding
can still be premature, unsafe, mistargeted, or unhelpful to affected users.
Embargo, remediation availability, channel, audience, and disclosure authority
must all be visible before publication approval is requested.

## How it works

1. Admit one validated `vuln-triage` advisory packet and preserve its evidence
   binding and exact advisory ids.
2. Review the specified channel, target, embargo posture, remediation context,
   and disclosure checklist.
3. Decide `hold` or `ready_for_publication_approval`. The review may not add or
   omit affected advisory ids or silently rewrite the payload.
4. Deterministically package a digest-bound outbox packet for the named target.
5. The default runner stops with `publication_status: not_published` and
   `provider_status: not_called`.
6. `publish` recomputes the payload digest, requests approval for that exact
   packet, invokes `advisory.publish` under one idempotency key, and then calls
   `advisory.read`. The returned advisory ref and payload digest must match.

Safe review and packaging need no human approval because they remain local. The
native provider lane owns the human gate, idempotency, configured binding,
publication request, and stable readback. No provider token or HTTP client lives
in this package. Until the independent read succeeds, no receipt may describe
the advisory as live.

The binding may be local, self-hosted, third-party, or Runx-hosted. This skill
does not own credential acquisition, tenant selection, or a connector vendor.

## When to use it

Use this skill only after triage has produced a validated advisory and the
intended publication channel and target are known. Keep the result on hold when
affected scope, remediation, embargo timing, maintainer coordination, or
authority remains unclear. If public disclosure would not materially help
affected users, preserve the remediation packet instead of manufacturing a
publication milestone.

## Inputs and result

The planning input is the validated advisory packet plus exact channel, target,
embargo, remediation, and review context. Its result is either a reasoned hold
or a digest-bound publication outbox with no delivery claim. `publish` accepts
only that ready packet, the expected provider, and one stable idempotency key;
its native mutation and readback packets are the publication evidence.

## Stop conditions

- Stop on invalid source validation, missing evidence binding, unknown advisory
  ids, or an imprecise publication target.
- Hold when remediation or coordinated-disclosure posture is not ready.
- Do not broaden affected scope, embellish impact, or move private speculation
  into the public payload.
- Refuse a missing, ambiguous, wrong-provider, or under-scoped provider binding;
  never fall back to a raw token or package request client.
- Never interpret local review as publication approval or provider success.
  Provider acknowledgement without matching `advisory.read` is incomplete.

## Example

A validated triage packet names two advisories, affected versions, and a tested
upgrade. Disclosure review can hold the packet until the maintainer confirms an
embargo date or package it for a specific GitHub advisory target. If only one of
the two ids appears in the review output, finalization fails. Once ready,
`publish` still requires exact human approval and a matching provider read
before the advisory is treated as live.

## Agent task contract

### `vuln-disclosure-review`

Decide whether the exact evidence-bound advisory should be held or sent to a
publication approval boundary. Return rationale, the exact admitted advisory
ids, and checklist. Do not rewrite the payload, approve publication, call a
provider, or claim publication happened.
