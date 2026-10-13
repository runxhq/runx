<h1 align="center">runx</h1>

<p align="center"><strong>the governed runtime for agent skills</strong></p>

<p align="center">
  <a href="LICENSE"><img alt="license: Apache-2.0" src="https://img.shields.io/badge/license-Apache--2.0-111111?style=flat-square"></a>
  <a href="https://www.npmjs.com/package/@runxhq/cli"><img alt="npm @runxhq/cli" src="https://img.shields.io/npm/v/@runxhq/cli?style=flat-square&color=f97316&label=%40runxhq%2Fcli"></a>
  <a href="https://github.com/runxhq/runx/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/runxhq/runx/actions/workflows/ci.yml/badge.svg"></a>
  <a href="https://runx.ai/x"><img alt="catalog" src="https://img.shields.io/badge/catalog-runx.ai%2Fx-ff2e88?style=flat-square"></a>
  <a href="https://runx.ai/spec"><img alt="spec" src="https://img.shields.io/badge/spec-read-7c5cff?style=flat-square"></a>
</p>

---

```text
a skill is a URL.
a graph is what unfolds.
authority narrows. it does not pass through.
every act produces a receipt.
```

A skill is expertise published as a portable `SKILL.md`: an operating manual
that a human can understand and an agent can act from. Skills compose into
graphs and real work without bespoke glue code. Runx supplies the boundary:
it admits each act under explicit authority, delivers credentials without
turning them into prompt material, supervises execution, and seals the result
into a verifiable receipt.

Authority narrows through the chain, so agent work compounds without becoming
ambient trust.

## what runx is not

Runx is the governed runtime **beneath** agents and orchestration tools, not
another agent framework. There are no prompt chains or vector stores here, and
no model loop runs unless you opt into the managed agent with
`--managed-agent`; runx sits under whatever orchestration layer you already
run. Runx admits each act under explicit authority, delivers credentials
without turning them into prompt material, supervises execution, and seals the
result into a verifiable receipt.

