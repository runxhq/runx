# Loop Orchestration

Long-running agent work is a loop over governed runs. Each time a loop goes
round, one run of one skill's method runs, with its own authority, approvals
and sealed receipt. The loop decides when to go round again and when to stop.

runx keeps that line in code. Its engine, `runx-core` and the runtime that
runs a skill, learns no loops: it knows no loop noun, and a single `runx skill`
run touches no loop code. Loops are a layer above it, built only on the
runtime's public API, in two forms:

- **runx's own loops**, `loops/<name>.yaml` in a workspace, served by the
  always-on runx (`runx on`). The loop layer is `crates/runx-loops`, and the
  always-on runx is in `runx-cli`.
- **an outside host**: an app, a script, cron, Temporal, LangGraph, n8n,
  Zapier or Make. It submits runs the same way and reads the same receipts.

The words, and where each stops, are in the [glossary](glossary.md).

## Shape

- A **loop** lives outside the engine. It owns what wakes it, its durable
  state, and when it stops.
- An **iteration** is one governed run of a skill's method, the one thing a
  loop submits. It has explicit inputs, authority, the calls its steps declare,
  optional `context.skills`, approval gates, and one sealed receipt. An agent
  step inside it opens a **turn**, whose agent loop runs bounded rounds.
- A **handoff** is a receipt-backed artifact or tool-shaped result asking
  another agent, loop, or system to continue.
- A **signal** is an external event delivered to the loop: for runx's loops,
  a clock time, a connector's feed event, a webhook's payload, or the
  person's message. It may cause a new iteration, but it is not hidden model
  context.
- A **projection** is loop-readable state derived from receipts and admitted
  external events.
- A **stop condition** is checked outside the model: budget, deadline,
  approval state, failures, or a run proposing it's done.

```text
loop
  takes what woke it: a clock, a feed, a webhook, the person
  chooses the next iteration's request
  submits a run of a skill's method with authority + context
  receives sealed receipt / pause / denial
  records how it came out
  goes round again only when something wakes it and its policy allows

runx run
  parses the skill and its workflow
  loads digest-bound context_skills
  admits each call against the skill's limits
  executes bounded model and tool steps
  routes approvals for risky effects
  seals receipt before success
```

## runx's Loops

A loop file names one entry method of a skill, its brief and inputs, what
wakes it (`on`), the authority its runs act under, and its policy. `runx loop
new <skill> <name>` copies a skill's template; `runx start <name>` starts it,
once its inputs are what the method takes.

- **What wakes it** goes into its **inbox**, each event once under its
  identity: a clock time (`every`, `at`, `cron`), a page of a connector's
  **feed**, a request on one of its **webhooks**, the person's message, or
  `runx run <name>`. A feed is read by the always-on runx with no run around
  it; a poll that finds nothing records nothing. A webhook's address comes
  from `runx loop token <name>`. A request on it is one event by its body,
  unless its trigger names the header its sender gives each delivery's id,
  `{webhook: inbound, delivery: X-GitHub-Delivery}`: then a delivery sent
  again is the same event whatever its body, and a request without that
  header is refused.
- **A wake** is one run of the method on a batch of the inbox, claimed and
  started in one commit, one at a time. The run is given the loop, its brief,
  the time and its day, and what woke it, as two context entries,
  `runx.loop.wake` and `runx.loop.batch`. What arrived from outside is
  evidence, never anyone's instruction.
- **A run that waits keeps its code.** Each run keeps the packages it was
  admitted with, by digest, so a skill improved while a run waits on a person
  or a time never strands it: it finishes on what it started with, and every
  new run binds the improvement. A package changed under a caller's own pin
  on a fresh run is still refused.
- **How it came out** goes in the loop's **record**. A run changes its loop
  only by proposing, in its `loop` output: a wake, a notice for the person,
  or that it's done, which runx checks, or a change to its brief or inputs,
  which only a person accepts (`runx loop accept`).
- **The always-on runx**, one per runx home, serves every workspace with a
  started loop, each loop on its own, and sleeps until something is due. A
  sleeping machine runs nothing; on waking it runs what came due, marked late.

