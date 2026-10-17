<h1 align="center">runx</h1>

<p align="center"><strong>The governed runtime for agent skills</strong></p>

<p align="center">
  <a href="LICENSE"><img alt="License: Apache-2.0" src="https://img.shields.io/badge/license-Apache--2.0-111111?style=flat-square"></a>
  <a href="https://www.npmjs.com/package/@runxhq/cli"><img alt="npm @runxhq/cli" src="https://img.shields.io/npm/v/@runxhq/cli?style=flat-square&color=f97316&label=%40runxhq%2Fcli"></a>
  <a href="https://github.com/runxhq/runx/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/runxhq/runx/actions/workflows/ci.yml/badge.svg"></a>
  <a href="https://runx.ai/x"><img alt="Skill catalog" src="https://img.shields.io/badge/catalog-runx.ai%2Fx-ff2e88?style=flat-square"></a>
  <a href="https://runx.ai/spec"><img alt="Specification" src="https://img.shields.io/badge/spec-read-7c5cff?style=flat-square"></a>
</p>

<p align="center">
  <a href="#quickstart">Quickstart</a> ·
  <a href="#skills-working-together">Workflows</a> ·
  <a href="#a-skill-carries-judgment">Write a skill</a> ·
  <a href="#what-a-receipt-proves">Receipts</a> ·
  <a href="#build-on-runx">Build on Runx</a>
</p>

---

```text
A skill is a URL.
A graph is what unfolds.
Authority narrows. It does not pass through.
Every act produces a receipt.
```

A skill is expertise published as a portable `SKILL.md`: an operating manual
that a human can understand and an agent can act from. It carries the method,
the evidence to look for, the decisions to make, and the tools the work needs.
Skills can call other skills, passing their results forward into a larger job.

Runx makes that work executable. A research brief can become an outreach draft.
A product launch can connect release work, documentation, and customer updates.
Search and product analytics can inform the next change to your site. Across
those handoffs, Runx admits calls under explicit authority, delivers credentials
at the tool boundary, supervises execution, and seals a verifiable record.

