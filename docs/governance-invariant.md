# The uniform-governance invariant

Every governed execution in Runx passes through one ordered chain:

```text
admit -> resolve grant -> deliver -> execute -> seal
```

This is the contract that makes Runx a permissions broker. A front cannot run
an act without admission, reconstruct authority from its manifest, receive
ambient secrets, or finish without a receipt that records the granted authority
and the boundary the runtime actually observed.

## The stages

### 1. Admit

The orchestration admits the act against the configured authority and effect
policy before dispatch. Local skills use the same pure policy owner. An
unadmitted act never reaches an adapter.

### 2. Resolve grant

The request is the selected runner's `ExecutionRequirements`: auth, arbitrary
scope strings, declared environment names, and runtime metadata. Connected grant resolution and attenuation produce the authority
decision. Adapters consume that decision; they do not reinterpret YAML or
invent scopes.

### 3. Deliver

The runtime projects only the resolved non-secret environment and credential
delivery into the selected execution lane. Environment declarations never
become a credential transport. Secret values remain separate, are delivered
only through the credential contract supported by that lane, and are redacted
before captured output enters a receipt.

### 4. Execute

Execution truth is lane-specific:

- native capabilities enforce their own typed scope and effect contract;
- deterministic JavaScript receives an in-memory module bundle, JSON,
  declared non-secret environment, fixed limits, and no host APIs;
- managed agent work records a remote-provider boundary;
- CLI tools, local MCP servers, and process protocols are trusted host code.

For host processes Runx controls exact argv, cwd, delivered environment,
credentials, stdin, timeout, bounded output, interruption, process groups or
Job Objects, kill-tree behavior, and cleanup. It does not claim portable
filesystem, network, or syscall confinement.

Every executed lane records one typed `ExecutionBoundaryObservation`:
`native_capability`, `deterministic_worker`, `trusted_host_process`, or
`remote_provider`.

### 5. Seal

The orchestration seals the outcome centrally with its admission witness,
resolved grant references, public credential observations, typed execution
boundary, output hashes, and closure state. Raw credential material and ambient
environment never enter the proof.

## Adding an execution front

A new front must reuse the existing requirements, grant, credential-delivery,
process-supervision, and receipt owners. It may add a new domain protocol, but
not a second parser, scope vocabulary, environment loader, process wrapper,
approval system, or receipt projection.

The front must identify the boundary it can actually observe. A declaration is
not evidence. A host process remains `trusted_host_process` even when the skill
requests narrow scopes; those scopes govern Runx capabilities and provider
calls, not the process's operating-system syscalls.

## Conformance

| Invariant | Primary proof |
| --- | --- |
| admission and sealing | `crates/runx-runtime/tests/governance_witness.rs` |
| exact environment and credential delivery | `credential_grant_policy.rs`, `credential_delivery.rs`, `process_invocation_contract.rs` |
| process lifecycle and kill-tree behavior | process supervisor unit tests |
| deterministic worker isolation and limits | JavaScript worker hostile-module and supervisor tests |
| typed execution-boundary evidence | adapter, native-dispatch, agent-context, and receipt-history tests |
