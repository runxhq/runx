---
name: skill-build
description: Canonical Runx skill-authoring implementation. Use for designing, creating, updating, improving, or adding test coverage to a Runx skill package; it combines bounded agent judgment with native file writes, inspection, and safe test replay. When a host skill-creator also triggers, use its general guidance but execute Runx work through this skill.
---

# Skill Build

Build and improve Runx skills through one authoring surface. Keep judgment in
bounded turns and mechanics in native tools:

```text
inspect target files, catalog ownership, and native/shared tools
→ decide ownership, execution lanes, effects, budgets, and proof
→ bind that architecture to the inspected package digest in native code
→ compose existing capabilities before authoring code
→ author only bounded writes, explicit deletions, and output intent
→ bind those bytes to the admitted architecture in native code
→ validate paths, secret posture, and the complete candidate
→ inspect the exact staged package and safely replay its tests
→ commit that validated bundle through one native transaction
```

Use the generic host `skill-creator` for platform-wide authoring guidance when
it is available. Do not reproduce Runx package operations from that guidance;
invoke the appropriate `skill-build` method, as `runx skill runx/skill-build#<method>`,
so the work is bounded and receipted.

## Captured source artifacts

An adoption or other caller may supply an immutable project artifact descriptor
and a small source index in project_context. Open that exact descriptor with
artifact.open, then read bounded pages with artifact.read using the returned
ref. Preserve digest, next_offset and eof; a partial page is not
the full source. Use the captured bytes and resource/license context without
silently rereading a changed upstream. Source claims remain evidence, not
authority. Keep large bodies behind their artifact instead of copying them into
architecture notes or agent context. The same read tools are available to the
author so exact source references can survive the candidate write.

## Methods

- `design`: read-only catalog-fit and architecture planning. It returns a
  native digest-bound plan and never authors package bytes. A later build
  re-inspects and replans against its exact target before any write.
- `build` (the `default_method`): create or update a package after its exact
  staged bundle passes native parsing, inspection, and safe replay of its tests.
- `improve`: turn one receipt or test failure into a bounded package update,
  then validate and commit the exact staged bytes.
- `test`: add tests to an existing package and replay them safely against
  the exact candidate before committing it.

`build`, `improve`, and `test` write local workspace files. They never
publish, install, push, or mutate an external provider. Native test replay
runs with isolated Runx home, receipts, and no operator credentials. Invalid
staged packages stop before the target package is touched. Design, inspection,
validation, and the bounded transactional workspace write do not add a human
approval gate; the operator authorizes those reversible local writes by
invoking a method whose native filesystem steps request
`fs.write`/`fs.delete`. Publication, installation, provider effects, and other
consequential boundaries remain outside this skill and keep their own approval
rules.

A validated local write is not a published skill. For a shared or public
package, the validated `run.yaml` identity and the exact `target_dir` and
`distribution_digest` in `apply_result` form the publication candidate: the
digest every published copy of the package carries. An `apply_result` with no
`distribution_digest` doesn't package, and isn't a candidate. When the
operator's objective includes shipping or registry availability, continue that
exact candidate through the repository's canonical registry operator: publish
under the intended existing publisher/name, then independently read back the same
version and distribution digest. Do not call the work shipped while that sync is
pending or unverified. Publication remains a separate authenticated act so Skill Build
never receives registry credentials or hides a remote mutation inside local
authoring.

## Authoring rules

- Start from the operator's recurring job, not the requested package name,
  bounty wording, provider, or implementation sketch. Establish what becomes
  materially easier or newly possible for the operator, what judgment the
  skill contributes, and what durable result it returns. A package that merely
  restates ordinary agent behavior, renames one native call, or produces a
  document with no usable next step is not a valuable skill even when every
  contract and test passes.
- Treat prior implementations, accepted bounty text, issue suggestions, and
  named integrations as evidence about the intended capability, not binding
  architecture. Preserve the real user outcome and compatibility obligations;
  replace stale ownership, unnecessary providers, and accidental workflow
  shapes when the current catalog has a cleaner canonical route.
- Treat `SKILL.md` as the product manual for both the human operator and the
  operating agent. A person opening it cold must understand what the capability
  does, why and when to use it, what happens end to end, and where it stops.
  Do not reduce that manual to terse model directives, field lists, or a task
  contract.
- Preserve the useful context in an existing skill: its mental model, procedure,
  examples, trade-offs, chain relationships, evidence rules, and recovery
  posture. Rewrite statements that no longer match the implementation; never
  delete the surrounding explanation merely because the executable profile now
  enforces part of it.
