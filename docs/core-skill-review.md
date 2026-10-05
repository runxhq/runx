# Core Skill Product Review

Human-reviewed product decision record, dated 2026-07-20 and updated
2026-08-07 for the operator-first contract. This document is not
regenerated from a parallel JavaScript model of Runx execution; current
structural truth comes from native package validation, operator-context
expansion, the official lock, and package harnesses.

**Status: implemented.** This covers all 83 top-level skill packages: 79 public
and 4 internal.
No additional package is removed or hidden by this review. Improvement recommendations preserve the capability until a separate product decision approves a migration.

## Product bar

A core skill earns its place by providing specialized workflow, domain expertise, reusable resources, governed execution, or a durable artifact that materially improves what an agent can do. Deterministic execution is one valid shape, not the definition of skill value.

1. **Runtime and provider operations** execute a real boundary and prove the effect with runtime or provider readback.
2. **Domain and operator workflows** encode non-obvious procedure, structured judgment, gates, handoffs, and recovery across a recurring job.
3. **Artifact and distribution skills** produce durable, provenance-bound research, content, security, growth, or ecosystem artifacts with review and publication handoffs.
4. **Builder skills** make designing, testing, improving, packaging, or distributing skills materially easier and strengthen the Runx ecosystem flywheel.
5. **Context skills** create reusable bounded packets that improve downstream work without claiming an external mutation.

Every archetype must have a truthful closure, explicit authority and stop conditions, a declared artifact or effect, and replayable proof appropriate to its claim. Provider execution needs real readback. Agent-authored work needs a stable output contract and realistic harness or forward-test evidence.

Provider operations follow one evidence contract: runtime-resolved credentials,
pre-call scope and idempotency checks, optional exact approval only when the
effect explicitly requests a separate human decision, provider execution
through a declared boundary, acknowledgement kept separate from finality, and
stable-id readback before a terminal effect claim. A scoped grant is sufficient
for routine bounded writes. The keyless NWS lane is the reference read shape;
Nitrosend is the reference credentialed account-operation shape.

Managed-agent execution is optional infrastructure, not an admission category. Agent acts yield `needs_agent` by default. In-process execution requires per-run `--managed-agent` consent and a visible round budget; configured credentials are availability, not consent.
Prepared context remains digest-bound and drift-checked for every run, but it
is evidence rather than an approval gate. A separate human decision belongs to
exactly one action owner: the native effect when it requests approval, or an
explicit graph gate when no native effect owns that decision.

The normative detail lives in
[Skill Quality Standard](skill-quality-standard.md).

## Operator-first execution contract

The public name remains the product surface. Direct invocation receives the
exact digest-bound `SKILL.md` and selects one truthful default; nested graphs
select named phase runners and pass typed evidence instead of repeating prior
work. Planning-only behavior is an explicit runner for operation-class skills,
while artifact skills such as Business Ops may truthfully default to a route or
plan packet. Catalog semantic diagnostics are now a blocking package-admission
and core-audit gate, after the full catalog migrated cleanly.

GitHub provider effects now have one provider-neutral admission and readback
contract over two transports: a deterministic existing local `gh` session and
Runx Connect. Explicit bindings win, repository identity comes from caller or
checkout evidence rather than a grant, and both reads and mutations require
the exact admitted scope, idempotency, recovery, and readback. Routine
repository mutations use that scoped grant directly; a skill requests a
resumable human approval only for a genuinely consequential decision. Hosted
Connect continues to own only credential custody, grant
resolution, and bounded provider execution; it does not own skills, operator
state, or local workflow.

Default changes are versioned behavior changes but do not publish a release.
Truthful explicitly named legacy runners remain for at least one normal
compatibility release, and callers that require an explicit plan-only behavior must
select `plan` explicitly. Answer-file resume also remains compatible while
stdin/host-native continuation is the primary path.

## Implemented authoring consolidation

Five overlapping or under-owned package names were retired after their useful behavior moved into canonical owners:

- `design-skill` moved into the read-only `skill-lab design` runner.
- `write-harness` moved into `skill-lab harness`, with path validation and native replay.
- `improve-skill` moved into `skill-lab improve`, preserving evidence-led diagnosis, bounded writes, and regression coverage.
- `skill-testing` moved into `review-skill`, which now owns native inspection, safe harness execution, and evidence-bounded assessment.
- `evolve` was retired because its shipped surface stopped at shallow repository inspection plus spec planning; use `work-plan` for bounded planning, `skill-lab` for skill changes, and the named downstream execution lane for mutation.

The official lock, generated Rust catalog, packet schemas, documentation, and consumers now point only at the canonical owners. No generic host authoring guidance was copied into Runx: a host `skill-creator` may guide the agent, while `skill-lab` remains the only portable Runx implementation. `SKILL.md` is the sole static agent-instruction source; execution profiles contain only tasks, typed contracts, tools, context, authority, and effects. Skill Lab delegates workspace inspection and exact-candidate application to native catalog tools. Native apply reports measured before/after package complexity, stages the complete candidate, parses its contracts, runs the existing in-process harness, and only then commits the same bytes through one rollback-capable filesystem transaction. Skill Lab keeps executable package code to irreducible domain computation and stops a generic gap as `needs_core`; native apply enforces objective safety rather than model-authored architecture prose.

