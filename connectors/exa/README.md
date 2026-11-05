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

Each call is quoted at its documented ceiling of $0.014: Exa bills search at $4
per 1,000 requests up to 10 results, plus $1 per 1,000 for each result above
that, and this operation returns at most 20. Calls draw on your own Exa account;
Runx offers no account of its own, and hosted API-key execution is not available
yet. The quote is what a money limit reserves, not the amount Exa bills, which
the response reports. A request whose answer did not arrive may still have been
charged, so the run holds it rather than sending it again.

[Provider search/authentication contract](https://exa.ai/docs/reference/search),
[pricing](https://exa.ai/pricing).