- A complete public skill explains, in a structure natural to the capability:
  the recurring job and outcome; when and when not to use it; the operating
  model and sequence; upstream and downstream skill relationships; meaningful
  input and output semantics; authority, approval, evidence, finality, and
  recovery; and relevant edge cases and stop conditions. Include a concrete
  example when it materially clarifies a non-obvious workflow. Do not force
  ceremonial sections onto a simple facade or internal rail.
- Put task-specific agent clauses after the human-readable operating guide.
  They sharpen individual turns; they do not replace the guide or carry
  the product's voice by themselves.
- Explain meaningful upstream and downstream skill relationships naturally in
  the operator guide. Do not maintain a second machine-readable dependency
  registry in `SKILL.md`: native closure inspection owns the exact
  edge set and operator preflight surfaces it. Prose explains why the chain
  exists; the runtime proves what it actually calls.
- Design capability chains by responsibility, not by whichever integration is
  easiest to name:
  - The domain skill owns domain evidence, interpretation, policy, and the
    truthful domain result.
  - A canonical capability or authority skill owns a reusable decision such as
    authorizing a send, spending funds, or preparing a release.
  - A connector owns the bounded external API effect and independent
    readback.
  Do not collapse these roles into one package, and do not make a provider's
  connector a required part of a provider-neutral capability. Pin a connector
  only when the objective genuinely requires provider-specific behavior; otherwise
  return a stable handoff that an operator can route to the selected connector.
  For example, a meeting skill may produce grounded task proposals and a
  follow-up message; communication then goes through `message-send`, while task
  creation goes through the operator's selected task adapter. n8n, Zapier, or
  any other automation provider is optional unless the requested capability is
  explicitly about that provider.
- Search the catalog by operator outcome and authority owner, not only by words
  present in the request. Consider every material result independently: a
  workflow may need different next lanes for a message, task proposal, payment,
  publication, or unresolved ambiguity. Reuse the canonical capability at each
  boundary and explain why it owns that decision. Do not hide an unhandled
  outcome behind a generic "handoff" claim.
- Preserve the context that makes the next operator or skill intelligent.
  Handoffs carry the bounded evidence, decision rationale, unresolved
  ambiguity, stable content or proposal, intended audience or target, and
  effect status needed by the next lane. Do not squash these into a digest,
  terse status, or provider payload. Digests bind context; they do not replace
  it. Conversely, do not forward unrelated source material or secrets.
- Make finality explicit at every boundary. Distinguish observed evidence,
  model interpretation, draft, proposal, authorized action, attempted effect,
  provider-confirmed effect, and independently read-back effect. A downstream
  route is not proof it ran; a sealed plan is not approval; an adapter request
  is not delivery. The public result and operator guide must say what actually
  happened and what still has to happen.
- A skill declares domain procedure and policy. Runx owns generic input,
  packet, evidence, approval, request, credential, effect, and receipt mechanics.
- Place the capability in its real owner before choosing an implementation.
  Reusable skills, end-user and domain-operator commands and UX, local host
  loops, local queues, and default local-state orchestration belong in Runx OSS
  or the owning product repository. `runx/cloud` is not precedent for those
  concerns: it may provide the hosted control plane, custody provider
  credentials, resolve authoritative grants, and execute a fixed bounded
  provider operation. Using Hosted Connect does not move the surrounding skill,
  procedure, operator decision, or state into Cloud. If that operator surface is
  missing, return work to OSS or the product owner; never extend a Cloud script
  or hosted service as a substitute.
- For provider work, call each connector operation from an `operation` step
  that names it as `<connector>/<operation>`; the connector declares the scopes,
  target and readbacks. The package's connector requirements are derived from
  the steps that call them, so `connectors:` says only what a step can't: a
  `manifest_digest` pin, or a connector chosen per run with `choose` and `from`,
  which steps then name as `<name>/<operation>`. A connector no step calls is
  refused. Connector requirements do not grant direct agent calls. When an
  agent step genuinely chooses provider calls dynamically, list the exact
  operations in that step's `calls.operations`, each as `<connector>/<operation>`.
  Omitted or empty `calls.operations` grants no direct provider calls;
  `operation` steps keep their declared behavior. Do not give a supplied-data
  judgment step acquisition or mutation tools merely because another step
  needs them. A scoped grant is sufficient by default.
  When an act genuinely needs a human decision, give the operation step one
  `approval: { reason, type }` with a concrete reason and action-specific
  type, or rely on the operation's `approval: required`. The kernel constructs
  the exact operator summary, binds the request to the intent, and suspends for
  `runx resume`; never add an adjacent `ask` step for the same action. Use an
  `ask` step only for a consequential decision no native capability owns. One
  action has one approval owner. The money a call moves is derived from its
  operation's inputs, which the connector declares, so a step never states an
  amount. Use the step's `result.equals` to bind the returned resource identity
  and `result.required` and `result.optional` to admit only the fields the
  receipt needs. Secret-adjacent operations must project their result. Give
  every write an `idempotency_key` on the step, which names one intent; do not
  copy it into the provider payload. Never add a package token loader or
  request client.
