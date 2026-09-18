---
name: postmortem-maker
description: Read a real incident record at run time — a live web-fetch of an incident thread or an incident/ticket read_projection — reconstruct a source-cited postmortem that refuses invented facts, and execute a sealed outbox delivery only when the evidence settles the cause. Conflicting or insufficient evidence yields unknowns and publishes nothing.
runx.category: ops
---

# Postmortem Maker

Turn a resolved incident into a postmortem that cites its own evidence, and
deliver it through a sealed outbox transport only when the source actually
settles the cause. The read, reason, publish loop completes in one sealed run,
so the receipt shows what was read, what was concluded, and what was or was
not delivered.

## Procedure

1. **Read the incident record from a real source at run time.** The
   `read-source` step resolves `incident_source`: `{kind: "web_fetch", url}`
   fetches the incident thread over HTTPS via `web.fetch`;
   `{kind: "read_projection", ...}` reads an incident or ticket event-stream
   projection via `data.read_projection`; and `{kind: "inline", fragments}`
   replays supplied fragments, which is the deterministic path the harness
   uses. Every fragment keeps a stable `id`, `source`, and `text`.
2. **Digest** binds the exact fragment set with `data.digest`.
3. The **drafting** agent separates facts from hypotheses: a summary, an
   `impact` statement and scope, an evidence-cited timeline, a root cause with
   status `known`, `suspected`, or `unknown`, open `unknowns`, and owned
   `action_items`.
4. **Deterministic enforcement** checks every timeline entry and any
   non-unknown root cause: the cited fragment must exist and the quote must
   appear verbatim in that fragment's text. An invented citation refuses the
   whole run. Action items must carry an action and an owner.
5. The verdict separates completeness from honesty. A fully cited postmortem
   with a supported root cause and no open unknowns is `publishable`; a
   well-formed but incomplete one is `needs_more_evidence`; any fabricated or
   uncited claim is `refused`.
6. **Delivery is executed, not proposed.** When the postmortem is publishable
   and `postmortem_policy.allow_publish` is true, `send_plan.status` becomes
   `ready` and the graph runs `prepare-delivery`, `deliver`, and
   `record-delivery`: the sealed outbox transport (`fs.write`) writes the
   postmortem bound to its fragments digest, and `publish_result` records the
   executed send plan. When evidence is conflicting or incomplete, the send
   plan is `withheld`, those steps never run, and nothing is published.

## Inputs

- `incident_ref` — identifier of the resolved incident.
- `incident_source` — where the record is read at run time:
  `{kind: "web_fetch", url, allowlist?}` fetches a real incident thread,
  `{kind: "read_projection", data_source_ref, aggregate_id, resource}` reads a
  ticket or incident projection, or `{kind: "inline", fragments}` replays
  supplied fragments.
- `postmortem_policy` — `{allow_publish, outbox_dir}` governing whether a
  publishable postmortem may execute its sealed delivery and where.

## Output contract

The terminal postmortem is `runx.postmortem.v2`: `decision`
(`publishable`, `needs_more_evidence`, `refused`), `status`, `summary`,
`impact{statement, scope}`, a `timeline` whose every entry carries
`fragment_id` plus a verbatim `quote`, `root_cause{status, statement,
fragment_id}`, `unknowns[]`, `action_items[{action, owner}]`, a `send_plan`
that is `ready` or `withheld`, the `fragments_digest` of the exact source
read, and deterministic `validation` findings. On the delivery branch the run
also emits `publish_result` (`runx.publish_result.v1`) recording the executed
`sealed-outbox` write bound to the postmortem.

## Stop and recovery

Stop when the source cannot be read, the fragment set is empty, a citation
cannot be verified verbatim, or the evidence leaves the cause unresolved.
Never weaken a refusal into a plausible writeup: conflicting evidence stays in
`unknowns` and withholds the send plan rather than publishing a guess. Recover
by supplying a readable source, reconciling the conflicting records, or
collecting the missing evidence and rerunning the read.

## Agent task contracts

### `postmortem-maker-read`

Return exactly one `source_read` object: `read_kind` (the kind actually used),
`fetched_ref` (the URL, projection ref, or `inline:<incident_ref>`), and
`fragments[]`. Each fragment has `id`, `source`, and `text` copied from the
real record. For `web_fetch`, fetch the URL live with `web.fetch` and split the
record into faithful fragments; do not paraphrase, augment, or recall content
that is not in the fetched body. For `read_projection`, read the projection
with `data.read_projection` and project its events the same way. For `inline`,
echo the supplied fragments unchanged. Return an empty `fragments` list rather
than inventing content when the source cannot be read.

### `postmortem-maker-draft`

Return exactly one `postmortem_draft` object: `summary`, `impact{statement,
scope}`, `timeline[]`, `root_cause{status, statement, fragment_id, quote}`,
`unknowns[]`, `action_items[]`. Every timeline entry and every non-unknown
root cause must name a `fragment_id` from the read and a `quote` copied
verbatim from that fragment's `text`. When evidence conflicts or is missing,
set `root_cause.status` to `unknown`, put the unresolved questions in
`unknowns`, and still propose owned `action_items` for gathering what is
missing. Do not assert any fact the fragments do not contain.