The local CLI/runtime (`runx`, the `@runxhq/cli` npm package) is what executes
skills, replays their harness cases and seals receipts on your machine.
Connectors declare each provider operation and the credential that authorizes
it. The hosted surfaces are optional complements: the registry and catalog at
[runx.ai/x](https://runx.ai/x) publish and discover skills and connectors, runx
Connect holds provider grants, and the notary counter-seals a receipt you
choose to share. You can use the local runtime alone; none of the hosted
surfaces are required to seal your first receipt.

## quickstart

This README has an agent-readable twin at
[runx.ai/SKILL.md](https://runx.ai/SKILL.md). Give it to an agent and the agent
learns the CLI, the catalog at [runx.ai/x](https://runx.ai/x), and how to
return receipts.

Install the CLI:

```bash
npm i -g @runxhq/cli
# or: curl -fsSL https://runx.ai/install | sh
```

Then choose how you want to run skills.

### agent path

Hand the agent a goal and let it drive the runtime:

```text
Use runx to plan and execute end-to-end business ops for my company.
Signal: acme.com signed up 40 seats yesterday.
Stop before sends, spend, merges, deploys, or publishing. Return receipts.
```

### CLI path

Run a local or catalog skill directly:

```bash
runx skill <skill-ref> [runner] -i key=value --json
```

Seal a receipt locally in under five minutes, with no account and no hosted surface:

```bash
npm i -g @runxhq/cli
git clone --depth 1 https://github.com/runxhq/runx.git
cd runx
runx skill ./examples/hello-world -i message="hello, runx" --json
```

The checked-in `examples/hello-world` skill runs a local command, and the final
`--json` output is a `runx.skill_run.v1` result with `status: "sealed"` and a
`receipt_id`. Inspect the governance record with
`runx history <receipt-id> --json`.

Some other examples:

```bash
# Research and strategy: produce a governed decision brief.
runx skill deep-research \
  -i objective="Which launch risks should we resolve first?" \
  --json

# Maintainer operations: draft a useful issue response.
runx skill issue-triage \
  -i repository=runxhq/runx \
  -i issue_number=241 \
  -i objective="Draft the next helpful maintainer response" \
  --json
```

Build the native CLI from source when working on Runx itself:

```bash
cargo build --manifest-path crates/Cargo.toml -p runx-cli
```

On macOS 26, complete the
[Developer Tools permission prerequisite](CONTRIBUTING.md#macos-developer-tools-permission)
if this build stalls.

The npm package distributes the same Rust-owned behavior; it is not a second
runtime.

## a skill carries judgment, not runtime plumbing

`SKILL.md` is the capability's operating manual. It teaches the operator what
the work means, when to use the lane, what evidence matters, where judgment
ends, what requires approval, how failure and recovery work, and when to route
to an adjacent skill.

```markdown
---
name: hello-world
description: Echo a first Runx message through a checked-in command.
---

# Hello World

Use this package to prove the local execution and receipt path.
```

When a skill needs deterministic execution, typed inputs, graph stages,
authority, artifacts, or harness cases, it also carries a `run.yaml` execution
profile. Each runner is a list of steps, and each step is one of five kinds:
`agent`, `tool`, `operation`, `ask`, or `skill`.

```yaml
skill: hello-world
version: "0.1.0"

runners:
  default:
    default: true
    inputs:
      message:
        type: string
        required: true
    steps:
      - id: default
        tool: { command: node, args: [run.mjs] }
        inputs:
          message: $input.message
        outputs:
          message: string
```

The split is deliberate:

- `SKILL.md` owns the knowledge a human and acting agent need.
- `run.yaml` owns machine-checkable execution, authority, and evidence contracts.
- package JavaScript exists only for deterministic domain computation that the
  graph and native capability plane cannot express cleanly.
- HTTP, filesystem, process, credential, packet, and receipt mechanics belong
  to the runtime, not copied helpers inside skills.

Runx digest-binds the complete current manual into the acting context and the
resume envelope. Declared adjacent skills contribute bounded summaries until
invoked; invocation then supplies that skill's complete manual.

See [Skill to Graph](docs/skill-to-graph.md) and
[Skill Catalog](docs/skill-catalog.md).

## graphs make acts composable

A runner calls runx through the CLI subprocess (`runx skill <skill-ref> ...`),
the MCP surface, or the language bindings in `packages/`; there is no HTTP
server to stand up. One `runx skill` invocation is one governed run; to chain
multiple skills in a single run, compose a graph.

Graphs let one governed act consume the typed output of another:

```yaml
name: hello-graph
steps:
  - id: first
    skill: ../hello-world
    inputs:
      message: hello from graph
  - id: second
    skill: ../hello-world
    inputs:
      message: $steps.first.message
```

The boundary is not how many model calls happened. The boundary is what must be
guaranteed:

- **Graphs** own deterministic composition, branches, guards, and recovery.
- **Agent steps** own bounded judgment under the current manual and an explicit
  tool set.
- **Native capabilities** own reusable, product-neutral runtime mechanics.
- **Deterministic modules** perform isolated JSON-to-JSON domain computation
  with no ambient filesystem, network, process, environment, clock, or random
  authority.
- **Process tools** are intentional trusted local executables under an exact
  resolved grant. Runx controls argv, cwd, delivered environment, credentials,
  supervision, and evidence; it does not claim portable OS confinement.
- **Connector operations** perform governed HTTP, MCP, or Connect requests
  under typed authority and effect contracts.

Required mutations, API calls, payments, and provider writes belong in
deterministic effect-owning lanes. An agent or graph author cannot acquire an
effect merely by naming it in prose or input data.

## authority without secret leakage

Credentials belong to connectors, and skills only name the connector
operations they call. Add a skill (it brings the connectors it binds), then bind
an account once by piping the secret on stdin:

```bash
runx registry install runx/web-search
printf '%s' "$EXA_API_KEY" | runx connector use runx/exa --credential api_key

runx skill runx/web-search -i query="receipt verification" --json
```

The kernel delivers the bound instance's credential to each operation it
admits, so no skill, prompt, input, or receipt holds the material. Skill runs,
resume, inspect, managed agents, and MCP use the same readiness contract.

Receipts may include requested and granted scopes, grant references, typed
execution-boundary observations, approval decisions, provider observations,
and hashes. They must not contain raw tokens, passwords, ambient environment
dumps, or unchecked private provider bodies.

See [Credentials](docs/credentials.md) and
[Security Authority Proof](docs/security-authority-proof.md).

## what a receipt proves

A Runx receipt answers the questions that matter after the agent has moved on:

| Question | Receipt surface |
| --- | --- |
| What ran? | subject, skill ref, source type, runner metadata |
| Who or what admitted it? | actor ref, grant refs, execution terms |
| What was allowed? | scopes, resolved grants, approval metadata |
| What happened? | acts, output artifacts, exit status, closure summary |
| Can it be checked later? | content-addressed id, canonical digest, signature, lineage |
| Did secrets leak into proof? | redaction metadata and hashed material refs |

Every governed execution passes through one invariant:

```text
admit -> resolve grant -> deliver credentials -> execute -> seal
```

Runx signs every receipt. With no signer configured, it signs with this runx
home's own key, created on the first run. Verify the workspace's receipts
against it:

```bash
runx verify --json
```

A receipt from another home or a hosted issuer verifies against the public key
you configure for it. The receipt is not the product by itself; it is where authority, action, evidence, and future
learning meet in one verifiable object.

The committed contract is
[`schemas/receipt.schema.json`](schemas/receipt.schema.json); a real sealed
receipt example lives at
[`fixtures/receipt-verify/valid-production/receipt.json`](fixtures/receipt-verify/valid-production/receipt.json).

## demos that prove boundaries

These checked-in paths produce receipts rather than screenshots or prose-only
claims:

| Demo | What it proves | Run |
| --- | --- | --- |
| `examples/hello-world` | Native CLI skill path and sealed receipt baseline | `runx harness examples/hello-world` |
| `examples/hello-graph` | One skill consumes another's typed output in a sealed graph | `runx harness examples/hello-graph/harness.yaml` |

See [Demos](docs/demos.md).

## publish and trust

A public skill is a standalone package: a substantive `SKILL.md`, optional
`run.yaml`, and only the files Runx can consume. Publish locally first:

```bash
runx registry publish ./skills/<your-skill>
```

Then publish to the hosted catalog when you want shared discovery:

```bash
runx login --for publish
runx registry publish ./skills/<your-skill> \
  --registry https://api.runx.ai
```

Hosted publishing reconstructs the submitted package, reruns its harness, and
stores immutable package digests. Publisher declaration alone is not trust.

See [Publishing](docs/publishing.md).

## architecture

Runx has one owner for every contract and behavior:

| Layer | Owner |
| --- | --- |
| portable wire contracts | `runx-contracts` |
| pure policy, authority, and state transitions | `runx-core` |
| package parsing and the aggregate validated package IR | `runx-parser` |
| canonical receipts, hashing, signatures, verification | `runx-receipts` |
| execution, capabilities, process supervision, adapters, effects | `runx-runtime` |
| argument parsing and presentation | `runx-cli` |
| generated language bindings and narrow extension protocols | `packages/` |
| operator knowledge and irreducible domain computation | `skills/` and product-owned packages |

The native CLI, SDKs, schemas, catalog views, exported agent shims, and docs all
consume those owners; none is a parallel parser, executor, credential loader,
authoring framework, effect registry, or provider client.

The normative contract is
[Runx System Architecture](docs/architecture/runx-system.md). Historical design
notes explain how the repository arrived here but do not override it.

## docs

| Read this | When you need |
| --- | --- |
| [getting started](docs/getting-started.md) | first skill, first receipt |
| [system architecture](docs/architecture/runx-system.md) | ownership, execution lanes, boundaries |
| [credentials](docs/credentials.md) | connector credentials, instances, hosted grants |
| [skill to graph](docs/skill-to-graph.md) | compose governed acts |
| [security authority proof](docs/security-authority-proof.md) | scope, credentials, grants, verification |
| [demos](docs/demos.md) | runnable proof paths |
| [publishing](docs/publishing.md) | local and hosted skill publishing |
| [skill catalog](docs/skill-catalog.md) | categories, search, first-party map |
| [reference](docs/reference.md) | CLI, crates, registry, receipts, extension protocols |
| [the spec](https://runx.ai/spec) | act model, receipt grammar, public contracts |
| [the catalog](https://runx.ai/x) | governed skills by URL |

## contributing

Setup, focused test selection, and sign-off rules are in
[CONTRIBUTING.md](CONTRIBUTING.md). Security policy:
[SECURITY.md](SECURITY.md). Runx is Apache-2.0 licensed; see [LICENSE](LICENSE).

---

<p align="center"><sub>built in Rust &middot; Apache-2.0 &middot; <a href="https://runx.ai">runx.ai</a></sub></p>