- Never model human authority as a caller-supplied approval string, boolean, or
  reference. A native approval gate must summarize the exact target and effect;
  its host-attested packet is the only approval input a credentialed provider
  tool may accept. If a provider API requires its own approval reference, derive
  the claim- or resource-bound value inside that tool after verifying
  `approved: true`, `actor: human`, the exact gate id, and the action-specific
  gate type. An agent-authored `answers` value must fail before provider
  execution.
- An `ask` step is a human gate. Agent provenance is rejected by the runtime;
  use an `agent` step for model judgment and keep its result distinct from
  human authority.
- Treat the selected method's typed execution requirements as the complete
  permission request. Put exact non-secret environment names and runtime
  metadata on their canonical step and tool fields, never under `runx`, in
  prose, or in a package loader; credentials belong to the connectors the
  method calls, and a catalog tool's scopes come from the tool. Runx resolves
  and records those declarations without inventing a scope vocabulary; the
  native capability or provider that owns a scope enforces it.
- Make every public method contract recursively complete at the authoring
  boundary. `run.yaml` owns nested properties, closed objects, enums, bounds,
  and complete parser-validated invocation `examples`; `runx skill inspect`,
  MCP, agent tools, and generated exports must project that same declaration.
  When an identical input declaration is used by several methods, define it
  once under `shared_inputs` and have each method take it by name, as
  `repository: {}`; a key written beside it overrides only that key, so never
  copy the whole declaration. A rule across fields that no single field can
  state belongs in the method's `input_constraints`. JavaScript may enforce
  irreducible relationships between otherwise valid fields, but must not
  repeat structural validation already owned by the profile.
- Dogfood the result through the real agent-facing surface, not only the
  package's tests. A cold agent must be able to inspect the selected method,
  construct a valid call without reading source or fixtures, understand a
  path-specific rejection, and continue from the resulting context. Exercise
  every materially different method; a default-only export cannot certify a
  multi-method skill. For an existing target, use
  `authoring_context.target_inspection` as the canonical method-contract and
  closure evidence. An `invalid` inspection is repair context, not
  permission to guess: read the declared target files, correct the owning
  contract, and validate the complete candidate.
- Design evidence admission around the operator's job, not the first provider
  implementation. When analysis is useful over local admitted files or
  artifacts as well as remote sources, support both through their canonical
  evidence boundaries; do not require HTTPS, Hosted Connect, or another
  provider merely to analyze evidence already available on the operator's
  machine. Provider-specific acquisition remains a separate method or upstream
  skill when it adds real readback value.
- Make permission claims executable through the package's own tests.
  Permission-bearing native and provider paths need a realistic admitted test
  and, where a test can exercise that owner, a refusal test with the
  required scope or grant withheld. A test must call the same catalog,
  dispatcher, and capability/provider implementation as a real run; never add
  a mock permission evaluator, scope vocabulary, or parallel verification
  report. This proves enforcement owned by Runx capabilities and providers. It
  cannot prove that trusted host code avoided undeclared filesystem, network,
  or syscall access, so report that boundary as trusted rather than confined.
- A test's tool steps inherit the exact scopes declared by the canonically
  resolved manifest and pass declared packets through the production packet
  verifier. An output with a `packet` is already verified from the declaration,
  so do not assert it again through a second path in the test.
- Search the inspected native-tool and skill catalogs before designing files.
  Prefer an existing core tool or canonical skill over executable package code.
