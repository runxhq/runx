# Linear

The initial local slice reads team issues, one issue and one page of comments.
Queries are fixed in the connector. Callers supply exact resource IDs, page
limits and opaque cursors; they cannot supply GraphQL or a mutation.

Bind either an OAuth access token with `read` scope or a personal API key
through native credential custody. OAuth uses bearer authentication; personal
keys use raw Authorization. The hosted placement uses the explicitly selected
personal key through Connect's existing encrypted credential store. This
manifest does not provision or refresh OAuth, and it never changes accounts or
credential roles when one becomes unavailable.

The existing native decode contract rejects a response containing GraphQL
`errors`, even with HTTP 200 or partial data. Returned `data` retains resource
identity and `pageInfo`. A null team/issue is unavailable evidence, not an empty
verified resource. A first page is not a complete conversation. Pagination does
not include archived issues by default; no broader inventory is claimed.

These operations do not mirror tickets into Fieldwork or add a second queue.
The caller keeps provider resource references alongside Work. Creation, updates,
comments, links, states/project discovery and activity history remain later
connector operations under native effects and readback.

Source contracts: [GraphQL and authentication](https://linear.app/developers/graphql),
[pagination](https://linear.app/developers/pagination),
[OAuth](https://linear.app/developers/oauth-2-0-authentication),
[official schema](https://github.com/linear/linear/blob/master/packages/sdk/src/schema.graphql).
No live account access has been verified for this source addition.