## How this review was performed

- **Static execution audit:** followed each top-level `X.yaml` default runner
  transitively and recorded agent acts, capability boundaries, artifact declarations, metadata gaps, consumers, and fixtures.
- **Archetype-aware proof:** operation fixtures prove effects; supplied-answer harnesses prove agent-artifact contracts without spending model tokens; provider evidence remains separate and read-only.
- **Product-role review:** checked the root skill contract, operator architecture, first-party catalog map, public registry evidence, distribution value, and complete workflow ownership.
- **Removal guard:** a weak implementation becomes an improvement recommendation unless canonical ownership and consumer migration prove the package redundant.

## Evidence recorded at review time

- Top-level core packages: 79
- Public packages: 70; internal packages: 9
- Packages containing agent work: 47
- Agent-only default closures: 2
- Packages without operation proof: 7
- Packages without semantic output proof: 3
- Native standalone/composed operator-journey claims: 140
- Packages without any replayable contract proof: 3
- Public manuals failing the cold-operator floor: 0
- Public manuals missing a composed-skill relationship: 0
- Public harness trials: 70 passed, 0 failed, 0 unproven
- Public packages currently meeting their complete archetype bar: 50
- Recommendations: improve=20, internal_fixture=3, internal_runtime=6, keep=50
- Archetypes: artifact=12, builder=8, context=3, operation=27, runtime=8, workflow=21

The detailed per-fixture results are not checked in as a generated mirror; the
fixtures and native harness are the replayable source of truth. Two safe live
read-only observations supported the provider claims in this review:

| Skill | Lane | Receipt | Observed |
|---|---|---|---|
| `nws-weather-forecast` | keyless read | `sha256:5ac7f93c7684606e3122e4200e881396a54634a0c22fb49221ac59b142367bdb` | 2026-07-16T15:46:02.263Z |
| `nitrosend` | credentialed account read | `sha256:8936ef07b8ae9ae7994c22cad8592a60c516fb9fed6fb5c139a3eb4c0fa18faa` | 2026-07-16T10:36:18.971Z |

Packet distribution was audited separately from runner inspection. Every
runner remains responsible for a complete inspectable input and output schema,
but only reusable named boundaries are emitted under `dist/packets/`. The
generator reads typed parser output, uses explicit native public owners where
there is no skill producer, and removes generated files whose last owner has
disappeared. This prevents both missing public contracts and a global schema
artifact for every runner-local object.

The 20 `improve` decisions below are evidence gaps, not permission to add more
runtime layers. They close through the named safe provider observation or a
harmless scoped mutation and readback, with exact human approval only where the
action itself requires it. Until that evidence exists, the package must retain
its truthful non-final state.

### Packet-depth remediation completed

The ownership audit found 24 packet ids whose producers exposed only
`type: object`. Each was traced rather than mechanically expanded:

- 15 reusable public boundaries now declare their complete nested schema at
  the producing `X.yaml` output;
- 9 runner-local or graph-intermediate identities were removed while their
  ordinary typed outputs remain inspectable; and
- the native payment price, challenge, verification, charge-plan,
  invoice-plan, and refund-plan packets now compose canonical Rust types rather
  than copied or generic nested schemas.

The existing output declaration is the source for inspection, agent context,
runtime validation, packet generation, registry packaging, and harness replay.
No second packet registry or contract version was introduced. Generation now
rejects a bare public object, conflicting producers, orphaned manual packets,
and stale generated artifacts. Open JSON remains only at deliberate protocol
boundaries such as a provider tool's generic arguments, a named extension map,
or an opaque hosted-admission payload inside a typed payment envelope.

## Recommendation meanings

- `keep`: the package has a clear core role and evidence appropriate to its current claim.
- `improve`: keep the package core and close the named execution, artifact, metadata, proof, or provider gap.
- `consolidate_review`: compare overlapping packages as one product pipeline; preserve all names until a canonical owner and migration are explicitly approved.
- `internal_fixture`: retain non-public deterministic test packages used to prove canonical parent skills.
- `internal_runtime`: retain non-public provider execution rails behind their canonical parent skills.

## Package-by-package review

