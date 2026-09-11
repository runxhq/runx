---
name: postmortem-maker
description: Read an incident record from its live source, turn it into a postmortem whose every timeline entry and root cause is quoted from that source, refuse invented citations outright, and execute the comms send only when nothing is left unknown.
---

# Postmortem Maker

Produce a postmortem that never pretends unknowns are facts, from evidence the
skill reads itself. The incident record is fetched at run time from a public
source URL, fragments are derived from that fetched text, and deterministic code
checks every claim against those fragments. A postmortem that is complete and
authorised is published in the same run, so the receipt records a send that
happened rather than a proposal that might.

## Why this exists

A postmortem is worth reading only when a reader can trace each statement back
to something that was recorded during the incident. Two failures make one
worthless: citing evidence that does not exist, and asserting a cause that the
evidence does not support. This skill makes the first impossible and the second
visible.

## Procedure

1. `web.fetch` reads the incident record from `source_url`. That fetch is the
   only evidence the run gets; nothing is pasted in by the caller.
2. Native `data.digest` binds the fetched bytes, so the evidence set is
   reproducible from the receipt.
3. `buildFragments` splits the fetched text into stable, verbatim fragments.
   Lines carrying incident signal (timestamps, errors, deploys, rollbacks,
   alerts, thresholds, recovery) become citable fragments; conversational noise
   is dropped. Fragment text is never paraphrased.
4. The drafting agent reads only those fragments and assembles a summary, an
   evidence-cited timeline, a root cause with status `known`, `suspected`, or
   `unknown`, open unknowns, and owned action items.
5. `finalizePostmortem` enforces the citations deterministically. Every timeline
   entry and every non-unknown root cause must cite a fragment that exists and
   quote text that appears verbatim in it. One invented citation refuses the
   whole run. Action items must carry an action and an owner.
6. The verdict separates completeness from honesty, then acts on it:
   - `publishable` — fully cited, supported root cause, no open unknowns. If
     `postmortem_policy.publish` is true and a channel is set, the postmortem is
     rendered and sent in the same run, and `publish_result.executed` is true.
   - `needs_more_evidence` — grounded but incomplete. Unknowns remain and
     nothing publishes.
   - `refused` — the draft claimed something the source does not support. The
     timeline is emptied and nothing publishes.

Publication is withheld whenever the policy withholds it, so an operator can run
the same incident in draft mode and in publish mode and compare the receipts.

## Inputs

- `source_url` (required) — public URL of the incident thread, ticket export, or
  status page holding the record. Fetched at run time.
- `postmortem_policy` (required) — `publish` (boolean), `channel`, `audience`,
  and optional `transport_ref`.
- `incident_ref` (optional) — identifier for the incident; defaults to the
  source URL.

## Output

`postmortem` (`runx.postmortem.v1`) carries `decision`, `summary`, the cited
`timeline`, `root_cause`, `unknowns`, `action_items`, the `source` block with
the URL, digest, fragment count and read time, `publish_result` with the
executed send or null, `publish_performed`, the fragments digest, and
`validation` with any findings.

## Agent task contracts

### `postmortem-maker-draft`

Read `fragment_set` and `incident_ref` from step inputs. Return
`postmortem_draft` with `summary`, `timeline` entries (each `entry`,
`fragment_id`, `quote`), `root_cause` (`status`, and for known or suspected a
`statement`, `fragment_id`, and `quote`), `unknowns`, and `action_items` (each
`action`, `owner`). Quote fragments verbatim, mark anything the fragments do not
support as unknown rather than asserting it, and never invent fragments, quotes,
owners, or deadlines. Citing a fragment that does not exist, or quoting text
that does not appear in the fragment you cite, refuses the entire run.
