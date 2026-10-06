---
name: postmortem-maker
description: Read a real incident record at run time, turn it into a traceable postmortem that separates fragment-cited facts from hypotheses, and — only when it is publishable — publish it in the same run by composing send-as so the receipt carries an executed send_plan, not an inert proposal.
runx:
  category: ops
---

# Postmortem Maker

Produce a postmortem that never pretends unknowns are facts, and — when the
postmortem is publishable — publish it in the same governed run.

The skill reads the incident record from a **real source at run time**: the
`incident_source` handle is a public incident/ticket thread URL, fetched through
`web-fetch` before anything is drafted. The fetched record is split into
digest-bound fragments, the drafting agent separates what happened from what is
suspected, and deterministic code verifies every claimed fact against those
fragments. An invented citation refuses the whole run.

## Procedure

1. `web-fetch` reads `incident_source` live: final URL, status, content digest,
   extracted text, provenance. A non-2xx or unreadable source fails the run
   before any drafting happens.
2. Native `data.digest` binds the exact fragment set derived from that read.
3. The drafting agent assembles a summary, an evidence-cited timeline, the
   impact, a root cause with a status of `known`, `suspected`, or `unknown`,
   open unknowns, and owned action items.
4. Deterministic enforcement checks every timeline entry and any non-unknown
   root cause: the cited fragment must exist and the quote must appear verbatim
   in that fragment's text. An invented citation refuses the whole run. Action
   items must carry an action and an owner, and a publishable postmortem must
   carry an impact statement.
5. The verdict separates completeness from honesty:
   - fully cited + supported root cause + no open unknowns + impact present ->
     `publishable`, and the graph proceeds to `publish`;
   - grounded but incomplete -> `needs_more_evidence`, nothing publishes;
   - claims the fragments do not support -> `refused`, nothing publishes.
6. `publish` composes the shipped `send-as` skill under the operator's
   `postmortem_policy`: plan once, apply the digest-bound send once, and close
   only on independent provider readback. The sealed receipt then records an
   executed `send_plan` and `send_result` bound to the postmortem digest;
   held postmortems carry `publish_result: null` and `publish_performed: false`.

Publication is a gated effect of this skill, not a suggestion: it runs only
after deterministic validation passes, only when the policy names a compatible
connector, and only through the human approval gate `send-as` enforces at the
live-delivery boundary.

## Inputs

- `incident_source` (required): the source handle — a public incident or
  ticket thread URL that is fetched at run time.
- `postmortem_policy` (required): publish policy carrying `principal`,
  `audience`, `connector` (`provider` + `target`), `consent_basis`, and the
  operator `gate_note` that frames the approval decision.

## Output

`postmortem` (`runx.postmortem.v1`) carries `decision`
(`publishable`, `needs_more_evidence`, `refused`), `summary`, the cited
`timeline`, `impact`, `root_cause`, `unknowns`, `action_items`,
`publish_proposal`, `publish_performed`, the sealed `publish_result` (or
`null`), `validation`, and the fragments digest.

## Agent task contracts

### `postmortem-maker-draft`

Read `incident_ref` and `incident_fragments` from step inputs. Return
`postmortem_draft` with `summary`, `impact`, `timeline` entries (each `entry`,
`fragment_id`, `quote`), `root_cause` (`status`, and for known or suspected a
`statement`, `fragment_id`, and `quote`), `unknowns`, and `action_items` (each
`action`, `owner`). Quote fragments verbatim, mark anything the fragments do
not support as unknown rather than asserting it, and never invent fragments,
quotes, owners, or deadlines.
