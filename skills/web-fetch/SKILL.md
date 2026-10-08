---
name: web-fetch
description: Fetch and extract one public web source, optionally restrict its hosts, and return the content by digest with full provenance.
---

# Web Fetch

Fetch one URL, prove it was allowed, extract the part the caller asked for, and
return that slice by digest with the provenance needed to trust it later.

## What this skill does

`web-fetch` retrieves a single public URL,
extracts text, metadata, or links, and seals the result so a downstream step can
cite the fetch without re-fetching. It checks the final host against the
effective host policy before and after redirects, reads the page whole, and returns
the final URL, the HTTP status, a `content_digest` over the retrieved body, the
extracted slice, and a provenance block recording when it ran, every redirect
hop, and how many bytes it read. The body is referenced by digest; only the
extracted slice is inlined.

This is the primitive an agent reaches for when it has already decided which page
to read. The decision it makes easier is "can I read this page, and what did it
actually say", with the answer backed by a digest instead of a remembered
paraphrase. The calling agent decides which sources matter and synthesizes
across them; `web-fetch` retrieves exactly one source. By default it may follow
that source across any publicly routable host. Supply `allowlist` only when the
operator needs to narrow the initial request and every redirect to named hosts.

The request path is native Runx code. Skills do not supply a JavaScript HTTP
client: the runtime owns public-network enforcement, redirect admission,
timeouts, retries, the operator's page ceiling, response digests, and
receipt-safe evidence.

## When to use this skill

- An agent has chosen a specific page and needs its content bound to a
  `content_digest` so a later step can cite it without re-fetching.
- A research pass needs each source retrieved through one bounded public-network
  fetch with a complete redirect chain and byte count.
- A review must later prove what a page said at fetch time.
- A follow-on governed operation needs one source extracted as
  `text`, `metadata`, or `links`.

## When not to use this skill

- To judge, rank, or synthesize across sources. That remains caller reasoning;
  `web-fetch` retrieves exactly one source and refuses to reason over many.
- To reach a private, loopback, link-local, or otherwise non-public address.
- To reach a host outside an explicitly supplied `allowlist`, including a host
  reached only through a redirect.
- To write anything. The only scope is `web.read`; there is no repo, file,
  wallet, or send authority here.
- To inline a large raw body. The extracted slice is the payload; the full body
  lives behind `content_digest`.
- To carry credentials, cookies, or custom authorization headers. Use the
  appropriate authenticated provider capability for those reads.

## Procedure

1. Require `url` at runner preflight. A missing target fails before the graph
   starts with the exact missing input.
2. Use `allowlist: ["*"]` when the caller omits it, meaning any publicly
   routable host. A caller-supplied allowlist narrows that default. Match the
   URL host against the effective allowlist. On a miss, return `policy_denied`
   before any network call, recording the attempted host and the allowlist it
   was checked against.
3. Fetch, following redirects, re-checking each redirect target's host against
   the same `allowlist`. A redirect that lands off-allowlist halts the fetch,
   fails with `policy_denied` naming the hop that failed. The page is read
   whole: a page larger than the operator's ceiling (`RUNX_WEB_READ_MAX_BYTES`,
   64 MiB by default) fails with `response_too_large` instead of being cut.
4. Compute `content_digest` over the retrieved body.
5. Extract per `extract`: `text` (readable body text, default), `metadata`
   (title, description, canonical, declared language, content type), `links`
   (absolute hrefs found in the document), or `json` (the body as one JSON
   document).
6. Return the `provider_operation` packet whose result holds the final URL,
   status, digest, extracted slice, and provenance. A refused URL, a transport
   failure, or a non-2xx answer fails the step; no result stands for a page
   that was not read.

## Edge cases and stop conditions

- **Missing `url`:** fail runner preflight; do not start or seal an empty fetch.
- **Omitted `allowlist`:** use `["*"]`; private-network protection still applies.
- **Host off the allowlist:** stop with `policy_denied` before any network call;
  record the attempted host, not a response body (there is none).
- **Redirect off the allowlist:** halt the fetch, return `policy_denied` naming
  the hop that failed, and discard the partial body.
- **Page past the ceiling:** fail with `response_too_large`; the operator
  raises `RUNX_WEB_READ_MAX_BYTES` to read it. A partial page is never
  returned.
- **Large raw body:** never inline beyond the extracted slice; anything bigger
  than the requested view is reachable only through `content_digest`.
- **Authenticated source:** stop and use the relevant provider read skill; this
  public fetch does not accept credentials or custom headers.

## Output schema

```yaml
provider_operation:
  data:
    provider: web
    operation: page.read
    result:
      final_url: string            # URL after redirects, the one the digest is over
      status: number               # HTTP status of the final response
      content_digest: string       # digest of the body, algorithm prefix included
      extract_mode: text | metadata | links | json
      extracted: string | object | array   # text, metadata object, hrefs, or the JSON document
      provenance:
        fetched_at: string         # timestamp of the fetch
        redirects: array           # ordered hops, each re-checked against the allowlist
        bytes: number              # bytes read
```

The sealed `runx.receipt.v1` carries the final URL, status, `content_digest`,
byte count, and the redirect chain. A denied host fails the step with
`policy_denied` before any request leaves. The receipt carries no header
values, no cookies, and no raw body beyond the digest.

## Worked example

Input: `url` of the HTTP Semantics RFC, an `allowlist` of `www.rfc-editor.org`
and `rfc-editor.org`, and `extract: text`.

Output: the host matched the allowlist before the request left; no redirects;
status `200`; `content_digest` is taken over the whole body; `extracted` holds
the readable text slice; provenance records `fetched_at`, an empty redirect
chain, and `184302` bytes.
The receipt seals with the final URL, status, digest, and byte count; no header
value reaches it.

## Inputs

- `url` (required): the single public HTTP(S) URL to fetch.
- `allowlist` (optional): permitted hosts or host patterns for the URL and every
  redirect target. Omit it for `["*"]`, meaning any publicly routable host.
- `extract` (optional): `text`, `metadata`, `links`, or `json`. Defaults to
  `text`.
