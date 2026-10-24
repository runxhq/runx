# Runx OSS Conventions

## Scope

This file applies to the OSS workspace under `oss/`. It complements
`AGENTS.md`, `CLAUDE.md`, `docs/architecture/runx-system.md`, and
`docs/trusted-kernel-package-truth.md`.

## Contract Vocabulary

Governed runtime artifacts use the harness spine:

- `harness`: governed execution boundary with attenuated authority.
- `act`: contained payload with `intent`, `form`, and `closure`.
- `receipt`: sealed proof of a harness node.
- `decision`: accountable harness lifecycle choice.
- `signal`: world-before-action input.

Do not introduce compatibility aliases, `.v2` contract ids, or retired central
object names at governed boundaries. Product-facing skill names may remain
recognizable; wire contracts must use the spine vocabulary.

Each party has one name, in contracts, code, variables and prose alike:

- `publisher`: the registry namespace and organization that publishes a skill
  or connector. A skill is `<publisher>/<name>`.
- `seller`: the publisher's principal enrolled to sell its paid skills. A
  purchase names it as `seller_ref`.
- `settlement principal`: who takes a seller's money; the seller unless a
  deployment binds a treasury.
- `merchant`: a payment provider's account that receives a payment, such as a
  Stripe or PayPal account or an x402 `payTo`.
- `buyer`: the principal that pays for a purchase.
- `payee`: a party outside the registry that a buyer pays.
- `provider`: the service a connector talks to, such as GitHub or Stripe.
- `owner`: only the principal a private resource belongs to, such as a grant,
  a held purchase or a connector instance. Never the publisher.
- `purchase`: one paid skill run bought by a buyer from a seller, from quote to
  receipt. Other calls are invocations; a purchase is not.

Names follow these rules:

- `<thing>_ref` is a runx `Reference` (`{type, uri, ...}`) to another record,
  `<thing>_uri` a URI string, `<thing>_id` a bare identifier, `<thing>_path` a
  file path, and `<thing>_locator` an opaque locator a provider or a turn
  gives. A selector the caller writes is named for the thing it selects:
  `skill`, `tool`, `connector`, or `package` for a versioned registry package
  such as `<publisher>/<name>@<version>`. Relation names (`previous`,
  `children`, `supersedes`) hold References under the relation's own name, and
  a provider's own field word (a git `ref`) stays at its boundary.
- A content digest is a `digest`, or `<thing>_digest`, written
  `sha256:<hex>`; never a hash or a fingerprint.
- A package or offer label is a `version`, never a revision.
- `state` is where something is in a lifecycle that moves; `status` is how a
  finished check or response came out; `outcome` is how a run or receipt
  ended: `completed`, `deferred`, `superseded`, `declined`, `blocked`,
  `failed`, `killed` or `timed_out`.
- A field that holds a project artifact is `<thing>_artifact`; a list of them
  is `artifacts`.
- A run is a run and an act is an act. `execution` names a boundary, a
  closure or a location; the program a binding names is its `executor`.

`runx.receipt.v1` is the wire runx 0.11 signs, and its issuer names the
`runx_version` that signed it. A receipt issued before 0.11 verifies with the
runx release that issued it.

## Package Boundaries

Package names carry trust claims:

- contracts define portable schemas and generated validators.
- Rust `runx-core` owns pure state-machine and policy decisions.
- Deleted TypeScript core packages must not be restored as compatibility shims
  or build-only fallbacks.
- `runx-runtime` coordinates local execution, adapters, exact process
  invocation and supervision, caller interaction, and receipts.
- host adapters and protocol adapters touch external processes and protocols.
- `runx-cli` is the native command shell over the runtime.

OSS packages must not import cloud code. Core must not import runtime, adapter,
CLI, host-adapter, filesystem, network, or subprocess concerns.