| Skill | Archetype | Catalog role | Default execution shape | Evidence | Decision | Rationale | Improvement |
|---|---|---|---|---|---|---|---|
| agency | workflow | public/canonical | javascript, tool:data.append_event, tool:data.read_events; 1 agent act -> declared artifact | open/advance/status harness only; unattended execution unproven | improve | The case event log and roster check work, but member execution, receipt verification, pending-dispatch recovery, TTL and complete-stream replay are not implemented. Do not compose it with an assistant work loop. | Narrow or rename the public capability after implementing the missing handshake and proof; do not claim it runs a team today. |
| answer-from-docs | context | public/context | javascript, tool:data.digest; 1 agent act -> declared artifact | complete archetype bar | keep | The contributor-owned documentation lane binds one supplied corpus by native digest, requires exact source quotations, and seals unsupported or conflicted outcomes instead of answering from outside knowledge; four focused journeys prove grounded answers, honest refusal, invented-quotation rejection, and missing-input handling. | none |
| audit-receipt | workflow | public/canonical | javascript, tool:receipt.query; 1 agent act -> declared artifact | complete archetype bar | keep | Authority-versus-evidence judgment consumes Runx's native redacted receipt detail, checks only approval requirements declared by the exact act, and never mistakes an admitted routine write for an approval failure; caller summaries are supplemental and missing detail fails closed. | none |
| brand-voice | context | public/context | javascript, tool:data.digest; 1 agent act -> declared artifact | complete archetype bar | keep | The evidence-bound voice packet now has four focused journeys covering safe claims, missing inputs, unknown bindings, and forwarding the exact packet digest into both Ghostwrite and Twitter planning without granting downstream authority. | none |
| business-ops | workflow | public/canonical | javascript | complete archetype bar | keep | The default returns one selected typed lane without storage ceremony or unrelated branches; durable append/readback and the seven-lane planning fan-out remain explicit composed runners. | none |
| charge | operation | public/canonical | tool:provider.mutate, tool:provider.read | harness passed; 0 blocking finding(s); 1 operation proof(s); provider readback unproven | improve | The public contract is a thin hosted mutation and readback boundary; pricing, verification, settlement, recovery, credentials, and ledger state remain private. | Capture safe hosted settlement and recovery evidence before treating paid-operation forwarding as live-proven. |
| chief-of-staff | artifact | public/context | javascript; 1 agent act -> declared artifact | complete archetype bar | keep | The skill now truthfully consumes normalized supplied mailbox and calendar evidence: every source requires an upstream digest and observation time, admission enforces freshness, and deterministic finalization validates priorities, replies, availability, invented references, and mandatory sensitive review across six focused journeys without claiming provider fetch or mutation. | none |
| content-pipeline | artifact | public/context | javascript, tool:data.compare, tool:data.digest, tool:evidence.index_sources, tool:evidence.verify_artifact; 2 agent acts -> declared artifact | complete archetype bar | keep | The distinct local content lane now composes citation-verified research, evidence-bound drafting, deterministic channel packaging, and a provider-neutral handoff without claiming delivery; ready and no-evidence journeys are independently proven. | none |
| contract-drafter | artifact | public/canonical | javascript, tool:data.digest; 1 agent act -> declared artifact | complete archetype bar | keep | The template-bound drafting lane keeps clause wording as bounded agent judgment while deterministic reconciliation rejects missing terms, unresolved placeholders, and undeclared deviations; delivery remains a separate human-gated send proposal. | none |
| crm-cleanup | artifact | public/canonical | javascript, tool:data.digest; 1 agent act -> declared artifact | complete archetype bar | keep | The proposal lane binds the transcript and current records, permits only schema-allowlisted updates backed by verbatim evidence, preserves before values for drift detection, and never claims a CRM write. | none |
| cve-audit | operation | public/canonical | javascript, tool:data.digest, tool:http.query, tool:http.read | complete archetype bar | keep | Native bounded HTTP now owns lockfile and OSV transport, host admission, response limits, and retries; package code retains only exact npm inventory and independently replayed OSV semantics, proven by two live cases and a pre-network refusal. | none |
| data-store | operation | public/canonical | tool:data.append_event, tool:data.read_projection | complete archetype bar | keep | Direct writes now default to append plus projection readback, while composed graphs retain exact append and bounded read runners over the same local-first adapter contract. | none |
| data-subject-request | workflow | public/canonical | javascript, tool:data.append_event, tool:data.digest | complete archetype bar | keep | The privacy-request gate deterministically binds identity proof, policy, lawful basis, and bounded data classes, records one idempotent verdict through data-store, and emits only a downstream handoff without claiming erasure or export. | none |
| deep-research | artifact | public/context | javascript, tool:data.compare, tool:data.digest, tool:evidence.index_sources, tool:evidence.verify_artifact; 2 agent acts -> declared artifact | complete archetype bar | keep | The specialist composition now inherits governed source indexing and citation verification from research, evidence-bound drafting and final acceptance from Ghostwrite, and proves both a publication-free decision brief and a no-source stop in two focused journeys. | none |
| dispute-respond | workflow | public/canonical | javascript, tool:data.compare, tool:data.digest, tool:provider.mutate, tool:provider.read, tool:receipt.prove; 1 agent act -> declared artifact | harness passed; 0 blocking finding(s); 2 operation proof(s); 1 agent-contract proof(s); 2 operator journey(s); provider readback unproven | improve | The default resolves exact receipts, prepares the response, and files the admitted packet through one approval and independent readback; respond and file remain reusable phase runners. | Capture a genuine provider-charge settlement receipt and one safe approved provider filing/readback before treating live dispute completion as proven. |
| ecosystem-brief | artifact | public/context | javascript, tool:data.compare, tool:data.digest, tool:evidence.index_sources, tool:evidence.verify_artifact; 2 agent acts -> declared artifact | complete archetype bar | keep | The specialist brief now freshness-filters governed sources before canonical research and Ghostwrite composition, with fresh, missing-source, and stale-source journeys proving its contract. | none |
| extract | operation | public/canonical | tool:structured.extract | complete archetype bar | keep | Schema-validated extraction with digest provenance is a deterministic and independently proven capability. | none |
| ghostwrite | artifact | public/context | javascript, tool:data.digest, tool:evidence.verify_artifact; 1 agent act -> declared artifact | complete archetype bar | keep | The reusable writing primitive now releases drafts only through the canonical evidence verifier, requires explicit context bindings, and separates deterministic packaging and provider-neutral handoff from provider delivery across five focused journeys. | none |
| github-sync | operation | public/canonical | javascript, tool:data.compare, tool:data.digest, tool:provider.mutate, tool:provider.read | harness passed; 0 blocking finding(s); 5 operation proof(s); 1 operator journey(s); provider readback unproven | improve | The end-to-end default performs the selected bounded pull or push; local authenticated GitHub reads are preferred, writes stay digest-bound and scoped by the admitted grant, and plan/pull/push remain reusable composed runners. | Capture one safe live pull and one harmless scoped push/readback, including ambiguous-grant refusal evidence. |
| google-analytics | operation | public/branded | javascript, tool:data.digest, tool:provider.read | harness passed; 0 blocking finding(s); 3 operation proof(s); 3 operator journey(s); provider readback unproven | improve | The connector-neutral GA4 skill has focused proof for ordered report normalization, privacy-threshold caveats, and header-drift refusal, while its optional hosted driver bounds OAuth scope, discovery, report translation, quota, and metadata projection; live provider readback is not yet proven. | Capture safe live property, metadata, standard-report, and realtime-report receipts, including quota, privacy caveats, pagination, ambiguous-grant refusal, and a provider error. |
| google-search-console | operation | public/branded | javascript, tool:data.digest, tool:provider.read | harness passed; 0 blocking finding(s); 8 operation proof(s); 7 operator journey(s); provider readback unproven | improve | The connector-neutral skill has eight focused journeys covering bounded performance and freshness evidence, hourly-data refusal, HTTP migration properties, sitemap planning, and general-indexing refusal; its mutation lane is digest-bound, grant-scoped, idempotent, narrowly projected, and independently read back, but live provider execution is not yet proven. | Capture safe live site, performance, and URL-inspection reads, then one harmless scoped sitemap submit/readback with ambiguous-grant and provider-error evidence before treating provider execution as fully proven. |
| governed-outbound | workflow | public/context | javascript, tool:data.digest, tool:provider.mutate, tool:provider.read, tool:web.fetch; 2 agent acts -> declared artifact | complete archetype bar | keep | The default fetches, redacts, binds one scrubbed payload placeholder, then delegates exact approved delivery and readback to send-as; plan remains an explicit provider-neutral handoff. | none |
| helpdesk | workflow | public/canonical | javascript | complete archetype bar | keep | The published and dogfooded support classifier has truthful capability metadata, a verified deterministic runner, and a human-gated send boundary. | none |
| incident-commander | workflow | public/context | javascript; 1 agent act -> declared artifact | complete archetype bar | keep | One bounded incident turn returns an exact roster-constrained dispatch, approval escalation, receipt-verified resolution, or actionable blocker; canonical agency remains the durable case owner. | none |
| issue-intake | workflow | public/context | 1 agent act -> declared artifact | complete archetype bar | keep | Turning noisy requests into bounded change artifacts and explicit lanes is recurring operator work, with truthful metadata and four replayable intake-contract cases. | none |
| issue-to-pr | workflow | public/canonical | javascript, tool:external_receipt.verify, tool:provider.mutate, tool:provider.read; 1 agent act -> declared artifact | harness passed; 0 blocking finding(s); 2 operation proof(s); 1 agent-contract proof(s); 3 operator journey(s); provider readback unproven | improve | The host now edits and tests normally, calls finalize once, and returns a typed host result; native external-receipt verification binds that evidence to the exact commit, while optional PR publication is one grant-scoped mutation/readback with no mandatory feed, outbox, or duplicated lifecycle. | Capture one harmless scoped draft-PR push with stable provider readback before treating provider completion as proven. |
| issue-triage | workflow | public/canonical | javascript, tool:data.digest, tool:provider.read; 1 agent act | harness passed; 0 blocking finding(s); 2 operation proof(s); 3 agent-contract proof(s); 2 operator journey(s); provider readback unproven | improve | The default lane now reads an exact issue snapshot through native scoped provider access and produces a response draft without mutation; supplied snapshots remain an explicit offline path labelled as supplied evidence. | Capture one safe live provider-read triage receipt to complement the focused draft-versus-mutation harness proof. |
| knowledge-router | workflow | public/context | javascript, tool:data.digest; 1 agent act -> declared artifact | complete archetype bar | keep | It now owns a distinct knowledge-catalog contract and deterministically rejects invented source, owner, and follow-up skill references before sealing a route. | none |
| lead-enrichment | artifact | public/context | javascript; 1 agent act -> declared artifact | complete archetype bar | keep | The skill now truthfully owns supplied-signal synthesis: admission requires stable upstream source digests and timestamps, labels their provenance without claiming provider verification, enforces freshness and consent, rejects invented evidence, and proves the complete packet handoff into lead-router across five focused journeys. | none |
| lead-router | workflow | public/context | javascript, tool:data.digest; 1 agent act -> declared artifact | complete archetype bar | keep | Validated enrichment evidence and consent now bind every route; do-not-contact deterministically records a hold only in the signed Runx receipt, outreach emits the exact canonical send-as input contract, and four focused journeys prove forward compatibility and invented-evidence refusal without claiming delivery. | none |
| least-privilege | operation | public/canonical | javascript, tool:receipt.query | complete archetype bar | keep | The auditor now compares the caller's grant baseline with normalized exercised scopes from native redacted receipt detail and defers when that evidence is absent. | none |
| ledger | operation | public/canonical | javascript, tool:receipt.query | complete archetype bar | keep | Cross-run receipt queries and chain verification run through the direct native reader; isolated production-signed trials prove bounded result limits and fail-closed broken-chain reporting. | none |
| list-hygiene-judge | operation | public/canonical | javascript, tool:data.append_event, tool:data.read_projection | complete archetype bar | keep | Bounded current consent evidence drives a conservative deterministic decision; only admitted suppression or re-permission transitions reach the native ledger append, and exact projection readback proves the write while unsubscribe, existing suppression, stale evidence, and clean recent contacts remain safe. | none |
| meeting-followup | workflow | public/context | javascript, tool:data.digest; 1 agent act -> declared artifact | complete archetype bar | keep | The contributor-owned meeting lane turns a bounded transcript into evidence-quoted decisions, action items, unsent follow-up copy, and explicitly uncreated task proposals; nine focused journeys prove owner and date ambiguity, injection resistance, exact evidence, non-actionable discussion, and missing-input stops without embedding a provider adapter. | none |
| marketplace-invoke | operation | public/canonical | javascript, tool:provider.mutate | harness passed; 0 blocking finding(s); 2 operation proof(s); 2 operator journey(s); provider readback unproven | improve | The generic marketplace invocation contract composes one rail-specific buyer skill behind a stable listing, vendor, settlement-family, authority, and idempotency boundary; it neither owns provider credentials nor embeds vendor-specific behavior. | Capture one safe hosted paid invocation with provider, resource, and nested receipt readback; add sibling settlement-family branches only when their hosted adapters exist. |
| mock-charge | runtime | internal/harness-fixture | javascript | internal; 0 blocking finding(s); not trialled | internal_fixture | Deterministic local simulator proves the charge package shape and always reports that no money moved. | none |
| mock-pay | runtime | internal/harness-fixture | javascript | internal; 0 blocking finding(s); not trialled | internal_fixture | Deterministic local simulator proves the spend package shape and always reports that no money moved. | none |
| mock-refund | runtime | internal/harness-fixture | javascript | internal; 0 blocking finding(s); not trialled | internal_fixture | Deterministic local simulator proves the refund package shape and always reports that no money moved. | none |
| moltbook | operation | public/context | javascript, tool:data.digest, tool:provider.read; 1 agent act -> declared artifact | harness passed; 0 blocking finding(s); 3 operation proof(s); 2 agent-contract proof(s); 4 operator journey(s); provider readback unproven | improve | The default now scans a bounded live provider feed; supplied-snapshot analysis is explicit, and publication remains a separate digest-bound approved mutation/readback lane. | Capture one safe live scan and one explicitly approved harmless post/readback before treating live publication as proven. |
| n8n-handoff | operation | public/context | tool:control.prepare_handoff, tool:provider.mutate, tool:provider.read | harness passed; 0 blocking finding(s); 3 operation proof(s); 5 operator journey(s); provider readback proven in harness | keep | A scoped idempotent n8n handoff is a real integration boundary; native handoff normalization and a tenant-agnostic provider binding own context validation, exact approval, delivery, and readback. | none |
| nitrosend | operation | public/branded | javascript, tool:http.query | complete archetype bar | keep | The unified Nitrosend domain skill has isolated adapter trials and sealed live read-only provider evidence. | none |
| nws-weather-forecast | operation | public/branded | tool:http.read | complete archetype bar | keep | The NWS skill performs bounded keyless provider reads with live HTTP proof. | none |
| open-meteo-weather-forecast | operation | public/branded | tool:http.read | complete archetype bar | keep | The Open-Meteo skill performs global keyless forecast and air-quality reads with live HTTP proof. | none |
| operator-inbox | operation | public/canonical | tool:data.list_stream_heads | complete archetype bar | keep | Direct use now opens a bounded local `.runx` action queue; explicit write and read runners preserve dispositions and normalized provider observations without moving queue ownership into Cloud. | none |
| attention-review | context | public/context | javascript; 1 agent act -> declared artifact | harness passed; live read-only attention turn; no delivery claim | keep | The bounded attention rubric turns admitted source evidence and current dispositions into a source-bound brief, exact read-only proposals, and a clamped next-check recommendation. The local operator retains source reads, queue state, timer and delivery authority. | Prove the composed approval and follow-up journey before claiming an unattended operating assistant. |
| conversation-review | context | public/context | javascript; 1 agent act -> declared artifact | three harness cases and live Slack/Nitrosend source-to-draft trials; no delivery claim | keep | A normalized, complete thread yields an unsent reply, follow-up proposal, or quoted coding handoff; incomplete coverage is held. The skill owns no provider reads, sends, queue, or timer. | Prove exact approved send and follow-up admission; review receipts alone do not establish delivery. |
| ops-desk | workflow | public/canonical | tool:data.read_projection; 1 agent act -> declared artifact | complete archetype bar | keep | The default starts from bounded durable state and returns the chosen governed lane or exact blocker; supplied-state, agency dispatch, and action-review runners remain explicit. | none |
| organic-growth | artifact | public/canonical | javascript, tool:evidence.verify_artifact; 1 agent act -> declared artifact | complete archetype bar | keep | The provider-neutral planning skill turns bounded search, analytics, site, and market evidence into a prioritized decision packet, then deterministically binds every material claim and action to admitted source digests; three focused journeys prove cross-source usefulness, invented-evidence refusal, and a no-evidence stop without claiming execution or ranking effects. | none |
| adopt-skill | builder | public/canonical | javascript, tool:data.digest, tool:fs.read, tool:git.blob_digest, tool:runx.skill.validate; 1 agent act -> declared artifact | complete archetype bar | keep | The public adoption lane targets Runx's native upstream binding architecture: it recomputes local source and Git-blob pins, requires explicit provenance, keeps profile design as bounded agent judgment, emits exact binding.json and X.yaml artifacts, and releases them only after native inspection, isolated harness proof, and catalog-check dogfood. | none |
| policy-author | builder | public/canonical | javascript, tool:policy.lint; 1 agent act -> declared artifact | complete archetype bar | keep | Compiling plain-English governance into validated Runx policy is core builder work; the native lint finalizer now rejects invalid drafts and prevents the tightening lane from widening existing authority. | none |
| purchase-approval | operation | public/canonical | javascript, tool:data.digest | complete archetype bar | keep | The deterministic pre-spend gate binds the exact request and policy, refuses missing or exceeded authority, routes threshold decisions to a named human lane, and never claims that money moved. | none |
| github-pr-comment | workflow | public/canonical | tool:data.compare, tool:data.digest, tool:provider.mutate, tool:provider.read | harness passed; 0 blocking finding(s); 1 operation proof(s); 1 operator journey(s); provider readback unproven | improve | The default GitHub PR-comment lane uses its scoped grant once, binds the comment digest and idempotency key, and performs independent readback; the explicitly selected MCP composition retains its own human-gated contract. | Capture one safe connector comment/readback in addition to the existing MCP harness evidence. |
| postmortem-maker | artifact | public/canonical | javascript, tool:data.digest; 1 agent act -> declared artifact | complete archetype bar | keep | The postmortem lane binds the fragment set, admits only verbatim fragment-cited timeline entries and root causes, blocks publication while unknowns remain, and keeps the comms send behind a human-gated proposal. | none |
| prior-art | builder | public/context | javascript, tool:fs.read_bundle, tool:runx.skill.inspect; 1 agent act -> declared artifact | complete archetype bar | keep | The builder now deterministically indexes bounded repository sources and the local skill catalog before agent comparison, then rejects missing sources, invented verified citations, and unknown adjacent skills before sealing a reuse, amendment, new-work, or stop decision. | none |
| redact-pii | operation | public/canonical | javascript, tool:data.digest; 1 agent act -> declared artifact | complete archetype bar | keep | The trust-boundary guard now keeps semantic detection as reviewer judgment while deterministic code owns replacements, digests, residual scanning, and release of scrubbed content; seven focused journeys prove ready, block, missing-input, direct and obfuscated leakage, ambiguity, and tokenization paths. | none |
| reflect-digest | builder | internal/context | javascript, tool:data.list_stream_heads; 1 agent act -> declared artifact | complete archetype bar | internal_runtime | Cross-run reflection aggregation remains exact internal improvement plumbing behind run-history, diagnose-skill-run, and skill-lab improve. | none |
| refund | operation | public/canonical | tool:provider.mutate, tool:provider.read | harness passed; 0 blocking finding(s); 1 operation proof(s); provider readback unproven | improve | The public contract delegates receipt resolution, authority accounting, provider reversal, recovery, and ledger state to hosted execution and requires readback. | Prove bounded hosted reversal and recovery against a safe provider sandbox. |
| release | workflow | public/canonical | javascript, tool:command.execute, tool:command.plan, tool:fs.read; 1 agent act -> declared artifact | harness passed; 0 blocking finding(s); 2 operation proof(s); 1 agent-contract proof(s); 2 operator journey(s); provider readback unproven | keep | The flagship release workflow now reads and digests project-owned profiles through the native filesystem boundary, plans and executes exact argv commands through the native process boundary, binds one approval to the publish plan, and requires independent provider readback; focused journeys prove both successful release and blocked preparation without package-owned process plumbing. | none |
| research | artifact | public/context | tool:data.compare, tool:evidence.index_sources, tool:evidence.verify_artifact; 1 agent act -> declared artifact | complete archetype bar | keep | The reusable research primitive admits bounded remote/provider packets or native local files into one source index, releases one shared synthesis path only through source and content-digest verification, and fails closed on missing evidence or invented citations. | none |
| diagnose-skill-run | builder | public/context | javascript, tool:receipt.query; 1 agent act -> declared artifact | complete archetype bar | keep | Failure diagnosis uses native redacted receipt detail and its stable packet is validated and preserved by skill-lab improve before any package mutation. | none |
| review-skill | builder | public/canonical | tool:runx.skill.validate; 1 agent act -> declared artifact | complete archetype bar | keep | It is the canonical skill evaluation surface: native inspection and safe harness evidence feed a separate bounded assessment runner without publication or mutation claims. | none |
| run-history | operation | public/canonical | javascript, tool:receipt.query, tool:runx.skill.inspect | complete archetype bar | keep | The default runner now queries native Runx history and catalog projections directly, computes deterministic outcome and coverage metrics, and routes follow-up without an agent-authored data layer. | none |
| sbom-maker | artifact | public/canonical | javascript, tool:data.digest | complete archetype bar | keep | The deterministic inventory lane derives a CycloneDX SBOM from one exact lockfile digest, surfaces license risk and evidence locations, refuses unsupported or unpinned inputs, and performs no network fetch. | none |
| schema-guard | operation | public/canonical | javascript, tool:data.digest | complete archetype bar | keep | The compatibility gate binds both contracts and real samples, deterministically refuses disallowed breaking changes or invalid coverage, and emits only an approval-gated publication proposal with no live write claim. | none |
| send-as | workflow | public/canonical | javascript, tool:data.digest, tool:provider.mutate, tool:provider.read; 1 agent act -> declared artifact | complete archetype bar | keep | The default now plans, derives one digest-bound provider-neutral request, performs the exact approved send, and verifies readback; plan, apply, and verify consume typed prior artifacts without repeating work or caller-authored provider plumbing. | none |
| settle-invoice | workflow | public/canonical | tool:provider.mutate, tool:provider.read | harness passed; 0 blocking finding(s); 1 operation proof(s); provider readback unproven | improve | The public contract delegates invoice admission and settlement to hosted execution and requires provider readback. | Prove one safe hosted invoice settlement and recovery path. |
| sign-receipt | operation | public/canonical | tool:receipt.attest | complete archetype bar | keep | The thin public skill now delegates bounded evidence validation and stable attestation digesting to native receipt.attest, then relies on the runtime's real receipt signer; four semantic journeys and a production-mode Ed25519 tree verification prove the boundary without claiming the external action was verified. | none |
| skill-lab | builder | public/canonical | tool:runx.skill.apply, tool:runx.skill.bind, tool:runx.skill.inspect, tool:runx.skill.plan; 2 agent acts -> declared artifact | complete archetype bar | keep | The canonical authoring surface keeps all static operating knowledge in SKILL.md and agent judgment bounded while native Runx owns measured before/after complexity, exact-candidate validation, harness replay, and one rollback-capable apply transaction; its package harness proves build, improve, fixture, and earned needs_core paths without package-local authoring scripts. | none |
| slack | operation | public/branded | tool:provider.read | harness passed; 0 blocking finding(s); 1 operation proof(s); 1 agent-contract proof(s); 1 operator journey(s); provider readback unproven | improve | Reusable Slack search, bounded thread reads, digest-bound reply authorization, approved idempotent delivery, and exact-message readback now compose native provider tools without putting operator workflow or state in Cloud. | Capture one safe real-workspace bounded search and one approved thread reply/readback, including ambiguous-grant and rate-limit evidence, before treating live Slack operation as fully proven. |
| slack-notify | operation | public/branded | javascript, tool:data.compare, tool:data.digest, tool:provider.mutate, tool:provider.read; 1 agent act -> declared artifact | harness passed; 0 blocking finding(s); 3 operation proof(s); 1 agent-contract proof(s); 3 operator journey(s); provider readback unproven | improve | The default binds, approves, posts, and reads back one exact Slack notification; plan and deliver remain typed phase runners with drift refusal. | Capture one safe real-workspace post/readback and the ambiguous-grant refusal before treating provider execution as fully proven. |
| sourcey | workflow | public/context | tool:fs.apply_bundle, tool:sourcey.build, tool:sourcey.package, tool:sourcey.verify; 4 agent acts -> declared artifact | complete archetype bar | keep | Sourcey proves bounded authoring, scoped transactional filesystem writes, deterministic build, critique, revision, rebuild, verification, and packaging in one isolated operator journey without a performative approval before reversible local work. | none |
| spend | operation | public/canonical | tool:provider.mutate, tool:provider.read | harness passed; 0 blocking finding(s); 1 operation proof(s); provider readback unproven | improve | The public contract delegates quote, reservation, rail execution, recovery, finality, credentials, and ledger state to hosted execution and requires readback. | Prove hosted fulfillment and recovery with independent provider readback on each supported rail. |
| sql-analyst | artifact | public/context | javascript, tool:data.list_stream_heads, tool:data.read_events, tool:data.read_projection; 4 agent acts -> declared artifact | complete archetype bar | keep | The default validates a read-only plan, performs one declared data-store read, and interprets only the returned evidence; the explicit plan runner still never emits or executes raw SQL. | none |
| stripe-pay | operation | public/branded | tool:provider.mutate, tool:provider.read | harness passed; 0 blocking finding(s); 1 operation proof(s); provider readback unproven | improve | Stripe payment is a thin discoverable facade over the canonical hosted spend contract; no Stripe SDK or rail implementation remains in OSS. | Prove a safe hosted sandbox mutation and readback path. |
| taste-profile | context | public/context | javascript, tool:data.digest; 1 agent act -> declared artifact | complete archetype bar | keep | The scoped preference packet now has four focused journeys covering thin evidence, bounded preferences, unknown bindings, and forwarding the exact packet digest into both Ghostwrite and Twitter planning without granting downstream authority. | none |
| twitter | operation | public/branded | javascript, tool:data.digest, tool:http.read | harness passed; 0 blocking finding(s); 4 operation proof(s); 2 agent-contract proof(s); 4 operator journey(s); provider readback unproven | improve | Native HTTP owns live transport, OAuth delivery, host admission, pagination, response capture, and readback; native fs.read_bundle now owns all archive bytes; a live bearer read returned five digest-bound items in one 200 response and eight semantic package journeys pass. | Capture one explicitly approved harmless provider mutation with stable readback before treating the write lane as fully proven. |
| vault-unseal | workflow | public/canonical | javascript, tool:provider.mutate, tool:provider.read | harness passed; 0 blocking finding(s); 3 operation proof(s); provider readback unproven | improve | The default prepares, approves, executes, and reads back one bounded opaque handle request; plan-only use remains explicit and secret material never enters the result. | Capture a safe sandbox issuance/readback and confirm no secret material crosses the projected provider boundary. |
| vuln-disclosure | workflow | public/canonical | javascript, tool:data.compare, tool:data.digest, tool:provider.mutate, tool:provider.read; 1 agent act -> declared artifact | harness passed; 0 blocking finding(s); 2 operation proof(s); 2 agent-contract proof(s); 2 operator journey(s); provider readback unproven | improve | The default preserves the cve-audit/triage evidence chain through exact preparation, approved publication, and advisory readback; preparation and publish remain reusable phase runners. | Capture one safe sandbox or draft-channel publication/readback before treating live disclosure as proven. |
| vuln-triage | artifact | public/canonical | javascript, tool:data.digest; 1 agent act -> declared artifact | complete archetype bar | keep | Every assessment is now bound to independently verified exact-version CVE identity, requires bounded confidence for exposure and priority judgment, emits deterministic escalation criteria, and refuses unverified or invented findings across four focused journeys. | none |
| weather-forecast | artifact | public/canonical | javascript; 1 agent act -> declared artifact | complete archetype bar | keep | Provider-neutral normalization keeps forecast interpretation as bounded analyst judgment, then deterministically binds location, horizon, timestamp, provenance, and period names; focused journeys prove ready, missing-evidence, invented-period, and life-safety paths. | none |
| web-fetch | operation | public/canonical | tool:web.fetch | complete archetype bar | keep | Allowlisted fetch with digest provenance is a genuine network operation with live keyless proof. | none |
| work-plan | builder | public/context | javascript, tool:runx.skill.inspect; 1 agent act -> declared artifact | complete archetype bar | keep | The planner now preserves issue-intake change sets and captured control context, keeps decomposition as bounded agent judgment, then deterministically validates ordered phase and step DAGs, mutation scopes, context dependencies, catalog references, and canonical skill-lab ownership before releasing an executable plan. | none |
| x402-pay | operation | public/branded | javascript, tool:provider.mutate, tool:provider.read | harness passed; 0 blocking finding(s); 1 operation proof(s); provider readback unproven | improve | The public contract delegates x402 validation, signing, settlement, recovery, and ledger state to hosted execution and requires paid-resource readback. | Prove bounded testnet settlement, recovery, and independent paid-resource readback with a hosted adapter. |
| zapier-handoff | operation | public/context | tool:control.prepare_handoff, tool:provider.mutate, tool:provider.read | harness passed; 0 blocking finding(s); 3 operation proof(s); 5 operator journey(s); provider readback proven in harness | keep | A scoped idempotent Zapier handoff is a real integration boundary; native handoff normalization and a tenant-agnostic provider binding own context validation, exact approval, delivery, and readback. | none |

## Consolidation and removal guard

Private payment runtime packages were removed from OSS by explicit product decision. Future removals must still identify the canonical replacement, prove consumer and registry migration, preserve useful artifacts and history, and receive explicit product approval before the tree changes.
