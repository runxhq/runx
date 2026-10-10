---
name: web-search
description: Discover a bounded set of fresh web source candidates from one explicit search query, preserve provider evidence and reported cost, and prepare source selection for fetching and research.
---

# Web search

Discover sources for a specific question. The initial `search` runner uses the
selected Exa account for one query and at most 20 results. It normalizes source
identity, removes exact duplicate URLs, rejects malformed non-web candidates
and preserves the provider operation alongside the result. It does not silently
choose another provider, execute extra queries, fetch full pages or write a
research conclusion. Exa is the initial executable backend; additional backends
need their own declared operations and result mapping.

## Search procedure

Choose one concrete information need per query. Include the relevant entity,
decision and discriminating terms. Prefer direct evidence: official documentation,
first-party announcements, original reports or research. Use domain restrictions
when the task requires authoritative sources. Use publication-date bounds for a
time-sensitive question, remembering that a publication date is not necessarily
the event date. Do not add filters that exclude the contrary evidence you need.

Limit queries and results to the caller's actual budget. Search may consume paid
credits. The result includes reported cost when available; missing cost remains
unknown. The result cap is not a monetary spending limit. This package adds no
billing authority or automatic retry after an uncertain paid request.

## Result and composition

`search_result` contains the query, provider request ID, usable source candidates,
discard counts and reported cost. `provider_operation` retains the exact native
provider evidence and account binding. An empty provider result returns
`no_results`; a nonempty response with no usable candidates returns
`unusable_results`. Neither means the question has been researched exhaustively.

Titles, dates and any returned excerpt are discovery metadata, not verified claims
or full page content. Fetch selected URLs through `web-fetch` and feed the actual
retrieved packets to `research`. Check contradictions, source diversity and
coverage before drawing conclusions. `research` over supplied evidence does not
perform fresh discovery by itself. Reuse adequate existing evidence rather than
repeating a paid search.
