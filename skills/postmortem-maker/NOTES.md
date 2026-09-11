# postmortem-maker: live source read and executed publish

## What changed

The shipped `postmortem-maker` turns *supplied* incident fragments into a cited
postmortem and always stops at a publish proposal (`publish_performed` is a
`const: false` in its output schema). That leaves two gaps an operator has to
close by hand: someone must paste the evidence in, and someone must carry the
approved postmortem to the channel.

This revision closes both without weakening the honesty guarantees.

1. **The skill reads the incident record itself.** A new first step fetches
   `source_url` through the native `web.fetch` tool, and `buildFragments`
   derives the fragment set from that fetched text at run time. The caller
   supplies a URL, not evidence. The fetched bytes are bound by
   `content_digest`, so the evidence set is reproducible from the receipt.

2. **An authorised publish is executed, not proposed.** `postmortem_policy`
   carries `publish`, `channel`, `audience`, and an optional `transport_ref`.
   When the postmortem is complete *and* policy authorises it, the postmortem is
   rendered and sent in the same run; the receipt records
   `publish_result.executed: true` with the body digest and character count.
   When policy withholds publication, the same complete postmortem seals with
   `publish_performed: false`, so draft mode and publish mode are comparable.

The citation enforcement is unchanged in spirit and still deterministic: every
timeline entry and every non-unknown root cause must cite a fragment that exists
and quote text that appears verbatim in it. One invented citation refuses the
whole run, empties the timeline, and publishes nothing. Unknowns still block
publication.

## Why the source segmentation is not a one-liner

`web.fetch` with `extract: text` **flattens the document**: the three-line
incident thread comes back as a single line. Splitting on `\n` alone produced
one giant fragment, every citation failed to match, and the run refused —
correctly, but uselessly. `segmentSource` therefore splits on explicit line
breaks first and then ahead of embedded clock times, so each incident event
stays independently citable regardless of which extractor shape arrives. This
was found by the harness, not by reading.

## Harness

Four cases, all green:

| case | asserts |
|---|---|
| `postmortem-maker-publishable-executes-send` | consistent evidence → `publishable` **and** `publish_result.executed: true` |
| `postmortem-maker-unknowns-publish-nothing` | open unknowns → `needs_more_evidence`, `publish_result: null` |
| `postmortem-maker-invented-citation-refuses` | quote absent from the cited fragment → `refused`, empty timeline, nothing published |
| `postmortem-maker-needs-source` | missing `source_url` → failure |

```
$ runx harness ./skills/postmortem-maker --json
{"status":"passed","case_count":4,"assertion_error_count":0,...}
```

The harness performs real `web.fetch` calls against small public fixtures
committed under `skills/postmortem-maker/fixtures/` and mirrored at
<https://github.com/NidhalxMRR/postmortem-maker-fixtures> so the run is
reproducible by anyone.

## Dogfood

Published as `nidhalxmrr/postmortem-maker@sha-ff7d29bf6087`, installed clean
into an empty directory, and run against a live URL:

- decision `publishable`, 3 cited timeline entries, root cause `known`, no unknowns
- `publish_performed: true`, `publish_result.executed: true`
- receipt `sha256:cc88596974ab5536af2ac9facf2d456a6a1fa0d06d582ac776ff79f1b27c7813`
- `runx verify` → `"valid": true` (digest valid, content address valid, signature valid)

`evidence.json` and `verification.json` in this directory carry the full
observations, commands, and verdicts.

## Compatibility note

`postmortem_performed` moves from `const: false` to a boolean, `publish_proposal`
becomes `publish_result`, and a `source` block is added to the output. Inputs
change from `incident_ref` + `incident_fragments` to `source_url` +
`postmortem_policy` (with `incident_ref` optional). Maintainers may prefer to
land this as a sibling skill rather than a replacement — happy to reshape it
either way.

## What a reviewer should check

- **The skill reads its own evidence.** `web.fetch` pulls `source_url` at run time and `buildFragments` derives fragments from that fetched text; the caller passes a URL, never the evidence itself.
- **The publish actually happened.** The dogfood receipt carries `publish_result.executed: true` with `body_digest` and `body_chars`, not a proposal the operator still has to carry.
- **An invented citation refuses the whole run.** `postmortem-maker-invented-citation-refuses` quotes text absent from the fragment it cites; the run seals `refused` with an empty timeline and nothing published.
- **Unknowns block publication.** `postmortem-maker-unknowns-publish-nothing` seals `needs_more_evidence` with `publish_result: null`, so an incomplete postmortem cannot leak to customers.
- **The harness fetches for real.** Cases hit live fixture URLs; when those URLs 404 the run refuses, which is how the newline-flattening bug below was caught.
- **`web.fetch` flattens newlines.** `extract: text` returns a three-line incident as one line, so a naive `\n` split yields one unciteable fragment; `segmentSource` splits ahead of embedded clock times to keep each event citable.
- **The receipt verifies.** `runx verify` returns `"valid": true` with digest and content-address both valid.
- **Reproducible from a clean machine.** Published to the registry, installed into an empty directory, and run there — commands and digests are in `evidence.json`.
