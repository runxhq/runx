# Exa

`search.query` performs one public-web discovery request through native HTTP.
It requires a query and result limit (1–20); domain and publication-date filters
are optional. The response retains provider request/query metadata, returned
URLs and reported cost. Search results identify sources; they do not prove the
claims in those sources. Fetch selected pages through the existing governed
web-fetch path before relying on their contents.

The native credential store owns the key. Exa documents bearer authentication
as an alternative to `x-api-key`, so this implementation uses the existing
native bearer presentation. There is no package HTTP client, credential loader,
alternate-provider fallback or automatic query expansion.

Calls can consume provider credits. This package does not declare them free or
offer a Runx-owned account. Native per-call provider-cost enforcement and hosted
API-key execution remain shared-core dependencies; this initial placement is
local/BYO. A result cap bounds returned items, not the provider's monetary bill.
Do not repeat an uncertain charged request merely because it was a read.
The shared HTTP driver sends each provider request once, reads included: it
never retries one on its own. A request whose answer did not arrive may still
have been charged, and a later run that sends it again is a second charge.
Fixture execution and manifest validation do not establish safe paid-call
recovery.

[Provider search/authentication contract](https://exa.ai/docs/reference/search).
