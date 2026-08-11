---
name: list-hygiene-judge
description: Judge whether a contact should be verified, suppressed, or moved to re-permission from bounded engagement evidence and a durable data-store projection; record exactly one consent-state transition with optimistic concurrency, or stop for human review without writing. Use for list hygiene decisions before a separate send-as run reads the recorded consent state.
registry_owner: emilianochagoya
---

# List Hygiene Judge

Turn contact-level engagement and bounce evidence into one durable consent-state
decision. The skill reads the contact's current projection first, compares that
projection with the caller's bounded evidence, then either records one transition
or stops without writing. It never sends a message and never creates authority.

Use it when a contact stream already exists in the canonical `data-store` and an
operator needs a reproducible decision before future campaign delivery. Do not use
it to import a list, infer engagement metrics, recover an unsubscribe, or send a
re-permission message.

## Operating model

1. Read the contact projection from `data-store` using `data_source_ref`,
   `resource`, and `aggregate_id`.
2. Bind the caller's `expected_version` and `current_consent_state.evidence_version`
   to the version actually read. A missing, stale, or ambiguous read stops.
3. Require durable hard-bounce evidence before suppression: when
   `engagement_history.hard_bounces` is positive, the projection's latest event
   must be `contact.hard_bounce_observed`.
4. Refuse to re-permission any contact carrying an active unsubscribe marker.
   Ambiguous recovery also stops for a human list-hygiene reviewer.
5. Decide deterministically:
   - a proven hard bounce becomes `suppress`;
   - engagement older than `bounce_policy.decay_threshold_days` becomes
     `re_permission`;
   - otherwise the contact remains in `verify`.
6. Append exactly one `contact.consent_state_recorded` event with the supplied
   `idempotency_key` and compare-and-set `expected_version`, then read the
   projection back.
7. Return the decision and recorded transition only after the readback proves the
   new version. A retry with the same key is handled by `data-store` rather than
   producing a duplicate transition.

## Evidence and finality

The result distinguishes three things:

- `source_read` is the contact projection observed before judgment;
- `decision` is the bounded consent-state verdict;
- `recorded_transition` is the post-append projection readback, or `null` when
  the skill stopped before writing.

`write_performed: true` means the consent transition was committed and read back.
`send_performed` is always `false`. The result is not permission to contact the
recipient.

## Relationship to send-as

Future delivery is a separate governed run selected by the operator. `send-as`
must read the recorded consent state at send time and gate delivery from that
durable state. It does not trust or consume this skill's output as a substitute
for the store read. A suppressed or unresolved contact therefore cannot receive
a campaign merely because an old result packet is supplied.

## Stop conditions and recovery

Stop without appending when any of these conditions holds:

- the projection is missing or unreadable;
- `expected_version` or `evidence_version` differs from the durable version;
- engagement evidence is marked missing, stale, or ambiguous;
- a hard-bounce count lacks a matching durable hard-bounce observation;
- an unsubscribe marker is active;
- bounce recovery is ambiguous.

The result names `human:list-hygiene-reviewer` and preserves the source read,
reason, and input identity required to resolve the ambiguity. After an operator
records corrected evidence in the contact stream, rerun with the new projection
version and a new idempotency key. Never weaken the version guard or clear an
unsubscribe marker inside this skill.

## Reproducible harness

The default `judge` runner never invents or seeds source data. The non-default
`harness_judge` runner exists only to make the three public harness cases
reproducible in an isolated workspace: it appends one declared fixture event to
a fresh SQLite-backed `data-store`, then runs the same read, decide, optional
append, readback, and finalization path. Fixture documents are UTF-8 JSON so the
hosted registry can reconstruct and rerun the package without binary sidecars.

## Example

A contact has durable projection version 3, no unsubscribe marker, no hard
bounces, and a last engagement 121 days ago. With a 90-day decay threshold,
the skill records `re_permission` at version 4. A later `send-as` run still reads
version 4 and owns its own delivery gate; this skill sends nothing.

## Agent task contract

This package uses deterministic JavaScript rather than model judgment.

- Treat `contact_readback` as the authority for the durable version and latest
  evidence type.
- Never invent engagement counts, consent state, bounce evidence, or recovery.
- Never append on stale evidence, a version mismatch, or an unsubscribe marker.
- Never emit an operational proposal, provider payload, or claimed send result.
- Preserve `aggregate_id`, `idempotency_key`, the source projection digest, and
  the recorded version in the final result.
