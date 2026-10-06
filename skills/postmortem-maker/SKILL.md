---
name: postmortem-maker
description: Read a real incident record at run time — a live web-fetch of an incident thread or an incident/ticket read_projection — reconstruct a source-cited postmortem that refuses invented facts, and execute a sealed outbox delivery only when the evidence settles the cause. Conflicting or insufficient evidence yields unknowns and publishes nothing.
runx.category: ops
---

# Postmortem Maker

Turn a resolved incident into a postmortem that cites its own evidence, and
deliver it through a sealed outbox transport only when the source actually
settles the cause. The read -> reason -> publish loop completes in one sealed
run, so the receipt shows what was read, what was concluded, and what was or
was not delivered.

## Procedure

1. **Read the incident record from a real source at run time.** The
   `read-source` step resolves `incident_source`: `{kind: "web_fetch", url}`
   fetches the incident thread over HTTPS via `web.fetch`,
   `{kind: "read_projection", ...}` reads an incident/ticket event-stream
   projection via `data.read_projection`, and `{kind: "inline", fragments}`
   replays supplied fragments — the deterministic path the harness uses.
   Every fragment keeps a stable `id`, `source`, and `text`.
2. **Digest** binds the exact fragment set via `data.digest`.
3. The **drafting** agent separates facts from hypotheses: a summary, an
   `impact` statement and scope, an evidence-cited timeline, a root cause
   with status `known`, `suspected`, or `unknown`, open `unknowns`, and owned
   `action_items`.
4. **Deterministic enforcement** checks every timeline entry and any
   non-unknown root cause: the cited fragment must exist and the quote must
   appear verbatim in that fragment's text. An invented citation refuses the
   whole run. Action items must carry an action and an owner.
5. The verdict separates completeness from honesty. A fully cited postmortem
   with a supported root cause and no open unknowns is `publishable`; when
   `postmortem_policy.allow_publish` is true it carries a `ready` send plan
   for the bundled **sealed-outbox transport**, which writes the sealed
   postmortem packet to the policy's `outbox_dir` under a deterministic
   compare-and-set name — an executed delivery recorded in the receipt, not
   an inert proposal. Grounded-but-incomplete seals `needs_more_evidence`;
   unsupported claims seal `refused`; both withhold the send plan and the
   delivery steps never run, so the run proves the absence.

`incident-commander` owns running the incident; this skill owns explaining it
afterward.

## Output

`postmortem` (`runx.postmortem.v2`) carries `decision` (`publishable`,
`needs_more_evidence`, `refused`), `status`, `incident_ref`, `summary`,
`impact`, the cited `timeline`, `root_cause`, `unknowns`, `action_items`,
the `send_plan` (`ready` or `withheld`), `validation`, and the fragments
digest. When the sealed-outbox transport executes, `publish_result`
(`runx.publish_result.v1`) records `executed: true`, the delivery path, and
the digest the send was bound to.

Inputs are `incident_ref`, `incident_source`, and `postmortem_policy`.

## Agent task contracts

### `postmortem-maker-read`

Read `incident_ref` and `incident_source` from step inputs. For
`kind: "web_fetch"` call `web.fetch` on the incident thread URL and extract
the record text; for `kind: "read_projection"` call `data.read_projection`
with the supplied stream coordinates; for `kind: "inline"` return the
supplied fragments unchanged. Return `source_read` with `read_kind`,
`fetched_ref` (the URL, stream id, or `inline:<incident_ref>`), and
`fragments` — each fragment carrying a stable `id`, its `source`, and the
verbatim `text`. Never paraphrase fragment text; never invent fragments.

### `postmortem-maker-draft`

Read `incident_ref` and `postmortem_policy` from step inputs and
`incident_fragments` plus `source_digest` from context. Return
`postmortem_draft` with `summary`, `impact` (`statement`, `scope`),
`timeline` entries (each `entry`, `fragment_id`, `quote`), `root_cause`
(`status`, and for known or suspected a `statement`, `fragment_id`, and
`quote`), `unknowns`, and `action_items` (each `action`, `owner`). Quote
fragments verbatim, mark anything the fragments do not support as unknown
rather than asserting it, and never invent fragments, quotes, owners, or
deadlines.
