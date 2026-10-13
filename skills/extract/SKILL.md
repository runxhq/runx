---
name: extract
description: Extract schema-validated JSON from messy HTML or text fixtures with digest-bound provenance.
---

# Extract

## What this skill does

Extract structured JSON from a bounded HTML or plain-text source inside the
skill package, validate the result against a declared JSON Schema, and emit a
digest-bound provenance packet. The runner is deterministic: it reads local
fixture bytes, extracts headings, useful paragraphs, and HTTP/API terms, checks
the packet against `packets/extraction.schema.json`, and returns
`runx.structured_extraction.result.v1`.

This is not a scraper or crawler. Network fetch belongs in a separate governed
web-fetch step. This skill starts after the bytes are already available as an
approved package fixture, so the runx receipt can bind the input digest, schema
digest, and validated output digest without depending on a live website.

## When to use this skill

Use this skill when an agent needs a reproducible extraction packet from messy
reference material, docs snapshots, benchmark fixtures, or captured public web
content. It is appropriate when the source bytes have already been fetched,
approved for local use, and pinned inside the skill package.

Use it as the extraction stage in a larger chain: fetch or curate bytes first,
run structured extraction second, then pass the validated packet to a downstream
agent, reviewer, search index, or evidence bundle.

## When not to use this skill

Do not use this skill to fetch URLs, bypass network policy, process private
customer data, parse credentials, or summarize a source without a schema. Do
not use it when the desired output is free-form prose; this runner exists to
produce typed, schema-checked JSON.

If the source bytes are not already present, stop with `needs_input` and use a
web-fetch or repository-read skill with its own authority grant and receipt. If
the schema is missing or unknown, stop with `needs_more_evidence` instead of
inventing a packet shape.

## Procedure

1. Resolve `input_path` and `schema_path` relative to the skill package.
2. Reject any path that escapes the package root.
3. Read the source bytes and JSON Schema bytes.
4. Record SHA-256 digests for both inputs.
5. Extract a bounded set of headings, paragraphs, and HTTP/API terms.
6. Build `runx.structured_extraction.result.v1`.
7. Validate the packet against the schema and deterministic internal checks.
8. Return the packet only when validation passes; otherwise fail closed.

## Edge cases and stop conditions

Return `needs_input` when `source_url`, `input_path`, or `schema_path` is
missing. Return `needs_more_evidence` when the source URL does not identify the
origin of the fixture bytes. Return `refused` if the caller asks the skill to
fetch live network content, read outside the package root, process secrets, or
extract private user data.

Fail the run if the input file or schema file cannot be read, the schema is not
valid JSON, the content type is unsupported, the extracted packet has too few
items to be useful, or JSON Schema validation fails. Do not emit a partial
success packet.

The authority scope is local fixture read plus local schema read. The proof
surface is the sealed receipt containing input digest, schema digest, output
digest, validation checks, and source URL.

## Output schema

The runner emits `structured_extraction_result` with packet schema
`runx.structured_extraction.result.v1`:

```json
{
  "schema": "runx.structured_extraction.result.v1",
  "source": {
    "url": "https://example.test/source.html",
    "content_type": "text/html",
    "input_path": "fixtures/source.html",
    "input_sha256": "sha256:<hex>",
    "input_bytes": 0
  },
  "extraction": {
    "title": "Document title",
    "summary": {
      "item_count": 0,
      "heading_count": 0,
      "term_count": 0,
      "paragraph_count": 0,
      "text_chars": 0
    },
    "items": []
  },
  "validation": {
    "schema_id": "runx.structured_extraction.result.v1",
    "schema_sha256": "sha256:<hex>",
    "valid": true,
    "engine": "native-json-schema-subset-v1",
    "checks": []
  },
  "provenance": {
    "mode": "fixture",
    "tool_version": "0.1.0",
    "source_kind": "real_public_document",
    "output_payload_sha256": "sha256:<hex>"
  }
}
```

The packet also includes artifact and signal records that let a downstream
receipt reference the input fixture, schema, and validated output.

## Worked example

Extract a compact evidence packet from the packaged RFC 9110 fixture:

```bash
runx skill "$PWD" extract \
  -i input_path=fixtures/rfc9110-http-semantics.html \
  -i schema_path=packets/extraction.schema.json \
  -i source_url=https://www.rfc-editor.org/rfc/rfc9110.html \
  -i content_type=text/html \
  -i max_items=18 \
  --json
```

Expected behavior:

- The run stays local and does not fetch the RFC URL.
- `source.input_sha256` identifies the exact fixture bytes.
- `validation.valid` is true only if the packet satisfies the schema and
  internal extraction checks.
- The sealed receipt links the source digest, schema digest, and output digest.

## Inputs

- `input_path`: package-relative path to an HTML or text fixture.
- `schema_path`: package-relative JSON Schema path.
- `source_url`: canonical public source URL for the fixture bytes.
- `content_type`: `text/html` or `text/plain`; defaults to `text/html`.
- `max_items`: maximum extracted items to include; clamped by the runner.

## Outputs

- `structured_extraction_result`: validated packet with source, extraction,
  validation, provenance, artifacts, and signal metadata.