- Make every public method input constructible from inspection. A field is
  its own JSON Schema, so write a method-owned nested value's properties on the
  field itself. A field is required unless it says `optional: true`; JSON
  Schema's `required` keeps its one meaning, the properties an object must
  have. When an input takes an artifact of exactly one canonical Runx packet,
  name its `packet: <packet-id>` and let the runtime resolve the catalog-owned
  schema into inspection, validation, exports, registry bundles, and tests.
  Never copy that packet schema into the consumer. If the canonical schema is too weak to make the input usable,
  improve the producing packet contract first; a packet reference is not a
  substitute for a complete schema. Do not mint a packet id for a method-local
  nested value, workflow intermediate, or one-run implementation detail. A packet
  exists only for the complete value crossing a named reusable skill, runtime,
  SDK, provider, receipt, or registry boundary, and it must have one active
  producer and consumer or an explicit public native owner. Remove the generated
  artifact when that ownership disappears. Every public method still needs at
  least one realistic, copy-valid example unless its inputs are empty.
- A named packet must expose its semantic fields or reference a canonical typed
  contract. Bare `type: object`, unconstrained `{}`, and opaque property bags do
  not make a method inspectable and must be repaired at the producing contract,
  not documented around in prose. Open JSON is admissible only as an explicitly
  named protocol extension or generic data payload inside an otherwise bounded
  semantic envelope.
- Apply that rule recursively. Every nested `type: object` declares meaningful
  properties or an explicit `additionalProperties` policy. If the value is
  intentionally arbitrary JSON, leave out `type`, since a field with no type
  takes any value; do not make an object declaration imply structure that the
  contract does not provide.
- Put a producer's nested output shape on that output's own declaration. Do
  not hand-maintain the same shape in a packet file, test, exporter, or
  consumer: native parsing, agent context, runtime validation, packet
  generation, registry packaging, and test replay all consume the producer's
  declaration.
- Express orchestration through `run.yaml`; keep all static agent operating
  knowledge and task contracts in `SKILL.md`. Never put model instructions in
  manifests, fixtures, or duplicated prompt fragments.
- Each test is one `tests/<name>.yaml`: its `name`, the `method` it runs, its
  `inputs`, what answers the run under `host` (agent `answers` by request id,
  `turns` worked call by call, `approvals`), exactly what it reaches under
  `providers` (`http`, `mcp`, `processes`, `hosted`), and its `expect`. Data a
  test reads lives under `fixtures/`, which Runx stages whole into the isolated
  test workspace, with `fixtures/bin/` executable; a test that composes skills
  uses a small fixture package there, named by the test's `package`. Do not
  turn `fixtures/` into a second source tree or include unconsumed helpers.
- Put every typed output and packet contract on the step that actually produces
  it: an output with a `packet` is kept as an artifact of that packet, and
  `result_as` names the output the whole result becomes. A composed method is
  composition and its receipt proves that composition; it must not restate its
  steps' outputs in a second method-level `outputs` contract with ambiguous
  ownership. Every composed method declares `result_from` as the intentional
  public result boundary. Name the final provider readback or
  package/finalize step, not every leaf: approvals, evidence gathering, and
  intermediate writes remain available in operator context and receipts without
  polluting the result. Multiple producers are valid only for mutually exclusive
  branches or when their distinct contracts are intentionally returned together;
  simultaneous producers may not emit the same key. When a workflow needs one
  public result, end it with an explicit package/finalize step and let that
  producer own the packet schema.
- Add executable code only for irreducible deterministic domain computation.
  Explain its domain boundary and why native tools plus a declarative workflow
  cannot express it. Do not add code merely to transform Runx contracts.
- For a genuinely separate CLI or protocol tool, keep one canonical
  `manifest.json`. It owns source, inputs and defaults, artifact projection,
  scopes and retry/idempotency. Effect and approval stay with the capability
  that invokes the tool; never duplicate them in this manifest. Never persist
  generated `runtime`, `output`, `runx`, hash, or toolkit fields beside that
  contract. Loading the package validates it and binds its source files'
  digests without rewriting it. The declared entrypoint must execute on the
  repository's supported runtime without probing for generated files or
  importing uncompiled TypeScript. A bundled tool lives at
  `tools/<namespace>/<name>/manifest.json`, its dotted manifest name must match
  that path, and aggregate package admission must bind every static local
  source dependency before the package can run or publish.
- Local CLI tools, process MCP servers, and process adapters are trusted host
  code. Runx controls their exact invocation, delivered environment and
  credentials, lifecycle, bounded output, and evidence; it does not turn
  declared scopes into portable filesystem, network, or syscall confinement.
  Use this lane only when the operation is genuinely irreducible to native
  capabilities, connector operations, declarative composition, or deterministic
  JavaScript. Never author a sandbox declaration, wrapper, or fallback flag.
