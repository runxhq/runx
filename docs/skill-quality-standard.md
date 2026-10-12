# Skill Quality Standard

Public Runx skills are portable execution contracts. A skill earns its place by
making an agent materially better at a recurring job: it may execute a governed
operation, encode a non-obvious workflow, produce a durable artifact, build or
improve other skills, or provide reusable bounded context. Deterministic
provider execution is one valuable shape, not the definition of skill value.

This standard applies to existing core skills and to every proposed addition.
It evaluates the claim a skill actually makes; it does not force every package
into the same architecture.

## Operator-value admission

A core skill must provide at least one of these forms of leverage:

1. **Operation** — crosses a runtime, state, protocol, or provider boundary and
   verifies the effect or readback.
2. **Workflow** — compresses a fragile recurring job with domain procedure,
   authority, gates, handoffs, recovery, and a truthful terminal state.
3. **Artifact** — creates a durable, provenance-bound output such as research,
   content, security analysis, growth intelligence, or a publication packet.
4. **Builder** — makes skills or governed systems easier to design, test,
   review, package, improve, or distribute.
5. **Context** — creates a reusable bounded packet that improves downstream
   decisions without pretending to perform an external action.

Internal runtime rails, fixtures, and owner-local graph stages can remain
non-public. Their value is judged through the canonical skill that owns them.

A package fails admission when it merely renames generic prompting, duplicates
another package without a distinct contract, claims an effect it cannot prove,
or leaves the caller to invent the actual procedure. Research, writing,
planning, and review are not disqualified: they qualify when the package adds
specialized sources, constraints, provenance, structure, evaluation, or
handoffs that materially improve the resulting work.

## Universal bar

Every public skill must have:

- a clear recurring job and a distinct owner in the catalog;
- a bounded default runner with a truthful terminal state;
- explicit inputs, outputs, authority, approvals, and stop conditions;
- a declared artifact or effect that survives beyond model prose;
- provenance for material facts, state, amounts, actors, and recommendations;
- replayable proof appropriate to its archetype;
- no hidden credentials, implicit consent, or unsupported provider claims;
- a concise package containing only files the skill consumes or emits.

Weak implementation is normally an improvement finding, not a deletion
decision. Removal or consolidation additionally requires a named canonical
replacement, consumer and registry migration, preservation of useful evidence,
and explicit product approval.

## Proof by archetype

Proof must match the claim:

| Archetype | Minimum proof |
|---|---|
| Operation | A no-managed-agent fixture or bounded live trial crosses the claimed boundary and verifies runtime or provider readback. |
| Workflow | A realistic path reaches each consequential gate and terminal state; deterministic stages use real fixtures, while supplied agent answers may prove the declared artifact contract. |
| Artifact | A realistic source packet produces schema-valid output with provenance, and a forward test or evaluator checks usefulness for its named consumer. |
| Builder | A fixture produces a valid package, policy, harness, or change artifact and runs the native validator that would accept it. |
| Context | A fixture produces the bounded context packet and a downstream forward test shows that the declared consumer can use it without inventing missing fields. |

`caller.answers` can prove graph wiring and an agent-artifact contract. It
cannot prove a provider mutation, network result, payment, send, publish, or
other external effect. Live destructive proof is never required when a faithful
isolated fixture plus refusal and, when requested, approval cases establish the
same boundary safely.

## Provider operation contract

A reusable provider operation keeps governance, transport, and proof separate:

1. Resolve a bounded provider operation and credential grant without exposing
   provider secrets to the model, skill inputs, command arguments, or artifacts.
2. Validate scope, audience, payload bounds, and idempotency before the provider
   call. The admitted grant is sufficient for a routine bounded write. If the
   action warrants a separate human decision, request it once at the native
   effect boundary, not during harmless planning or inspection. Pass one stable
   retry identity through the operation step's `idempotency_key`; Runx binds
   it to the attempt outside provider-specific payload JSON.
3. Execute through an `operation` step on a connector the skill lists: its
   declared HTTP requests, MCP tool, or process tool. On the hosted placement
   Runx resolves the unique active grant and Connect renders only the
   connector's declared request. Agent prose may choose or author inputs but may
   not stand in for transport.
4. Bind readback to the expected resource identity. Hosted operations may
   declare `expected_result` fields that must match exactly and `result_fields`
   that form the complete provider result allowed into the receipt. Secret-
   adjacent operations must use this projection so undeclared result material
   cannot cross into skill output.
5. Record provider acknowledgement separately from finality. An accepted
   request is not delivered, published, paid, or applied unless provider
   evidence proves that terminal state.
