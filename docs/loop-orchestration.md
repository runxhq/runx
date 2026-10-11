# Loop Orchestration

Loop orchestration is the pattern where developers stop prompting one model
directly and instead build bounded systems where agents observe state, call
tools, ask other agents to reason, receive feedback, and continue until an
explicit stop condition is reached.

runx should make those loops safe and verifiable without becoming the loop host.

## Shape

- A **loop** lives outside the trusted kernel. It owns scheduling, durable loop
  state, wakeups, projections, and stop policy.
- A **turn** is one governed runx skill or graph run. It has explicit inputs,
  authority, the skill's declared `tools:`, optional `context_skills`, bounded model/tool
  rounds, approval gates, and one sealed receipt.
- A **handoff** is a receipt-backed artifact or tool-shaped result asking
  another agent, loop, or system to continue.
- A **signal** is an external event delivered to the loop host. It may cause a
  new turn, but it is not hidden model context.
- A **projection** is loop-readable state derived from receipts and admitted
  external events.
- A **stop condition** is checked outside the model: max turns, budget, deadline,
  approval state, confidence threshold, or explicit terminal output.

```text
loop host
  loads projection from receipts/events
  chooses next turn request
  submits runx skill/graph with authority + context
  receives sealed receipt / pause / denial
  updates projection
  repeats only if stop policy allows

runx turn
  parses skill/graph
  loads digest-bound context_skills
  admits each call against the skill's limits
  executes bounded model/tool graph
  routes approvals for risky effects
  seals receipt before success
```

## Research Grounding

The design follows mature orchestration systems instead of inventing a special
runx loop model:

- LangGraph frames agent orchestration around durable execution, persistence,
  human-in-the-loop interrupts, and stateful graph execution:
  `https://docs.langchain.com/oss/python/langgraph/overview`,
  `https://docs.langchain.com/oss/python/langgraph/persistence`, and
  `https://docs.langchain.com/oss/python/langchain/human-in-the-loop`.
- Temporal separates durable workflow/event history from activities, and treats
  messages such as signals, updates, and queries as the right way to interact
  with long-running workflows: `https://docs.temporal.io/workflow-execution`,
  `https://docs.temporal.io/child-workflows`, and
  `https://docs.temporal.io/sending-messages`.
- OpenAI Agents SDK separates agents, handoffs, guardrails, human review,
  run state, and tracing. Handoffs are represented as tools, which maps cleanly
  to runx's governed tool boundary:
  `https://developers.openai.com/api/docs/guides/agents`,
  `https://developers.openai.com/api/docs/guides/agents/guardrails-approvals`,
  `https://openai.github.io/openai-agents-python/handoffs/`, and
  `https://openai.github.io/openai-agents-python/tracing/`.
- MCP standardizes external tools, resources, prompts, and workflows:
  `https://modelcontextprotocol.io/docs/getting-started/intro`. For runx, MCP
  is a front or transport; it is not the loop model itself.

## Security Gates

A loop host must not be able to smuggle authority through prompts.

Required gates:

- **Turn budget:** maximum turns per loop and maximum model/tool rounds per
  turn.
- **Authority budget:** spend/effect limits are consumed across linked turns;
  the next prompt cannot mint fresh authority.
- **Context budget:** `context_skills` and prior receipt summaries are capped,
  digest-bound, and marked untrusted.
- **Tool admission:** every model-selected tool must appear in the skill's `tools:`
  and pass normal runx tool-ref admission.
- **Side-effect review:** side-effecting tools may pause for human or policy
  review before execution.
- **Idempotency:** every turn has a stable loop id, turn id, and idempotency key.
- **Stop policy:** loops fail closed on exhausted turns, budget, stale
  projection, or missing required receipts.
- **Replay discipline:** replay reads receipts and projections. It never treats
  hidden prompts as source of truth.

## What Belongs Where

Use runx for:

- governed skill/graph turns;
- authority attenuation and approval gates;
- bounded model/tool execution;
- governed data operations through declared data-source adapters;
- digest-bound skill context;
- receipt sealing and verification.

Use the outer loop host for:

- scheduling and wakeups;
- durable loop state and projections;
- retries across turns;
- cross-agent routing;
- product-specific stop policy;
- resident hosted operation.

The loop host can be a local script, hosted runx service, product app, Temporal
workflow, LangGraph app, n8n/Make/Zapier workflow, or another orchestrator.
When the loop host stores state through runx, use the provider-agnostic data
plane in [docs/governed-data-plane.md](governed-data-plane.md): the host owns
scheduling and stop policy, while runx seals each bounded read, append, or
projection operation.

## Do Not Build

- No resident model daemon in `runx-core`.
- No `LoopResourceFamily`, `runx.loop.*` packet namespace, or product-specific
  contract branch.
- No hidden prompt-to-prompt continuation outside receipts.
- No implicit remote skill fetching during loop execution.
- No unbounded revise/improve loop.
- No product-specific backend lever in the kernel.

## Example

The canonical OSS example is `examples/loop-orchestration`: a local fixture
loop that submits bounded runx turns, prints each receipt id and next-turn
reason, demonstrates `context_skills`, includes a refusal path, and runs
without provider keys.

Run it from the OSS workspace:

```sh
sh examples/loop-orchestration/run.sh
```

For a repeatable harness check, use a clean receipt directory:

```sh
tmpdir="$(mktemp -d)"
"${RUNX_BIN:-crates/target/debug/runx}" harness examples/loop-orchestration \
  --receipt-dir "$tmpdir" \
  --json
```

The example proves the shape before any CLI sugar or hosted loop host is added.