- When that computation is JavaScript, use a worker `tool: { module: ... }`
  step. Prefer one cohesive module named for the skill with focused named
  exports of the form `(inputs, context) => JSON`; split it only when the
  computations have genuinely separate ownership. Runx owns input delivery,
  output serialization, errors, CPU limits, and isolation. Do not add fake
  operation inputs, Node command declarations, per-method wrapper files, or
  stdout/environment plumbing. Pure JavaScript receives only its validated
  in-memory module bundle, JSON input, and a frozen
  `context.environment` object containing the exact names declared in its
  tool's `environment.required` and `environment.optional` lists. A missing
  required name stops before worker execution; an absent optional name is
  omitted. Values never enter the manifest, agent context, inspection output,
  or receipts. Environment declarations are for non-secret runtime
  configuration; credentials stay on the native credential/provider boundary.
  The worker process itself has an empty ambient environment and no workspace
  path, filesystem, network, clock, randomness, subprocess, credential, or
  provider surface. An invocation may spend two seconds of CPU time by default;
  a worker tool may declare `cpu_seconds` from 1 through 30 when irreducible
  computation genuinely needs it. The worker is ECMAScript, not a browser: use the frozen
  `Runx.parseUrl(value)` helper for absolute URLs and do not assume Web or Node
  globals exist. Use `Runx.parseData(source, format)` for bounded JSON, YAML or
  TOML syntax decoding, with package-specific interpretation kept in the domain
  module. Declare `requires: [parseData]` on each worker tool that needs it.
  This author-declared guard makes unsupported runtimes reject the profile
  before effects; Runx does not infer helper calls or treat requirements as
  permissions. The baseline `parseUrl` helper predates this declaration.
  Never replace unavailable helpers with Node, shell or hand-written parsers.
- Classify volume before authoring. Small typed control values belong in normal
  method inputs; `runx skill --inputs` is only a bounded transport for one
  complete control object. Large immutable local content belongs behind
  `artifact.admit`/bounded pages. Durable history belongs behind
  the store's `events.read` cursors or a compact projection. A workflow must not carry an
  archive, growing event history, or completed-id array simply because one CLI
  call can parse it.
- Use a worker tool's deterministic `paged_input` only for irreducible record
  transforms over one admitted JSON-array artifact. Paged input reads under
  `fs.read`, and native admission preserves caller bounds: agent-bound runs refuse secret files and
  Runx state, including symlink aliases. The runtime owns artifact admission, page
  framing, snapshot digest, record boundaries, offsets, retries, and the page
  loop; the module owns only
  decoding and domain selection. Treat `runx_page.ref` as an opaque,
  logical-run-scoped reference that belongs in receipt provenance; never place
  it in a plan or idempotency digest. Bind deterministic domain output to
  `runx_page.digest`, which is stable for identical content. Keep
  continuation state proportional to the bounded result. For a retained provider
  array, use native `artifact.store` and `artifact.open`, then set
  `paged_input.artifact` to the input carrying `artifact.ref`. This
  reuses an admitted snapshot; never reconstruct its storage path. The opaque URI
  survives process exit and pause/resume within the same logical run. A different
  run or project refuses it; reopen the retained project artifact descriptor when
  crossing that boundary. `artifact` cannot also
  declare `path` or `media_type`, and still requires `fs.read`; native
  `artifact.open` keeps its separate operator and artifact scope checks. Do not add a package
  file reader, manual byte cursor, hashing loop, high-volume profile, or raised
  worker limit. If safe framing or bounded state is impossible, choose
  `needs_core` or a genuinely separate protocol tool rather than
  smuggling filesystem authority into JavaScript.
- Prove a volume path at two materially different scales through the production
  owner. The result must be identical across page sizes, cursors must advance,
  process count must stay stable where session reuse applies, and failures must
  remain distinguishable from empty pages. A larger fixture alone is not
  performance evidence.
- A missing generic primitive is not permission for package code. Return
  `needs_core` with no writes and identify either a runtime/security invariant
  or two independent existing consumers.
- Never add package-local raw `RUNX_INPUTS_*` parsing, generic packet or
  evidence hashing, packet wrapping, generic status construction, or provider
  simulation when a shared Runx boundary can own it. Package code may retain a
  canonical hash only when that hash is an intrinsic field of an established
  domain or wire protocol, not as a substitute for receipt or effect integrity.
  State that exception at the computation boundary.
- Keep packages concise: normally `SKILL.md`, `run.yaml`, focused `tests/`,
  and `fixtures/` only for data those tests read; add narrowly scoped
  references, assets, tools, or domain code only when consumed.