![Northstar's team-workspaces launch connects operations-review, audience evidence from lead-enrichment, release preparation, content-package, message-send, and email or Slack delivery. Product-analytics tracks the path from workspace creation to the first teammate invited.](docs/assets/readme/business-ops.svg)

**Your agent supplies the judgment. Runx owns the execution boundary.** Use the
CLI, MCP, or a language client with the agent or orchestration framework you
already use. Execution, state, and receipt signing run locally; hosted registry,
credential custody, and notary services are optional. The runtime is written
in Rust and licensed under Apache-2.0.

## Quickstart

### With your agent

Give your agent the [Runx operating manual](https://runx.ai/SKILL.md):

```text
Set up Runx: https://runx.ai/SKILL.md
```

Then give it a bounded job. For example:

```text
Use Runx to investigate where our organic visitors stop becoming active users.
Use Search Console, GA4, and our saved PostHog activation funnel. Compare the
last two complete 28-day periods. Bring back the evidence, three priorities,
and a proposed content or product change. Stop before publishing or sending.
```

The agent can discover skills, inspect their inputs, bind the tools you choose,
and work through the run's open turns. Missing accounts, evidence, or decisions
remain explicit requests. [Native skill exports](docs/agent-skills.md) are also
available for Claude Code and Codex.

### From your terminal

Install the native CLI through npm:

```bash
npm install --global @runxhq/cli
```

Or use the standalone installer on macOS or Linux:

```bash
curl -fsSL https://runx.ai/install | sh
```

Run the smallest complete example. This needs Git and Node.js 20 or newer;
it needs no Runx account, model, or provider key.

```bash
git clone --depth 1 https://github.com/runxhq/runx.git
cd runx

runx skill ./examples/hello-world --run-id first-run \
  -i message="Hello, Runx." --json
```

Selected fields from the result:

```json
{
  "status": "sealed",
  "run_id": "first-run",
  "result": { "message": "Hello, Runx." },
  "closure": { "disposition": "closed" },
  "receipt_id": "sha256:…"
}
```

Read the run, then verify the full receipt ID returned by the command:

```bash
runx history first-run --json
runx verify <receipt-id> --json
```

The example executes a checked-in Node.js command. It exercises the same
admission, execution, and signing path used by larger skills. The npm package
distributes the native runtime; Node.js is needed here for the example script.
See [Getting started](docs/getting-started.md) for the full walkthrough and
[installation options](https://runx.ai/start) for platform packages.

Skills can come from a local directory, a URL, or the registry. Published skills
keep their `publisher/name` identity; verified publishers' skills can also hold
a short root name. Local packages take precedence when resolving a bare name.
Use an exact version and digest when you need to reproduce someone else's run.

## Skills working together

Skills pass useful work forward: matched facts, a source-backed brief, a
validated draft, an exact action to approve, or a receipt from the last run.
The next skill can use that result directly.

These are compositions you can give an agent: Runx skills working with your
repositories, provider skills, and connected accounts. Your agent or a
project-owned graph makes the handoffs, carrying the actual outputs into the
next skill's inputs.

### Business ops: get the next release into customers' hands

```text
We're launching team workspaces in Northstar. Use Runx to prepare the release,
choose the customer audience, update the setup guide, draft the announcement,
and coordinate the rollout with the team. Establish the activation baseline
from workspace creation to the first teammate invited so we can measure adoption.
Bring back the launch kit, blockers, and receipts. Ask before sending,
publishing, or deploying.
```

[`operations-review`](skills/operations-review/SKILL.md) reviews the release state and proposes
the next launch actions. [`lead-enrichment`](skills/lead-enrichment/SKILL.md)
organizes supplied customer facts to inform the audience choice; the operator
supplies the actual audience and contact policy.

[`release#prepare`](skills/release/SKILL.md) checks the project's release
profile and produces the version-bound brief. The full `release` runner owns
approval, publication, and verification through the project's commands.
[`content-package`](skills/content-package/SKILL.md) combines release facts
and product documentation with `research` and `ghostwrite` to prepare the setup
guide and announcement, then packages them for publication.

[`message-send#plan`](skills/message-send/SKILL.md) binds the principal,
audience, accepted content, and approval requirements. The calling agent can
hand that plan to [Nitrosend](https://nitrosend.com/SKILL.md) or another email
provider's skill for its review, approval, and delivery workflow. For a team
update, `message-send#send` owns the Slack post and independent readback itself.
[`product-analytics`](skills/product-analytics/SKILL.md) establishes the
workspace-to-invitation baseline from supplied reports and compares later
results under the same measurement definition.

**What comes back:** a launch kit with release notes, a setup guide, the customer
announcement, a team update, and an activation baseline—plus the blockers,
decisions, and receipts needed to take it live and follow adoption afterward.

### Outreach: facts → fit → a useful first contact

![Outreach composition: lead-enrichment and lead-qualify establish facts and fit; research, brand-voice, taste-profile, and contact eligibility feed cold-outreach. Its ghostwrite admission and verification surround the draft. message-send planning hands the accepted email to a selected provider for approval, delivery, and readback.](docs/assets/readme/outreach.svg)

[`lead-enrichment`](skills/lead-enrichment/SKILL.md) normalizes supplied facts
about a confidently matched subject.
[`lead-qualify`](skills/lead-qualify/SKILL.md) checks those facts against your
ICP and keeps commercial fit separate from contact eligibility.
[`cold-outreach`](skills/cold-outreach/SKILL.md) takes that context, supported
research, and the exact recipient's contact disposition to prepare one email.

The draft can use [`brand-voice`](skills/brand-voice/SKILL.md) and
[`taste-profile`](skills/taste-profile/SKILL.md) without allowing either to
invent product claims. Inside `cold-outreach`, `ghostwrite#admit` validates the
evidence and writing context before drafting; `ghostwrite#verify` checks the
result's source bindings afterward. Missing eligibility, suppression, or an
unsupported claim produces a hold.

**What comes back:** the exact sender, recipient, subject, body, and a source
binding for review. `message-send#plan` can take that accepted draft into a
provider handoff. The selected email skill—Nitrosend or another provider—checks
its own audience and sender requirements, obtains approval for the exact send,
and reports the provider result. Choose a provider and delivery mode that permit
the intended contact; commercial fit alone never grants that permission.

Enrichment uses the evidence you supply; a provider lookup is a separate
connector operation. The calling application retains outcomes for follow-up.
A reply, opt-out, or changed eligibility changes what the next run may do.

Try it with your agent: *“Use the supplied prospect research, our ICP, brand
voice, and contact-policy decision to prepare one useful first-contact email.
Show the facts behind it and hold any recipient with unresolved eligibility.”*

### Organic growth: connect discovery to activation

![Organic growth composition: google-search-console, google-analytics, product-analytics, and site research meet in organic-growth. Priorities feed content-package or repository work through code-plan and github-issue-to-pr. Approved changes are measured again using comparable windows and definitions.](docs/assets/readme/organic-growth.svg)

Each source answers a different question. Search Console shows how people find
you. GA4 shows acquisition and behavior under the property's measurement model.
Product analytics shows where a defined journey loses people or accounts
after signup.

[`google-search-console`](skills/google-search-console/SKILL.md) and
[`google-analytics`](skills/google-analytics/SKILL.md) return normalized
evidence with source digests and measurement caveats. The
[`PostHog connector`](connectors/posthog/README.md) reads saved insights from
your selected Cloud project and region; it preserves their query definitions
and cached-result freshness.
[`product-analytics`](skills/product-analytics/SKILL.md) consumes the supplied
aggregate reports, calculates funnel and retention rates, and checks whether
the comparison is meaningful.

[`organic-growth`](skills/organic-growth/SKILL.md) combines search, analytics,
site, and market evidence into priorities and a measurement plan. Product
findings can join that evidence with their source reference and digest. For
example, a page getting more clicks while its visitors fail to activate may
need a clearer promise, a better onboarding path, or an instrumentation fix.

**What comes back:** observations, hypotheses, ranked actions, source bindings,
and a measurement plan. Content work can continue through `content-package`;
code and instrumentation work can go through
[`code-plan`](skills/code-plan/SKILL.md) and
[`github-issue-to-pr`](skills/github-issue-to-pr/SKILL.md) in the owning repository. The next
analysis reuses comparable windows and definitions. Missing or stale reports
remain measurement gaps.

### Run the local workflow proofs

From this checkout:

```bash
# Research → draft → channel package → publication handoff
runx harness skills/content-package/fixtures/composes-in-graph.yaml

# A specific outreach draft with supplied evidence and writing context
runx harness skills/cold-outreach/fixtures/prepares-approved-context-email.yaml

# Search + analytics + site evidence → growth priorities
runx harness skills/organic-growth/fixtures/composes-in-graph.yaml

# Defined product funnel → rates and source-bound recommendations
runx harness skills/product-analytics/fixtures/composes-in-graph.yaml

# Exact Slack post → approval → provider response → independent readback
runx harness skills/message-send/fixtures/post-default-notifies-and-verifies.yaml
```

These are fixture-backed executions, with no live account reads or sends. Live
work uses your provider bindings and actual evidence. Browse the
[skill catalog](https://runx.ai/x) for more workflows, or the
[demo gallery](docs/demos.md) for local execution, state, and loop examples.

## A skill carries judgment

`SKILL.md` is the capability's operating manual. It explains when to use the
skill, what evidence matters, how to make the decisions, when to stop, and what
completion means. It remains readable and useful on its own.

```markdown
---
name: hello-world
description: Echo a message through a checked-in command.
---

# Hello World

Print the supplied message so the caller can inspect the local execution
and its signed receipt.
```

When the work needs executable steps, typed inputs and outputs, authority,
artifacts, or harness cases, the package also carries a `run.yaml`:

```yaml
skill: hello-world
version: "0.1.1"

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

This is the execution shape of [`examples/hello-world`](examples/hello-world).
Its checked-in package also includes the command and harness case.

| Part | Responsibility |
| --- | --- |
| `SKILL.md` and references | Expertise, procedure, judgment, recovery, and completion criteria. |
| `run.yaml` | Executable runners, typed contracts, declared capabilities, authority, and harness cases. |
| Domain modules | Deterministic computation specific to the skill. |
| Runx | Provider transport, filesystem and process access, credential delivery, execution state, and receipts. |

Runx binds the manual and execution closure by digest. An acting agent receives
the invoked skill's full manual; declared adjacent skills contribute bounded
summaries until invoked. A graph can also supply a skill as explicit
`context_skills` on an agent step—for example, a writing rubric. Context informs
judgment; permission comes from admission.

## Graphs make acts composable

A runner can combine five step kinds:

| Step | Use it for |
| --- | --- |
| `agent` | Judgment under the skill's manual and an explicit allowance of calls. |
| `tool` | A native capability, deterministic computation, or declared process command. |
| `operation` | A typed connector call to an external provider. |
| `ask` | A question or decision the run needs before it can continue. |
| `skill` | Another skill's runner, with explicit inputs and outputs. |

A small graph makes the dataflow visible:

```yaml
name: hello-graph
steps:
  - id: first
    skill: ../hello-world
    inputs:
      message: Hello from the first skill.
  - id: second
    skill: ../hello-world
    inputs:
      message: $steps.first.message
```

The second skill consumes the first skill's typed output. The graph receipt
links their receipts. The same mechanism lets `content-package` pass a
research packet into `ghostwrite`, then pass the validated draft into its
packaging and handoff runners. Inspect its
[actual graph](skills/content-package/run.yaml) beside the
[minimal example](examples/hello-graph/graph.yaml).

Use an agent turn for work that needs judgment. Use a graph when phases,
branches, guards, or approval boundaries must be part of the execution
contract. Required side effects belong in explicit execution steps, where
Runx can record the attempt and result. An agent's completion text cannot
substitute for a provider operation.

Graphs and agent turns can both call skills. A host can pass an existing packet
artifact into a later call; Runx checks its content digest and input schema.
This is how a useful result becomes an input to more work without flattening
its provenance into prose.

[Compose skills](docs/skill-to-graph.md) ·
[Execution profile reference](docs/reference.md#skill-and-execution-profile)

## Work can outlive an agent turn

By default, an agent step yields work to the calling agent. Runx returns the
manual, typed request, and allowed calls. Your host can work the turn through
MCP, provide its result, and continue the same run. The optional
`--managed-agent` mode delegates those turns to a configured model provider.

```bash
# Inspect a running, paused, or completed run
runx history <run-id> --json

# Serve the paused run's agent turns over MCP
runx mcp --run <run-id>

# Continue with answers to the run's exact pending requests
runx resume <run-id> answers.json --json
```

The run records pending requests and completed calls. Independent read calls
can run concurrently; effects are serialized within the run. A retry reuses
the call's identity, and an uncertain external result remains explicit for
reconciliation. Resume checks the package and execution closure so a changed
skill cannot silently continue an old run.

Agent answers and human approvals have separate fields. Each decision binds to
the digest of the request it answers. The host is responsible for obtaining
the human decision; putting a JSON value in an approval field is a host
assertion, not proof of a person's identity.

For recurring operations, the outer host owns scheduling and stop policy.
Each iteration can read prior receipts and durable state, submit a bounded
run, and retain the result for the next iteration. See the
[loop example](examples/loop-orchestration) and
[continuation contract](docs/reference.md#local-cli).

## Connect tools without handing secrets to skills

Skills declare the connector operations they need. Connectors declare typed
requests, scopes, credential requirements, and readback behavior. You choose
the account and binding: a local credential, a supported existing session, or
an optional hosted connection.

For example, install web search and bind it to your Exa account:

```bash
runx registry install runx/web-search
runx connector use exa
runx skill runx/web-search -i query="Agent skill interoperability" --json
```

If the connector's declared credential variable is set, `connector use` binds
to it; otherwise it asks for the key. For supported sessions, such as an
existing `gh` sign-in:

```bash
runx connector use github --session --scope repo.read
```

The kernel delivers credentials only at the admitted operation's transport
boundary. Skills name operations and supply typed inputs. Secret material
stays out of their instructions, model context, and receipts.

One skill can work with different compatible providers. One connector binding
can serve several skills. Using hosted credential custody leaves the calling
operator and its state local unless you explicitly select a hosted service.

Connector names identify the capability provider: `github`, `posthog`,
`google/analytics`, or `google/search-console`. A skill binding a connector
outside its own plugin pins the manifest digest; the execution closure binds
the operations it will actually use.

[Credential setup](docs/credentials.md) ·
[Connector catalog](https://runx.ai/connectors) ·
[Connector manifests](connectors/)

## Authority follows the work

Every governed execution follows the same boundary:

```text
Admit → Resolve grant → Deliver credentials → Execute → Seal
```

Authority is checked where a call executes. Child calls remain within the
parent's grant; naming another skill or describing an action in a prompt does
not create permission. An operation that requires approval pauses with the
exact request for the host to present to the human.

The execution lanes have deliberate boundaries:

- **Deterministic modules** perform JSON-to-JSON domain computation without
  ambient filesystem, network, process, environment, clock, or random access.
- **Native capabilities** own reusable runtime mechanics and their admission
  checks, including file access, processes, artifacts, and data operations.
- **Connector operations** execute declared HTTP, MCP, or Connect calls under
  typed authority and effect contracts.
- **Process tools** are trusted local executables. Runx controls their argv,
  working directory, delivered environment, credentials, and supervision.
  They execute with the host's OS permissions; this is not portable OS isolation.

These guarantees cover work executed through Runx. The surrounding agent or
application owns its other tools, authentication of human decisions, and host
isolation.

[Authority and security proof](docs/security-authority-proof.md) ·
[Security policy](SECURITY.md)

## What a receipt proves

A receipt answers the questions that matter after the agent has moved on:

| Question | Evidence |
| --- | --- |
| What ran? | Skill identity, source and execution digests, and runner metadata. |
| Who or what admitted it? | Actor references, grant references, and execution terms. |
| What was allowed? | Scopes, resolved grants, and approval decisions. |
| What happened? | Acts, output references, execution outcomes, and closure. |
| How did the work connect? | Linked child receipts and artifact provenance. |
| Can I check it later? | A content-addressed ID, canonical digest, and signature. |

Runx signs receipts with Ed25519. On first use, an unconfigured Runx home
creates its own local signing key. `runx history` inspects the record;
`runx verify` checks its signature and linked receipt tree.

A valid signature establishes integrity against a trusted signer. Read the
receipt's outcome as well: a sealed refusal or failed run is still sealed.
A local content draft proves preparation; provider readback establishes the
external result. A receipt from another home or hosted issuer requires that
issuer's trusted public key.

Receipts carry references, decisions, hashes, and bounded observations.
Raw credentials and unchecked private provider bodies do not belong in proof.
The result payload and the signed receipt have separate jobs: the result is
what the next caller uses; the receipt records how the work was admitted and
what execution established.

Read the [receipt schema](schemas/receipt.schema.json), inspect a
[sealed fixture](fixtures/receipt-verify/valid-production/receipt.json), or
use the [standalone verifier](tools/verify/verify.mjs) to check proof outside
the execution runtime.

## Author, test, and publish

Keep a skill in your project, adapt an existing package, or publish it for
others to compose. Start an authoring run with:

```bash
runx new launch-review \
  --objective "Review a release and prepare its customer announcement"
```

This enters the `skill-build` workflow. Your agent works the authoring turns;
the package is written after validation. Without a managed agent, the command
returns the pending work and resume instructions.

A finished package should include harness cases for its successful path,
refusals, and composition with other skills. The harness replays declared
inputs and agent/provider responses so those contracts can be checked locally.

```bash
runx harness ./skills/launch-review
runx login --for publish
runx registry publish ./skills/launch-review
```

Hosted publication reconstructs the submitted package, reruns its harness,
and stores immutable package digests. Local harness results prove the cases
they exercise; live integration claims need evidence from the actual provider
path too.

[Authoring reference](docs/reference.md#standalone-skill-packages) ·
[Skill quality standard](docs/skill-quality-standard.md) ·
[Publishing](docs/publishing.md)

## Build on Runx

The CLI, MCP server, and language clients use the same Rust execution path.
An application can start a skill, expose its open turns to a model, route
human decisions, resume the run, and inspect its receipts.

| Surface | Entry point |
| --- | --- |
| CLI | `runx skill <skill-ref> [runner] -i key=value --json` — [reference](docs/reference.md#local-cli). |
| MCP | `runx mcp <skill-ref>` to expose a skill; `runx mcp --run <run-id>` to work an existing run. |
| TypeScript / Node.js | [`@runxhq/client`](packages/client/README.md), with explicit workspace and environment configuration. |
| Python | [Python SDK](packages/sdk-python/README.md). |
| Rust | [`runx-sdk`](crates/runx-sdk/README.md). |
| Contracts | [Schemas](schemas/) and the [public specification](https://runx.ai/spec). |

MCP exposes a turn's allowed calls as typed tools. The host owns the model
conversation; Runx admits and records the calls. You can integrate a single
skill or build a product around long-lived runs without introducing another
execution implementation.

<details>
<summary><strong>Inside the runtime: which layer owns what</strong></summary>

| Layer | Owner |
| --- | --- |
| Portable wire contracts | `runx-contracts` |
| Pure policy, authority, and state transitions | `runx-core` |
| Package parsing and validated package representation | `runx-parser` |
| Canonical receipts, hashing, signatures, and verification | `runx-receipts` |
| Execution, capabilities, processes, adapters, and effects | `runx-runtime` |
| Argument parsing and presentation | `runx-cli` |
| Language clients and extension protocols | `packages/` |
| Operator knowledge and domain computation | `skills/` and product-owned packages |

The [system architecture](docs/architecture/runx-system.md) defines these
ownership boundaries. SDKs, exported agent skills, schemas, and catalog views
consume the same contracts.

</details>

## Contribute

Runx is in active development. Start with a concrete workflow, a reproducible
execution problem, a connector contract, or a skill with useful harness cases.
The [contributor guide](CONTRIBUTING.md) covers setup, focused checks, and
sign-off requirements.

Build the native CLI from this checkout:

```bash
cargo build --manifest-path crates/Cargo.toml -p runx-cli
crates/target/debug/runx harness examples/hello-world
```

[Reference](docs/reference.md) · [Demos](docs/demos.md) ·
[Releases](https://github.com/runxhq/runx/releases) ·
[Open an issue](https://github.com/runxhq/runx/issues)

---

<p align="center"><sub>Built in Rust · Apache-2.0 · <a href="https://runx.ai">runx.ai</a></sub></p>