6. Read back the provider object by a stable provider id or deterministic
   request coordinate. A read-only operation may use the bounded provider
   response itself as readback when it includes the requested resource identity
   and fresh response evidence.
7. Seal the receipt with the attempted scope, idempotency key when relevant,
   provider evidence reference, truthful terminal state, and recovery posture.

Fixtures prove request construction, refusal, optional approval, retry, and
readback parsing. They do not prove a live provider accepted or applied an operation.
The keyless NWS forecast lane is the reference read shape; Nitrosend is the
reference credentialed account-operation shape. New provider skills should
reuse this contract instead of inventing adjacent credential loading, approval,
or finality logic.

## Agent execution and consent

Agent work is valid for judgment and authorship. It must be isolated from
deterministic effects and close into a declared artifact packet.

`SKILL.md` is the sole static operating-instruction source for an agent act.
Runx injects the complete owning document into the agent envelope and rejects
runner or graph-step `instructions` fields. The envelope's task, typed inputs,
outputs, allowed tools, and prepared context identify the current act without a
second prompt contract. Nested skills run under their own documents; downstream
steps receive declared outputs and context skills with provenance rather than a
flattened prompt dump.

The normal Runx path yields `needs_agent` to the caller. In-process managed
agent execution requires explicit per-run `--managed-agent` consent, displays
the act count and round budget before execution, and remains bounded. Available
model credentials are capability, not consent. A review must not spend model
tokens merely to prove a deterministic boundary or a supplied-answer contract.
Every managed run records logical rounds, actual model calls, tool-call counts,
and bounded tool statuses. A provider, tool, empty-turn, or round-budget failure
is sealed into local history with a sanitized reason and exits nonzero; prompts,
credentials, and raw provider or tool bodies are never failure telemetry.

Prepared context is always digest-bound and drift-checked, but it is not an
approval gate. It proves what was selected and prevents context or artifact
drift without fabricating human authority. Each consequential action has one
approval owner at the point of use. A connector operation step owns any exact
approval it declares or its connector requires; an explicit graph approval is
valid only when no native effect owns the decision. Never place a second
approval immediately before an effect that already requested exact human
approval, and never invent approval merely because a capability mutates state.

## `SKILL.md` content

`SKILL.md` is the public capability manual shared by the operator and the
operating agent. An operator opening it without prior context must be able to
understand what the skill does, why and when to use it, what it will do in
sequence, which other skills it composes with, what authority and evidence it
needs, what it returns, and where it will stop. The document should teach the
capability's mental model and preserve its product voice; it is not an internal
prompt stub.

Its frontmatter description owns triggering. Its body owns the capability
overview, operating model, non-obvious procedure, chain relationships, input
and output semantics, evidence and authority rules, finality and recovery,
relevant edge cases, useful examples, bundled resources, and any task-specific
agent contracts. Use a structure natural to the capability rather than empty
boilerplate. Simple facades and internal rails may be concise, but no public
guide may be replaced with terse imperatives, schema fields, or task clauses.

When an existing skill changes, preserve useful human context and reconcile it
with the executable profile. Remove prose only when it is false, duplicated, or
irrelevant. Moving enforcement into native code does not make the explanation
dispensable: operators still need to understand the boundary and its reason.
Task contracts belong after the operating guide and may sharpen a bounded
agent act; they never substitute for the guide.

The catalog audit enforces a deliberately structural anti-stub floor: before
any agent task contracts, a public guide needs a title, a real section, and
explanatory prose. Skill-chain facts are owned by the typed execution closure
emitted by native `runx skill inspect` and surfaced in operator preflight.
Manuals should still explain meaningful upstream and downstream relationships
in natural language, but they do not duplicate that closure in a machine-parsed
prose registry. The audit does not parse `run.yaml` through a parallel JavaScript
model. Private nested stages remain visible in native execution-closure
evidence.

That structural floor is not a claim that the guide is substantive, nor should
it be met with filler. Review still judges whether the manual explains the
capability accurately, naturally, and with enough domain weight for a cold
operator.

## Content bar

- Name the evidence and distinguish source facts from inference.
- Name the authority; intent alone does not grant permission.
- Name a separate human gate when an action genuinely requires that decision;
  do not invent one merely because a scoped capability writes state.
- Name finality precisely: a plan is not a delivery, and an accepted request is
  not a verified external effect.
- Fail closed on stale evidence, replay, scope mismatch, ambiguous ownership,
  missing consent, or missing provider readback.
- Keep raw secrets out of inputs, logs, artifacts, receipts, and examples.
- Preserve domain boundaries: auditors do not silently repair, planners do not
  claim execution, and provider facades do not replace canonical governance.