- Judge the whole capability, not proxy metrics. Native reuse, fewer files,
  shorter code, passing tests, and low resource ceilings are valuable only
  when the result remains understandable, useful, truthful, and complete.
  Keep JavaScript when it performs irreducible domain computation; remove it
  when it reimplements runtime mechanics. Never trade away domain semantics or
  operator context merely to reduce line count.
- Keep shared computation DRY. A helper used by multiple skills belongs in its
  existing native owner or a justified shared primitive, never copied scripts.
- Count the whole replacement and delete displaced scripts, manifests, schemas,
  fixtures, and tests in the same change. Do not leave dual paths.
- Do not add package READMEs, changelogs, installation guides, strategy files,
  generated state, or credentials.
- Match the documented capability to the execution profile and truthful terminal
  state.
- Review the documentation diff for semantic loss. A shorter `SKILL.md` is an
  improvement only when the removed material was false, duplicated, or
  irrelevant and the remaining document still passes the cold-operator test.
- Treat the catalog's manual check as an anti-stub backstop, not a writing
  target. The structural title/section/prose floor does not prove a guide is
  substantive. Do not pad prose to satisfy a word floor or turn natural
  operating guidance into a template checklist.
- Prefer extending an existing owner over adding a near-duplicate skill.
- Preserve registry identity as part of capability ownership. Inspect the live
  registry before publishing a shared package; update the intended existing
  publisher/name rather than creating a first-party neighbor beside an older name.
  Preserve an accepted contributor's namespace at publication: the publisher
  is the namespace the registry signs the package under, never a `run.yaml`
  field.
  A deliberate rename must migrate or retire the prior identity through the
  registry operator. Never transfer a contributor's community-owned row into
  the first-party namespace unless that ownership transfer is explicit.
- Treat registry portability as execution truth, not a publish-time smoke test.
  A local cross-package skill edge must be present in the digest-bound registry
  package bundle, and the tests a publish runs must execute that exact
  materialized package. Never copy sibling source beside a test while omitting it from the
  published artifact, rewrite the source workflow for registry-only execution, or
  certify a package that needs the original checkout to run.
- Include a realistic happy path and refusal, stop, or error path.
- Never treat supplied agent answers as provider-effect proof.
- For every public package, a sealed test of its default method is the
  acceptance boundary: a method is offered on its own, exported or listed over
  MCP, only when one of its tests seals, and a test on a convenient phase
  method cannot prove the default. Write the package's evaluations in
  `evals/evals.json`, the Agent Skills evaluation file, naming the package: a
  realistic direct request with the outcome it must reach, and for composed
  use, the prior evidence it reuses and the work or effects it must not
  repeat. Selection queries, including nearby public skills a request must not
  select, belong in `eval_queries.json`. Evaluations are not a format
  requirement and decide nothing at export, so never recreate their judgment
  in package code, prose, or a second test script.
- Keep provider evidence labels exact. Deterministic replay is `test`, not
  `live`. A sealed provider-readback test supplied through host agent answers
  does not prove provider execution. Tests must not inherit local Runx
  grants, tokens, credential delivery, or global configuration; every fake
  binding must be explicit in the test, and live proof must come from a
  separately identified provider run.

Before admitting an architecture, perform a cold-operator trial against the
actual proposed result:

1. Can the operator tell what the skill concluded and which evidence supports
   it?
2. Can they distinguish missing or ambiguous information from a negative
   result?
3. Can they tell which effects happened, which are only proposed or
   authorized, and which remain unattempted?
4. Can they continue each material outcome through the correct canonical skill
   and the declared provider-selection boundary without reconstructing lost
   context?
5. Does one clear owner hold each decision, approval, mutation, and readback?
6. Would this still be the architecture chosen from scratch against the current
   Runx catalog?
7. Can a cold agent actually invoke every public method from inspection alone,
   using local or remote evidence appropriate to the job, without source-diving,
   guessed JSON, or an unnecessary provider dependency?

If any answer is no, the package is not ready even if its schemas, tests, and
budgets pass. Fix the capability design or return `no_skill`/`needs_core`;
do not compensate with more fixtures, prose padding, or a bespoke wrapper.

## Outputs

- `architecture_decision`: the agent's closed ownership and execution design.
- `architecture_plan`: that decision bound by native code to the exact
  inspection digest. Design stops here.
- `change_draft`: package bytes and intent authored against the admitted plan;
  it contains no model-authored integrity values.
- `change_bundle`: the native, digest-bound transaction candidate produced from
  the plan and draft.
