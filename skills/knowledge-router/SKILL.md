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
The owning host selects its maintained index and passes the relevant bounded
neighborhood here once. Optional source `locator` fields may name local paths,
URLs or provider references; they are data, never new read authority. Keep the
exact input catalog with the result so its selected IDs can be resolved without
rediscovering the index. This skill neither creates a shadow index nor reads the
selected sources.

The route may return no follow-up skill when more context or manual review is
the honest outcome. An owner recommendation must match an exact domain that
owner declares in the catalog. If ownership is unknown, leave it null rather
than assigning the closest team.

## Procedure

1. Match the question or event to supplied source summaries and owner domains.
2. Propose the smallest source set needed to resolve the question.
3. Select an owner only when the catalog supports the ownership relationship.
4. Select a follow-up skill only when its declared purpose matches the next job.
5. Use `manual_review` for consequential legal, billing, security, privacy, or
   destructive decisions—not merely because the topic belongs to those teams.
6. Validate every source, owner, and skill reference, source-match consistency,
   declared owner domain and supplied selection limits deterministically.
7. Return `needs_more_context` when no supported route exists.

## Selection limits

`constraints` is a closed optional object. `allowed_sources`, `allowed_owners`
and `allowed_skills` narrow selection to exact catalog IDs. An omitted list adds
no restriction; an empty list allows none of that kind. `excluded_sources`
removes exact sources even when an allowlist also includes them. Every policy
reference must exist in this catalog. `notes` supplies advisory context; it is
not an executable policy or authority grant. Unsupported constraint keys are
rejected by input admission rather than echoed as enforced.

A valid source outside a declared limit still refuses. The result never
converts a failed constraint or unverified ownership into a successful route.
For `needs_more_context`, there are no selected sources, matches, owner or next
skill. Each selected source must otherwise have exactly one matching entry.

## Continue the route

Resolve selected IDs against the exact input catalog bound by
`validation.catalog_digest`. Pass the original question, selected rows and their
locators, relevant constraints and route rationale to the selected reader or
research lane. For local repository evidence, `prior-art` can inspect the exact
paths without broad rediscovery. For captured external evidence, use the normal
source acquisition and research owners. Keep their actual evidence and answer
separate from this route. No source was fetched, question answered, worker
dispatched or message sent by a routed result.

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
- `constraints` (optional): exact source/owner/skill selection limits and
  advisory notes. Constraints are not authority.

## Agent task contracts

### `knowledge-router-propose`

Route the question through only the supplied knowledge catalog. Return route_proposal with
verdict, route, source_matches, owner_recommendation, and next_skill. Every selected source_id,
owner_id, and skill must exactly match a supplied id. Choose the smallest useful source
set. Use needs_more_context with no selected ids when the catalog cannot support a route. Use
manual_review for consequential legal, billing, security, privacy, or destructive decisions, not
merely because the topic belongs to those domains. Do not answer the question or perform the
follow-up skill.