- Make recovery and idempotency explicit wherever retries can cause harm.

## Execution profile discipline

`run.yaml` owns executable capability and governance:

- runner-owned nested inputs declare their JSON Schema fragment inline;
- an input that is exactly one canonical packet declares `type: json` and
  `packet: <packet-id>`, so the runtime resolves one catalog-owned schema for
  validation, inspection, export, harness, and registry packaging;
- consumers never copy packet schemas, and a weak canonical packet contract is
  fixed at its producer before downstream adoption;
- a runner-specific nested value remains an inline schema; `packet` is reserved
  for the complete value at a named reusable skill, runtime, SDK, provider,
  receipt, or registry boundary, never as a way to make inspection richer;
- graph intermediates and one-run implementation values do not mint packet ids;
  every distributed packet has an active producer and consumer or an explicit
  public native owner, and generated artifacts are removed with that ownership;
- a named packet describes its semantic fields or references a canonical typed
  contract; bare `type: object`, unconstrained `{}`, and opaque bags do not pass
  as complete inspection merely because a packet id exists. Open JSON is valid
  only as a named protocol extension or generic data payload inside an otherwise
  bounded semantic envelope;
- object explicitness is recursive: every nested `type: object` declares
  properties or an explicit `additionalProperties` policy. Use `type: json` for
  deliberately arbitrary JSON instead of presenting it as a typed object;
- a producing output's inline `schema` owns nested properties, required fields,
  enums, and bounds once; parser admission, agent context, runtime validation,
  packet distribution, exports, and harness replay consume that declaration;

- runners, typed inputs and outputs, and default selection;
- agent-versus-deterministic step boundaries;
- tool, adapter, context-skill, and graph wiring;
- authority, approval, scopes, and receipt-act mappings;
- side-effect posture and truthful completion semantics;
- artifact packets and focused harness declarations.

Typed outputs and packets belong to the step that produces them. A runner is a
composition boundary, not a second producer: it must end in an explicit
package/finalize step when the workflow needs one reusable result, and that
terminal producer owns the output and packet contract.

## Packet versions and native catalog drift

Public packet ids use lowercase
`<publisher>.<name>[.<name>...].vN`, where `N` is a positive integer without
leading zeroes. Runx-owned packets use the `runx` publisher namespace; product
and extension owners retain their own namespace. Runtime capability admission
enforces this shape; `.v1` is not an informal label. Contracts change in
place, with every intentional native catalog change visible in the generated
capability snapshot. Do not add V2, aliases, or compatibility readers.

It does not own static model instructions. Keep those exclusively in the
owning `SKILL.md`; the runtime supplies the current task and contracts
separately.

Use the strict profile YAML subset: no anchors, aliases, merge keys, custom
tags, multi-document markers, duplicate mappings, or unknown fields. Do not put
strategy, generated state, secrets, campaign copy, or broad documentation in
the execution profile.

Standalone fixtures should live under `fixtures/` and exercise public runners.
Inline cases remain acceptable where the runtime package already uses them as a
focused evaluator or graph contract, but they should not turn `run.yaml` into a
large scenario archive.

## Architecture admission

Skill authoring must decide ownership before it writes files:

- Reusable skill procedure, end-user and domain-operator UX, local loops,
  queues, and default local state belong in OSS or the owning product. Cloud may
  custody credentials, resolve authoritative grants, execute registered bounded
  provider operations, and run its hosted control plane; it does not become the
  skill or operator owner merely because Hosted Connect is used. Existing Cloud
  code is not architectural precedent. A missing operator capability is fixed
  in OSS or the product, not in a Cloud dogfood script.
- Prefer a declarative graph composed from existing native tools and canonical
  skills.
- Treat the selected runner's execution requirements as the complete
  permission request. Opaque scopes, exact non-secret environment names, and
  runtime metadata stay on their canonical typed fields; credentials belong to
  the connectors the runner calls. Native capabilities and providers enforce the scopes they own;
  Runx does not reconstruct authority from prose or `runx` metadata.
- Prove permission-bearing native and provider paths through the package
  harness using the production catalog, dispatcher, and enforcement owner. A
  realistic admitted case and an exercisable withheld-scope or withheld-grant
  case are part of the capability proof. Do not introduce a harness-only scope
  vocabulary, evaluator, or report. Trusted host processes remain explicitly
  trusted because a portable harness cannot prove the absence of arbitrary
  filesystem, network, or syscall access.