`runx loops` lists every loop, what waits on the person, and what's next.

## Security Gates

A loop must not be able to smuggle authority through prompts, whichever host
runs it.

Required gates:

- **Iteration budget:** a bound on how much each loop may do; inside each run,
  its steps' `max_calls`, and the rounds of each agent turn.
- **Authority budget:** spend and effect limits hold across its runs; the next
  prompt cannot mint fresh authority.
- **Context budget:** `context.skills` and prior receipt summaries are capped,
  digest-bound, and marked untrusted.
- **Tool admission:** every model-selected tool must appear in its agent step's
  `calls.tools` and pass normal runx tool-ref admission.
- **Side-effect review:** side-effecting tools may pause for human or policy
  review before execution.
- **Idempotency:** every iteration has a stable loop id and a run id the host
  chooses, so a retried submission returns what that run sealed instead of
  running again.
- **Stop policy:** loops fail closed on exhausted budget, failures, stale
  projection, or missing required receipts.
- **Replay discipline:** replay reads receipts and projections. It never treats
  hidden prompts as source of truth.

runx's loops meet them in code:

- A loop's runs act under the authority its file names, or the workspace's
  standing one. A revision that would widen what it calls, its authority or
  its policy waits for a start.
- Each wake reserves from the day's budget (`policy.spend.per_day_usd`, zero
  when absent), so a loop that calls a priced operation needs one.
- An unattended wake runs only on a zero-price model.
- A failed wake is tried again on a ladder (`policy.retry`), and failures in
  a row pause the loop for the person, three by default
  (`policy.on_failure.pause_after`).
- A wake takes at most `policy.batch` events and `policy.batch_bytes` of
  them.
- Nothing a loop can't do is in a log alone. `runx loops` and `runx loop
  show` say why it couldn't run, each feed it can't read and since when
  (`feeds_failing` in `--json`), and what the always-on runx can't do for it
  (`host_troubles`); each clears once it works. `runx status` lists all the
  always-on runx can't do, a workspace it can't read among them.

## What Belongs Where

The engine, for every run:

- governed runs of skills' methods;
- authority attenuation and approval gates;
- bounded model/tool execution;
- governed data operations through declared data-source adapters;
- digest-bound skill context;
- receipt sealing and verification.

The loop, runx's or an outside host's:

- what wakes it, and when;
- durable loop state and projections;
- retries across runs;
- cross-agent routing;
- product-specific stop policy;
- resident operation.

runx's loops keep their state in host streams in the workspace's store. An
outside host storing state through runx uses the provider-agnostic data plane
in [docs/governed-data-plane.md](governed-data-plane.md): the host owns
scheduling and stop policy, while runx seals each bounded read, append, or
projection operation.

## Do Not Build

- No loop noun, loop effect family or loop packet in `runx-core` or the
  engine, and no product-specific contract branch.
- No resident model daemon in `runx-core`.
- No hidden prompt-to-prompt continuation outside receipts.
- No implicit remote skill fetching during loop execution.
- No unbounded revise/improve loop.
- No product-specific backend lever in the kernel.

## Research Grounding

The design follows mature orchestration systems instead of inventing a special
loop model:

- LangGraph frames agent orchestration around durable execution, persistence,
  human-in-the-loop interrupts, and stateful workflow execution:
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

## Examples

`examples/loop-orchestration` is an outside host: a local fixture loop that
submits bounded runx runs, prints each receipt id and the reason for or
against another iteration, demonstrates `context.skills`, includes a refusal
path, and runs without provider keys.

```sh
sh examples/loop-orchestration/run.sh
```

Its package is `examples/loop-orchestration/skill`, and the loop host,
`loop-host.mjs`, sits beside it. For a repeatable check of the package's
tests, use a clean receipt directory:

```sh
tmpdir="$(mktemp -d)"
"${RUNX_BIN:-crates/target/debug/runx}" test examples/loop-orchestration/skill \
  --receipt-dir "$tmpdir" \
  --json
```
