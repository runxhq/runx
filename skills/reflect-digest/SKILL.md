---
name: reflect-digest
description: Internal reflection stage that groups durable run signals by skill and emits validated handoffs to skill-build improve when evidence clears configured floors. Use diagnose-skill-run, run-history, or skill-build directly instead.
---

# Reflect Digest

Turn repeated post-run reflection signals into bounded improvement work without
re-diagnosing one receipt or drafting a parallel pull-request artifact.

The graph reads durable reflect events through `data-store`, admits explicit
replay projections when supplied, applies deterministic confidence and support
floors, then asks one bounded agent act to describe at most one improvement per
skill. A final deterministic step verifies every cited receipt belongs to that
skill's group and emits a `skill-build improve` handoff.

The skill does not write a package, open a pull request, publish, or invoke the
mutating improvement runner. The handoff preserves the target, objective,
receipt evidence, and non-goals for later governed execution.

## Composes

<!-- Generated from the native execution closure; run pnpm core-skills:composes:generate. -->

- `data-store#list_stream_heads`

## Inputs

- `reflect_projections`: explicit projections for replay; otherwise durable
  state is read from `data_source_ref` and `state_resource`.
- `skill_filter`, `since`: optional read bounds.
- `min_support`: minimum number of admitted projections for one skill.
- `min_confidence`: minimum confidence for each admitted projection.

## Outputs

- `proposals`: validated skill-specific improvement opportunities.
- `skill_build_handoffs`: executable request packets naming `skill-build`, runner
  `improve`, target directory, objective, primary receipt, evidence summary,
  and supporting receipt ids.

## Agent task contracts

### `reflect-digest`

Review each grouped reflection independently. Return at most one proposal per skill_ref, and
return an empty proposals array when no group supports a bounded fix. Every proposal must
contain skill_ref, repo-relative target_dir, objective, evidence_summary,
supporting_receipt_ids, and boundaries. Receipt ids must come from that group. Do not diagnose a
single run again, author code, draft a pull request, publish, or mutate the target package.
