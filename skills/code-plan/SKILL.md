---
name: code-plan
description: Compile a bounded objective or code-intake change set into a dependency-safe Runx execution plan, with deterministic checks for evidence preservation, mutation boundaries, catalog references, and skill-package ownership. Use before multi-step or cross-surface work; this skill plans but does not execute.
---

# Code Plan

Turn one objective into a plan that downstream runners can consume without reinterpreting the source request.

## Procedure

1. Start from `objective`. When `change_set` is supplied by `code-intake`, preserve it exactly; do not rewrite its target surfaces, invariants, success criteria, or commencement decision.
2. Classify the plan as `workspace_change` or `skill_package`. Skill-package authoring belongs to `skill-build`; do not route it through a generic repo-change step.
3. Split work at authority, mutation, dependency, and review boundaries. Keep steps coarse enough to produce a meaningful artifact.
4. Declare ordered phases and repo change requests. Parallel work must have no dependency or shared mutation target.
5. Declare orchestration steps with exact skill references, scopes, mutation flags, inputs, and prior-step context references.
6. Return `blocked` when a question must be answered before mutation. Never turn uncertainty into an executable plan.
7. Native catalog inspection supplies the actual installed skill set. The
   domain validator then verifies source preservation, ordered DAGs, mutation
   declarations, context references, catalog ownership, and the `skill-build`
   boundary. Invalid candidates are withheld and returned as a blocked packet.

This skill does not approve or execute mutations. A `mutating: true` field describes authority a future step will need.

## Output

```yaml
decision: ready | blocked
plan_kind: workspace_change | skill_package
change_set: object
objective_summary: string
workspace_change_plan: object
orchestration_steps: array
required_skills: array
open_questions: array
evidence: object
validation:
  status: pass | hold
  findings: array
```

`workspace_change_plan` contains `plan_id`, `change_set_id`, `objective_summary`, shared invariants, success criteria, ordered phases, integration checks, and open questions. Each phase contains ordered repo change requests with dependencies, validation commands, and mutation declarations.

`evidence.source_change_set_status` is `preserved`, `drifted`, or
`not_supplied`; `source_change_set_preserved: false` is never a successful
preservation claim. When a caller supplies source context, drift or loss blocks
the plan.

Inputs are `objective`, optional `project_context`, `thread_locator`, `thread`, `change_set`, and `harness_context`. `harness_context` is passed through from the caller; the agent does not reconstruct or advance it.

## Agent task contracts

### `code-plan-draft`

Return work_plan_draft with decision, plan_kind, change_set, objective_summary,
workspace_change_plan, orchestration_steps, required_skills, and open_questions. Preserve a
supplied change_set exactly. Split at authority, mutation, dependency, and review boundaries; do
not split for cognitive convenience. Every phase, repo request, and orchestration step needs an
id, ordered dependencies, and an explicit mutation flag. A ready plan has no blocking open
questions. Use plan_kind skill_package for Runx skill authoring and route that work through
skill-build. This runner plans only and never claims downstream execution.