The public TypeScript invocation path is integrations → `@runxhq/client` →
`@runxhq/contracts`. The client root owns only finite native command invocation;
its `./adapters` export owns portable response formatting and must remain Node-free.
The client never imports private Cloud features, framework harnesses or CLI
internals. Products own persistence, agent loops and scheduling.

## Connector and Tenant Neutrality

Portable Runx behavior targets provider capabilities, not connector vendors or
Runx-company tenancy. Skills and public contracts must not contain a credential
backend's connection ids, provider-config keys, hosted tenant ids, connector
endpoints, or assumptions that credentials are held by Runx Cloud. The operator
binds an eligible local, self-hosted, third-party, or Runx-hosted connector at
runtime.

Provider operations therefore have stable names, typed inputs, bounded result
projections, target-resource constraints, access classification, idempotency,
and readback semantics independent of the adapter that executes them. Hosted
adapters are optional implementations of those contracts. They must never
become prerequisites for an otherwise portable skill.

## Ownership Before Abstraction

Promote behavior into native code only for a runtime/security invariant or two
independent existing consumers. Keep domain policy in its owning skill, delete
the displaced path in the same cutover, and never expose fixture or package
layout through a generic API. `skills/skill-build/SKILL.md` owns the full skill
authoring contract; `docs/skill-quality-standard.md` owns the review standard.

## Packet Artifact Eligibility

Every public runner must expose a complete inspectable input and output
contract. That does not make every runner or skill a global packet owner.

- Keep runner-specific nested values in `run.yaml` as ordinary JSON Schema.
- Declare a packet only when the complete value crosses a named, reusable
  skill, runtime, SDK, provider, receipt, or registry boundary.
- Anything with a packet id is a file. A package's packet is
  `<publisher>/<package>.<name>.v<N>`, in the package's
  `packets/<name>.v<N>.schema.json`, at the `$id`
  `https://schemas.runx.ai/packages/<publisher>/<package>/<name>/v<N>.json`.
  A step output bound to a packet declares only its type inline; the packet
  file is its contract.
- A core packet, `runx.<name>.v<N>`, is a Rust type the kernel emits or
  reads, or a platform protocol any producer speaks. It uses
  `public_packet_artifact`, is published as its `schemas/` document, and is
  never copied into a package. Existence in `runx-contracts` is not enough.
- Keep graph intermediates and one-run implementation values as typed step
  inputs or outputs; do not mint packet ids to make inspection work.
- The producer owns the packet. A reader names it by id and never copies its
  schema; publishing pins a copy of each packet a package reads from another
  package.
- A published packet id never changes bytes (`packets.lock.json`). Changing a
  packet adds a new version beside the old one.
- A named packet schema describes its semantic fields or references another
  canonical contract. Bare `type: object`, unconstrained `{}`, and an opaque
  `additionalProperties` bag are not substitutes for a contract. An open JSON
  value is valid only as an explicitly named protocol extension or generic data
  payload inside an otherwise bounded semantic envelope.
- This rule is recursive. Every `type: object` in an input or distributed
  packet must declare structure or an explicit `additionalProperties` policy.
  Use `type: json` when the value is intentionally arbitrary JSON rather than
  disguising that intent as an object contract.

Review the active producer and consumer before adding or retaining a packet
artifact. An orphaned schema is legacy code, not compatibility evidence.

## Rust Bar

Rust code must keep the workspace green under:

```sh
cargo fmt --manifest-path crates/Cargo.toml --all --check
cargo clippy --manifest-path crates/Cargo.toml --workspace --all-targets -- -D warnings
cargo test --manifest-path crates/Cargo.toml --workspace
cargo deny --manifest-path crates/Cargo.toml check bans licenses sources
```

Workspace lints deny unsafe code and common escape hatches such as unwrap,
expect, panic, todo, unimplemented, dbg, and print macros. Do not work around
these with broad allows.

## Tests

