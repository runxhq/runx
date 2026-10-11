---
name: policy-draft
description: Draft or tighten a Runx operational policy from a governance brief, then validate the exact runx.operational_policy.v1 contract with the native policy engine. Use for repository/source/runner/owner policy design, fail-closed policy linting, or authority attenuation; never use this lane to widen an existing policy.
---

# Policy Draft

Produce one reviewable operational-policy proposal. Let the agent translate
intent into policy fields; let the native `policy.lint` tool decide whether the
exact in-memory result is a valid Runx policy.

## Workflow

1. Require concrete target repositories, source locators, owner routes, and an
   available runner. Return `needs_input` when any are unresolved.
2. Draft the smallest `runx.operational_policy.v1` that admits the stated work.
   Unspecified work remains denied.
3. When `existing_policy` is supplied, preserve its target/source/runner set and
   only attenuate authority. Do not add locators, actions, targets, sources, or
   runners; do not lower confidence or weaken human gates.
4. Pass the draft to the deterministic validation step. Never author or trust an
   agent-supplied lint verdict.
5. Return `ready` only when native lint passes. A native failure or attempted
   widening returns `reject` with structured findings.

## Native policy contract

Draft the exact contract consumed by `policy.lint` and the Runx policy parser:

```yaml
schema: runx.operational_policy.v1
schema_version: runx.operational_policy.v1
policy_id: string
sources:
  - source_id: string
    provider: string
    allowed_locators: [string]
    allowed_actions: [string]
    source_thread:
      required: boolean
      publish_mode: reply | comment | none
      missing_behavior: fail_closed
    minimum_confidence: number     # optional
runners:
  - runner_id: string
    kind: string
    state: available | unavailable
    allowed_actions: [string]
    target_repos: [owner/repo]
    scafld_required: boolean
owner_routes:
  - route_id: string
    owners: [string]
    target_repos: [owner/repo]
targets:
  - repo: owner/repo
    runner_ids: [string]
    allowed_actions: [string]
    default_owner_route: string
    scafld_required: boolean
    base_branch: string            # optional
dedupe:
  strategy: source_fingerprint
  key_fields: [string]
  on_duplicate: reuse | comment | block
outcomes:
  observe_provider: boolean
  verification_required: boolean
  close_source_issue: never | when_verified | always
  publish_final_source_thread_update: boolean
permissions:
  auto_merge: boolean
  mutate_target_repo: boolean
  require_human_merge_gate: boolean
```

## Stop conditions

- Missing repository, locator, runner, or owner evidence: `needs_input` with
  `policy: null`; native lint is not run.
- Proposed widening of an existing policy: `reject` before lint and identify the
  widened path.
- Native parse or validation failure: `reject`; never relabel it as ready.
- A request to widen authority: stop and route it to a separate explicit policy
  decision. This skill's existing-policy lane is attenuation-only.

## Output

The final `policy_proposal` contains:

```yaml
decision: ready | needs_input | reject
policy: object | null
validation:
  status: pass | fail | not_run
  engine: runx policy
  findings: [{ code, path, message }]
  readback: object | null
  reason: string
rationale: string
blockers: [string]
needs_input: [string]
success_checkpoint: object
```

The runtime receipt proves which draft was validated. It does not approve or
install the policy.

## Agent task contracts

### `policy-draft-draft`

Draft one policy proposal using the exact runx.operational_policy.v1 contract described by the
skill. Return decision, policy, rationale, blockers, needs_input, and success_checkpoint. Do not
author a lint verdict: the next deterministic step invokes native `policy.lint`. If required
repos, source locators, owners, or runner bindings are missing, return needs_input with policy
null. When existing_policy is supplied, never add authority, targets, sources, runners,
locators, or actions; only tighten permissions, confidence, routing, and outcomes.
