---
name: postmortem-draft
description: Turn resolved-incident fragments into a traceable postmortem that separates fragment-cited facts from hypotheses, blocks publication while unknowns remain, and keeps the comms send behind a human gate.
---

# Postmortem Draft

Produce a postmortem that never pretends unknowns are facts. The supplied
incident fragments are the only evidence; the drafting agent separates what
happened from what is suspected, and deterministic code verifies every claimed
fact against the fragments. Publication is a gated proposal, never an effect
of this skill.

## Procedure

1. Native `data.digest` binds the exact fragment set.
2. The drafting agent assembles a summary, an evidence-cited timeline, a root
   cause with a status of `known`, `suspected`, or `unknown`, open unknowns,
   and owned action items.
3. Deterministic enforcement checks every timeline entry and any non-unknown
   root cause: the cited fragment must exist and the quote must appear
   verbatim in that fragment's text. An invented citation refuses the whole
   run. Action items must carry an action and an owner.
4. The verdict separates completeness from honesty: a fully cited postmortem
   with a supported root cause and no open unknowns is `publishable` and
   carries a publish proposal gated on a human approver through `message-send`; a
   grounded but incomplete one seals `needs_more_evidence` and publishes
   nothing.

To build the fragment set from live systems, compose `web-fetch` or store
reads upstream; the digest binds whatever evidence was supplied.
Running the incident belongs to its operator; this skill explains it afterward.

## Output

`postmortem` (`runx.postmortem.v1`) carries `decision` (`publishable`,
`needs_more_evidence`, `refused`), `summary`, the cited `timeline`,
`root_cause`, `unknowns`, `action_items`, the gated `publish_proposal` or
null, `validation`, and the fragments digest. `publish_performed` is always
false.

Inputs are `incident_ref` and `incident_fragments`.

## Agent task contracts

### `postmortem-draft-draft`

Read `incident_ref` and `incident_fragments` from step inputs. Return
`postmortem_draft` with `summary`, `timeline` entries (each `entry`,
`fragment_id`, `quote`), `root_cause` (`status`, and for known or suspected a
`statement`, `fragment_id`, and `quote`), `unknowns`, and `action_items`
(each `action`, `owner`). Quote fragments verbatim, mark anything the
fragments do not support as unknown rather than asserting it, and never
invent fragments, quotes, owners, or deadlines.
