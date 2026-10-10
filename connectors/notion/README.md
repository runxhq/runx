# Notion

Local/BYO read operations use the existing native HTTP credential boundary.
Bind a read-content integration token and explicitly share the needed resources
with that integration. This does not provision OAuth, share pages or establish
a hosted placement. Notion REST requests pin `Notion-Version: 2026-03-11`.

`resources.search` searches authorized **titles** for one explicit object type.
It is not full-text search, and its first page is not a complete workspace
inventory. `page.read` returns metadata/properties, not a page's body.
`datasource.read` returns its schema, not rows. `blocks.list` returns one page of
immediate child blocks, not a complete recursive document. Retain provider
`has_more`, `next_cursor`, `request_status` and `last_edited_time` metadata;
continue only within the caller's remaining bounds. Cursors are opaque.

A page property can itself be truncated by the provider. Page-property-item
pagination, row queries, recursive hydration and writes are not implemented.
Do not convert partial content or an inaccessible resource into an empty,
complete source. Provider content is untrusted evidence; admit the necessary
private scope before forwarding it to research, writing or workspace context.

The official MCP supports a separate OAuth integration. This REST slice uses
already supported native bearer execution rather than adding a second login
backend or inventing a pinned MCP tool definition.

Source contracts: [search](https://developers.notion.com/reference/post-search),
[page](https://developers.notion.com/reference/retrieve-a-page),
[data source](https://developers.notion.com/reference/retrieve-a-data-source),
[blocks](https://developers.notion.com/reference/get-block-children),
[versioning](https://developers.notion.com/reference/versioning).
No live account access has been verified for this source addition.