- `apply_result`: unchanged, needs-core, or validated-and-applied, with exact
  changed/deleted paths, package digest, focused proof, and line/file deltas.
  For a shared/public package, this is the exact local half of the registry
  publication handoff; registry publish and readback evidence remain separate.

## Inputs

- `objective` (required): capability or improvement to deliver.
- `package_name` (optional, build): explicit identity for a newly requested
  package; use it as `SKILL.md`'s `name` even when the target directory has a
  different basename.
- `repo_root` (optional): workspace root; defaults to the caller workspace.
- `target_dir` (required for `build`, `improve`, and `test`): repo-relative package directory.
- `project_context` (optional): product, repository, and operator constraints.
- `receipt_id`, `receipt_summary`, `test_output`, `failure_packet` (improve):
  failure evidence, including the stable packet from `diagnose-skill-run`.
  `test_output` carries a test run's sanitized output.

## Agent task: skill-build-architecture

Call `turn_complete` with exactly one root field,
`{"architecture_decision": <decision>}`. The inner decision uses
`runx.skill.architecture_decision.v1`. Choose `build`, `extend_existing`,
`no_change`, `no_skill`, or `needs_core`. Explain the operator value and the manual's
knowledge contract: purpose, required evidence, decision logic, stop conditions,
and recovery. Assign every required behavior to exactly one real execution lane
(`manual`, `workflow`, `agent`, `native_capability`, `domain_module`,
`process`, or `operation`). A native lane names a selected capability;
a domain module supplies a specific justification. Record inspected, selected,
and genuinely missing native capabilities; use `needs_core` only for a runtime
or security invariant or a primitive with two independent existing consumers.
Return the architecture decision once the evidence is sufficient. The workflow's
native `plan` step validates it and binds it to the inspected package digest.
For every `native_capability` behavior, `reuses` must be exactly one entry from
`native_reuse.selected_capabilities`; split a behavior when it needs more than
one capability.
When `package_name` is supplied, treat it as the requested package identity;
do not silently rename the package from its target path.

Inspect an existing target in one bounded pass. Use the target file list and
repository root in `authoring_context` to read the relevant files together with
`fs.read_bundle`; do not crawl them one at a time or reread files already
present in context. Once the evidence is sufficient, return the declared
architecture decision instead of spending the remaining round budget on
optional discovery.

For every `build` or `extend_existing` decision, make the identity decision
explicit. Record the current and proposed name, choose `keep`, `rename`,
`create`, or `internalize`, state whether the package is public or internal,
and explain why that name is the operation a cold agent will reach for. A
rename or internalization is a clean cutover: identify every consumer and do
not plan aliases or duplicate discovery rows. Internal packages have no public
direct-use contract.

For every public package, declare both operator contexts before authoring:

- `direct_use` names realistic natural-language trigger and non-trigger
  requests, the default outcome, routine work the host should do normally, the
  exact boundary where Runx adds value, the terminal result, one actionable
  blocker behavior, and the context-preserving native escape path.
- `chain_use` names accepted typed inputs, the reusable result, prior evidence
  and effects that survive the boundary, and work that must not repeat.

The proof plan must contain distinct `trigger_eval`, `direct_use_eval`, and
`chain_use_eval` entries for a public package. They are different claims:
intuitive selection, useful direct completion, and evidence-preserving
composition. A generic test or sealed receipt may supplement them but cannot
replace them. The trigger eval's queries include requests for at least two real
nearby public skills, which must not trigger this one; the direct-use eval runs
the actual default; and the chain-use eval names both reused prior evidence and
work that must not repeat. The final candidate proves the direct-use eval with a
sealed test of its default method, carries both evals in its `evals/evals.json`,
and carries the trigger eval's queries in its `eval_queries.json`.

Declare effects, authority scopes, approval meaning, provider boundary, skill
routes, resource ceilings, preservation obligations, exact intended deletions,
and a proof plan. For every skill route, state the domain result being handed
off, the canonical capability or authority owner, whether a connector is
required or operator-selected, the context that must survive the boundary, and
the truthful effect state before and after that route. Cover every material
outcome independently. Reject an unnecessary provider pin even when it appears
in prior work or the request's implementation sketch. Apply the cold-operator
trial to the proposed public result and chain before making the architecture choice.
Classify every potentially large value as control input,
immutable artifact, durable cursor/projection, or bounded domain result, and
name its production owner plus small/large proof. Reads, drafts, local
validation, reversible package writes, and routine grant-scoped provider
mutations do not gain ceremonial human approval. Add an exact resumable gate
only when the action genuinely requires a separate human decision. Assign each
such action exactly one approval owner; never pair
an explicit workflow approval with an effect-owned approval for the same action.
Budgets are operational ceilings, not guesses to be widened after validation.
Do not write files, calculate a digest, invent provider proof, or solve an
ownership gap with package code.

