---
name: knowledge-router
description: Route one question or source event through a supplied knowledge catalog to validated source, owner, escalation, and follow-up skill references. Use for evidence discovery and ownership routing, not broad business-action fanout or answering the question itself.
---

# Knowledge Router

Produce a validated retrieval and ownership route from a bounded source
catalog. This skill decides where evidence should come from and who owns the
answer; it does not answer the question or perform the follow-up operation.

`knowledge-router` resolves references inside an explicit knowledge catalog. Its deterministic validator rejects
invented sources, owners, and skills.

## Source catalog

Supply:

```json
{
  "sources": [
    {"id": "auth-docs", "kind": "docs", "summary": "API authentication and key scopes"}
  ],
  "owners": [
    {"id": "security-team", "domains": ["authentication"]}
  ],
  "skills": [
    {"id": "research", "purpose": "Resolve a bounded source-backed question"}
  ]
}
```

Every selected reference must exist in the corresponding catalog collection.
The route may return no follow-up skill when more context or manual review is
the honest outcome.

## Procedure

1. Match the question or event to supplied source summaries and owner domains.
2. Propose the smallest source set needed to resolve the question.
3. Select an owner only when the catalog supports the ownership relationship.
4. Select a follow-up skill only when its declared purpose matches the next job.
5. Use `manual_review` for consequential legal, billing, security, privacy, or
   destructive decisions—not merely because the topic belongs to those teams.
6. Validate every source, owner, and skill reference deterministically.
7. Return `needs_more_context` when no supported route exists.

## Output

- `route`: domain, rationale, and validated source references.
- `source_matches`: validated source references and matching signals.
- `owner_recommendation`: validated owner reference, rationale, and escalation
  posture, or `null`.
- `next_skill`: validated skill reference and rationale, or `null`.
- `verdict`: `routed`, `needs_more_context`, `manual_review`, or the
  deterministic validator's `refused` result for an invented reference.
- `validation`: catalog digest inputs, invalid-reference findings, and result.

## Inputs

- `question` (required): question, event, or thread summary to route.
- `available_sources` (required): explicit `sources`, `owners`, and `skills`
  collections.
- `constraints` (optional): allowed sources, sensitivity, excluded systems, or
  preferred owners. Constraints are not authority.

## Agent task contracts

### `knowledge-router-propose`

Route the question through only the supplied knowledge catalog. Return route_proposal with
verdict, route, source_matches, owner_recommendation, and next_skill. Every selected source_id,
owner_id, and skill must exactly match a supplied id. Choose the smallest useful source
set. Use needs_more_context with no selected ids when the catalog cannot support a route. Use
manual_review for consequential legal, billing, security, privacy, or destructive decisions, not
merely because the topic belongs to those domains. Do not answer the question or perform the
follow-up skill.
