# Operational Intelligence

Operational intelligence is the generic runx boundary for turning source
signals into reviewable proposals. Core records what is proposed, why, what
evidence supports it, where ownership routes, and which human gate is required.
Products decide whether any proposal becomes a provider mutation, customer
message, tracking item, change request, publication, or final decision.

The composition spine is:

`source/context/signal/decision/proposal/action/outcome`

Every operational flow starts from an originating source thread, hydrates only
the safe context needed for reasoning, records signal and decision state, then
either emits a reviewable proposal or enters an already-governed action lane.
The final outcome is posted back through the source-thread/outbox path when
policy and provider readback supply the required references.

UI verbs map onto existing runx actions instead of creating fixed domain lanes:

- `check` is read-only triage and does not grant mutation permission.
- `reply` prepares draft context or a `reply-only` proposal; it does not send.
- `create issue` maps to the tracking-item lane through `issue-intake`.
- `build fix without prior check` maps directly to `issue-to-PR` when source
  context, policy, and authority are sufficient.
- `manual-review` stops with a human review packet.
- `escalate` emits `proposal_kind: escalation` with an owner route, evidence,
  severity, urgency, and the exact human decision needed. The provider-neutral
  `runx.escalation` extension carries severity and urgency when the base
  proposal envelope is used.
- Final outcome publication always returns to the originating source thread
  when policy requires source-thread continuity.

## Operational Proposal Contract

`runx.operational_proposal.v1` is the only public proposal packet promoted by
the operational contracts v1 work. Runx core must not publish domain-specific
packet families such as separate support, escalation, outreach, or product
signal proposal schemas. Those distinctions belong in `proposal_kind` metadata.

The proposal packet is a replayable, redacted decision envelope. It should carry:

- stable proposal id, source event id, idempotency key, dedupe fingerprint, and
  packet schema/version
- `proposal_kind`: a product-namespaced classifier, not a new public schema
- `source_ref` and optional `source_thread_ref` references; thread publication
  is required by policy/outbox, not by provider-specific proposal fields
- context and artifact references, including hydrated context refs and
  redaction status when provider context was used
- decision summary, rationale, confidence, risks, caveats, missing context, and
  recommended action-lane intents
- `evidence_refs` for the facts that justify the recommendation
- `owner_route_id` for product-owned routing without exposing owner maps,
  channels, projects, or customer identifiers
- `human_gates` describing the exact human decisions required before send,
  provider mutation, change-request creation, customer communication, or final
  change approval
- generic `result_refs` and `publication_refs` for tracking items, change
  requests, source publications, outcome observations, or provider links
- `recommended_actions`, which name governed action lanes; tool ids and tool
  input schemas stay behind those lanes and adapters
- public summary text safe for provider threads, source-thread comments, support
  surfaces, and admin readbacks

## Reference Contracts

All source, result, publication, evidence, artifact, receipt, story, and
outcome pointers use central reusable reference contracts.

- `runx.reference.v1` is the only object used for a concrete pointer.
- `runx.reference_link.v1` wraps a `Reference` with a role when the surrounding
  packet needs to say why a reference is present.
- `runx.operational_proposal.v1` uses those shapes for proposal refs and
  proposal ref links.
- External reference types never name a provider: `thread`, `event`,
  `comment`, `tracking_item`, `change_request`, `repository`, and
  `external_url`. A connector declares which of its
  locators name which type (`references` in `connector.yaml`), and the
  runtime classifies locators from those declarations.
- Provider names belong in `ref.provider`, provider locators belong in
  `ref.locator`, and provider URLs or URIs belong in `ref.uri`.
- Proposal, story, and outbox packets must not add top-level provider-specific
  fields such as issue URL, pull-request URL, channel id, or comment URL.

Existing action lane values remain the admission vocabulary for proposal
preparation. A proposal may be produced from `reply-only`, `manual-review`,
`work-plan`, `issue-intake`, `issue-to-pr`, `pr-review`, `pr-fix-up`, or
`merge-assist` when the caller, source, runner, and target are already admitted
by policy. The proposal does not create a new mutation lane by itself.

## Authority Model

Authority split:

- Read-only triage may normalize, hydrate, redact, summarize, dedupe, and emit a
  proposal. It must not mutate providers or target repos.
- Proposal preparation may write `runx.operational_proposal.v1` and receipts
  when admitted through an existing action lane.
- Provider publication may publish a tracking-item comment, change-request
  comment, or source thread reply only through the outbox/provider lane and only when the
  required `source_thread_ref` or target ref is recoverable.
- Tracking-item creation and change-request creation are separate action
  authorities. A proposal can recommend them, but provider credentials and lane
  admission must authorize the actual mutation.
- Customer send authority is never implied by a proposal. The `human_gates` must
  name the required approval before any customer-facing send.
- Final change approval is never implied by a proposal or change-producing lane.
  `auto_merge` stays false and `require_human_merge_gate` stays true where a
  repository provider exposes those policy names.
- `final_outcome` is observed or recorded after provider state is known. It is
  not proof that the proposal itself had merge or send authority.

`runx.operational_policy.v1` remains closed. operational_policy.v1 remains unchanged
in this spec: no new `permissions.*` or `outcomes.*` fields are added,
`permissions.auto_merge` remains literal false, and
`permissions.require_human_merge_gate` remains literal true. Proposals are
authorized through an existing action lane plus explicit gates on the proposal
packet, not by widening the policy permissions object.

## Consuming Application Boundary

Runx core owns the generic packet shape, redacted references, dedupe and replay
requirements, evidence refs, authority notes, receipt/story links, result refs,
publication refs, and schema validation. Core may require `proposal_kind`,
`owner_route_id`, `evidence_refs`, `human_gates`, `source_ref`,
`source_thread_ref`, `recommended_actions`, `result_refs`, `publication_refs`,
and `final_outcome` fields when a proposal or story path needs them.

Products own source filters, provider hydration, owner maps, provider channels,
alert projects, support queues, labels, project boards, customer context,
message templates, and concrete escalation destinations. Provider-specific
links such as issues, pull requests, merge requests, tickets, alerts, and
threads are represented as central generic references with roles.
Product-specific values must stay behind product policy or redacted artifact
refs unless a public consumer requires a stable generic schema.

Do not add domain-specific packet families to core. Add product-owned
`proposal_kind` values and product routing policy instead, then use the single
`runx.operational_proposal.v1` envelope for public exchange.

Consuming applications define the `proposal_kind` values and `owner_route_id`
routes that make sense for their product, then translate runx references into
their own provider UX outside this OSS layer. Consuming applications should
read back the same source-thread refs, evidence refs, human gates, result refs,
publication refs, receipts, and final outcome fields from the public envelope.
The hosted approval queue, routing workflow, and provider-specific controls are
product/control-plane concerns; the runx core contract only exposes the
provider-neutral shape those surfaces consume.