The default test for operator-visible behavior is an operator journey through
the outermost stable interface. For the CLI, spawn the native `runx` binary and
exercise the coherent lifecycle an operator actually follows: discover,
inspect, run or pause, resume when needed, seal, verify, and read history. Keep
human output, JSON output, exit codes, persisted state, and receipt evidence in
the same journey when they belong to the same promise.

Use focused unit or boundary tests when they prove a distinct invariant that a
journey should not enumerate: pure algorithms, parser diagnostics, security
rejections, wire compatibility, fault injection, and hard-to-reach platform
edges. Do not add a narrow test merely to repeat an assertion already owned by
a journey. When a journey absorbs existing coverage, delete the superseded
test only after the replacement proves the same behavior at a stronger
boundary.

A test proves current behavior. No test replays a migration, reads a
superseded shape, or asserts that something retired is gone. A rename moves
every test to the new names, and a test that only made sense under the old
names is deleted, never put back on them.

For skill behavior, the default is a replayable operator-story fixture through
the skill's public runner. Composite-skill harnesses own the business scenario
matrix: routing decisions, authority attenuation, approval and refusal stops,
downstream handoffs, provider-evidence requirements, receipt lineage, durable
readback, and replay or recovery. Use fixture agents and local or fake adapters;
live-provider smoke tests are separate and opt-in. CLI journeys prove that Runx
can drive a representative composite flow, but must not duplicate the skill's
full scenario matrix.

Native HTTP scenarios declare `caller.http_exchanges`. Each exchange binds a
response to the exact method, final URL, and explicit absent, structural JSON, or exact UTF-8 text body, so a
matched GET reads and a matched POST, PUT, PATCH, or DELETE proves a mutation
without reaching the network. Use `{text: "..."}` for an exact form, XML, or other text request body. Fixture URLs are canonicalized like native final
URLs and reject credentials and fragments, which are not valid exchange
identity. Any request no exchange matches is refused; a harness run never
falls back to the live network. Fixture bytes
still pass through the production admission, allowlist, redirect, extraction,
digest, redaction, and response-bound logic for the selected capability. Keep
live-provider availability checks in a separate opt-in smoke lane.

A sealed status is engine coverage, not operator-value proof. Every kept public
skill must have at least one semantic oracle through `expect.output`,
`expect.step_outputs`, or replayed `caller.answers`. Stateful workflows put the
whole transition and readback in one graph fixture. Individual fixtures must be
independent: never share hidden state through filename order or reuse a
single-use idempotency or capability key across cases.

Harness scratch state is project-owned under `.runx/harness` and is removed
after the run. Durable receipts remain under `.runx/receipts` or the explicit
receipt directory. Catalog sweeps and core-skill trials likewise isolate
disposable work under `.runx`; `/tmp` is reserved for an outer test framework
that deliberately supplies a disposable project root.

Rust integration modules remain consolidated into one test binary per crate.
Adding more files must not create additional Cargo integration binaries.

## Specs

Scafld specs are execution contracts, not notes. A spec that is stale against
the current harness spine or package truth must be repaired before approval or
build. Completed specs with failed, blocked, or not-run hardening need an
explicit follow-up or a recorded deviation before another spec treats them as
authoritative evidence.

## Fixtures

Fixtures are parity evidence. Do not regenerate fixtures merely to make a new
implementation pass. Preserve semantic meaning, review diffs, and add negative
fixtures when a contract rejects retired vocabulary or unsafe payloads.

## Financial and process results

A connector's bound financial amount is resolved from its admitted provider
arguments. Native admission uses that value for authority, the intent, approval
and run-budget reservation. A call may assert an amount, but it must equal the
bound value; omitting that redundant assertion never omits the financial charge.

A process write records its bounded acknowledgement before independent
readback. `implementation.process.result_from: readback` returns that confirmed
public answer; it requires a declared readback. The default remains the
acknowledgement. Do not put extra provider reads before acknowledgement custody
just to make the submission answer resemble the final result.