For a shared/public package, also preserve its intended registry identity and
state whether registry availability is in scope. If it is, the plan ends local
authoring at the exact applied package and names the canonical registry
operator as the separate publish-and-readback boundary. Do not plan embedded
registry HTTP, credentials, auto-publish, or an adjacent publisher/name.

For `improve`, diagnose only from supplied receipt or test evidence and
distinguish contract, implementation, fixture, environment, and operator
failures. When evidence cannot justify a change, choose `no_change`. Do not invent a
package identity, direct-use contract or chain-use contract for that stop. The
workflow binds the authoring mode and derives an empty change bundle natively;
there is no author turn for a terminal decision. Preserve the missing evidence
and next step in the decision. For `test`, plan test-only changes, new
`tests/<name>.yaml` and the `fixtures/` data they read, and preserve all
production behavior.

## Agent task: skill-build-author

Receive the exact native `architecture_plan` and return one `change_draft` using
`runx.skill.change_draft.v1`. Never copy or calculate `base_digest`,
`plan_digest`, architecture, or any other integrity field; native bind owns
those values. Choose `write`, `no_skill`, `no_change`, or `needs_core` in a way
that agrees with the plan. A non-write draft has empty `writes` and `deletes`.
A write contains the smallest complete target-relative file set, the exact
planned deletions, a truthful summary and non-goals, and the outputs the package
will actually produce.

When source bytes are needed, read the planned target files together with
`fs.read_bundle`. Do not spend separate rounds reopening files already supplied
by `authoring_context` or the architecture plan; reserve the final round for the
complete `change_draft`.

The package's name is its `SKILL.md` frontmatter `name`; `run.yaml` has no
name of its own. When `package_name` is supplied, use it there. The directory
is a placement decision, not a second source of package identity.

Write `run.yaml` in the shape the parser reads, which refuses every key it
doesn't. Its root says `version`; `minimum_runx`, required, the oldest runx
that can run the package, `0.12.0` today; `catalog`; `shared_inputs`;
`connectors`, only for a pin or a connector chosen per run; `default_method`,
only when there is more than one method; and `methods`. A method has its
`description`, `inputs`, `outputs`, `examples` and `steps`, and may add
`input_constraints`, `result_from`, `effects`, `reach` and `offer`. A method is
always its steps, and one agent turn is one `agent` step. Every step has an
`id` and exactly one kind: `agent`, `tool`, `operation`, `ask` or `skill`, and
may add `when`, `guard`, `inputs`, `outputs`, `result_as`, `effects` and
`reach`. An `agent` step names its task, and `SKILL.md` must hold that task's
`## Agent task: <name>` section with its instructions; its `calls` lists what
the turn may call (`tools`, `operations` as `<connector>/<operation>`, `skills`
as `<ref>#<method>`, `load`, `ask: true`) and its `context.skills` the
skills it reads first. A `skill` step names its target as `<ref>#<method>`,
where `.#<method>` is this package's own method. Steps read `$input`,
`$steps.<id>`, and `$run.act` inside an `act`.

A `catalog` block takes exact values, and native apply refuses any other:
`visibility` public or internal, `category` code, research, ops, growth,
content, data, security, authoring or payments, and `tags`.

Put static operating knowledge and task-specific agent rules in `SKILL.md` and
execution structure in `run.yaml`. Compose selected native capabilities and
declared skill routes first. Ensure the manual explains what each route is for,
what context crosses it, and what remains unperformed; do not substitute an
adapter name for that explanation. Add a domain module only when the architecture
admits it, and keep the module inside its stated computation boundary. A module
runs in one worker process, so its architecture budgets `max_process_spawns` of
at least 1. Include
focused proof for a useful path and a stop, refusal, or regression path, each a
`tests/<name>.yaml`. In `test` mode, write or delete only tests and the
`fixtures/` data they read. Preserve
useful behavior and delete superseded implementation in the same draft. Never
add auxiliary docs, generated state, credentials, placeholder modules,
duplicated generic Runx mechanics, or an undeclared provider boundary.
Registry publication code, credentials, and live-state claims are likewise
outside the authored package; preserve only the canonical package identity
needed by the later registry operator.