- Tool fixtures resolve the canonical manifest, execute under its declared
  scopes, and validate every declared packet through the production packet
  verifier. `expect.output.matches_packet` is only for a whole self-described
  output packet; do not repeat named artifact checks that the manifest already
  owns.
- Add package executable code only for irreducible domain computation. Its
  admission names the domain boundary, why the graph cannot express it, and
  which existing owners and tools were inspected.
- Express irreducible JavaScript through a worker step, `tool: { module: ... }`: a cohesive
  package module exposes focused functions from resolved inputs and frozen
  declared non-secret environment to JSON values, while Runx owns process
  input, output, errors, wall limits, and worker isolation. Do not simulate named
  operations with public inputs or create one process wrapper per graph step.
  The runtime must enforce a read-only, no-network worker with no workspace,
  writable paths, ambient OS environment, or injected credentials. Reserve a
  process tool, `tool: { command: ... }`, for a genuine executable or protocol
  boundary.
- Process tools and process MCP servers are trusted host
  code. Runx supervises exact invocation and delivered authority but does not
  claim portable filesystem, network, or syscall confinement. Use this lane
  only for irreducible protocol work; never generate sandbox declarations,
  wrappers, or degradation flags.
- Generic packet and evidence digests belong to native `data.digest`. A
  package-local canonical hash is admissible only when the hash is an intrinsic
  field of an established domain or wire protocol, never as a replacement for
  receipt or effect integrity; the module must state that exception at the
  computation boundary.
- Do not describe package code as filling a missing core capability. A genuine
  generic gap returns `needs_core` without writes. The proposal must identify
  either a runtime/security invariant or at least two independent existing
  consumers and explain why a package fallback was rejected.
- The authoring agent explains material ownership decisions when they affect
  the result. The native apply result reports objective before/after files,
  bytes, text lines, executable files, and executable lines.
- Improving a skill includes removing superseded code and manifests. Splitting
  one implementation into more files or moving it into Rust is not evidence of
  lower complexity.

`runx.skill.apply` fail-closes on objective facts: bounded paths, secret-like
material, package parsing, exact-candidate inspection, isolated harness replay,
target drift, and transactional application. Architectural judgment remains in
the `skill-build` operating contract and review; native code does not pretend to
validate it by counting model-authored prose fields.

## Catalog and review policy

Capability metadata describes the complete public runner surface of a package,
not only its default runner. `execution` and `completion` state the strongest
effect the package can truthfully close, and `approval` reflects the strictest
human gate required by those runners. The default runner remains the concise
entry path and is reported separately in catalog reviews.

Registry ownership is provenance, not a maintainer convenience. A public
package accepted from a contributor preserves that publisher namespace in
`run.yaml` as `catalog.owner`; packages without the field belong to the
first-party `runx` namespace. The field guides repository release
tooling but grants no publish authority. Moving a package between namespaces
requires an explicit ownership transfer, never an automatic rewrite during
merge or hardening.

Registry admission executes the exact digest-bound package that will install.
Local cross-package edges are carried in that package bundle; a harness may
not borrow sibling files from the source checkout that are absent from the
published artifact.

The catalog and package gates block structurally invalid or unusable packages:

- unresolved or cyclic default closures;
- agent-authored work with no declared artifact packet;
- no cold-selection proof against at least two real nearby public skills;
- no standalone journey through the actual advertised default runner;
- no composed journey that names reused evidence and work that must not repeat;
- a missing product archetype review.

The native semantic report owns the operator-readiness facts. The native
execution closure separately owns resolved package, runner, tool, and edge
truth; reviewers compare capability metadata with that closure rather than
reimplementing effect classification in TypeScript or package code. Provider
evidence remains graded: deterministic replay is `harness`, live-provider
evidence is `live`, and supplied agent answers never upgrade either. Harness execution strips ambient Runx grants,
tokens, credential deliveries, and global configuration before applying the
fixture's explicit fake bindings.

Metadata, live provider readback, forward tests, and evidence-depth gaps remain
visible improvement findings. They may prevent a skill from meeting the full
archetype bar without pretending the underlying product capability should be
deleted.

The dated [Core Skill Product Review](core-skill-review.md) records the human
product decision and evidence available at that review. Current structural
truth comes from native package validation, the official lock, operator-context
expansion, and package harnesses; no parallel review generator is authoritative.
The review does not authorize removal, relocation, or demotion.

Internal packages use two distinct review categories. `internal_fixture`
packages provide deterministic test rails for canonical public skills;
`internal_runtime` packages implement provider-specific execution paths. Both
are evaluated through their parent integration contracts, remain non-public,
and are not exempt from replay, refusal, recovery, or evidence requirements.
