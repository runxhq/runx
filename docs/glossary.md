# Glossary

runx's words, each for one thing. [reference.md](reference.md) holds the exact
contracts; this page says what each word means and where it stops.

## What you write

- **Skill**: a package that does one job: its `SKILL.md`, its `run.yaml`, and
  any tests, evals, fixtures and code it carries.
- **Method**: what you can call on a skill, with its own inputs, outputs and
  requirements, named as `skill#method`. One operation, a workflow or one agent
  turn implements it.
- **Workflow**: a method's fixed steps, run in written order, each gated by its
  `when` and `guard`.
- **Step**: one part of a workflow. Its kind is one of:
  - `tool`: a catalog tool, a deterministic worker or a trusted host process;
  - `operation`: a connector operation, admitted and executed by the kernel;
  - `skill`: a child run of another skill, holding no more than its step;
  - `agent`: an agent turn;
  - `ask`: a resolution the run waits on, such as an approval or a question;
  - `load`: a package a turn's catalog rule admits for the turn's later calls;
  - `script`: deterministic code with no I/O that drives the calls its step
    lists, each admitted, run and recorded by the kernel.
- **Connector**: how runx reaches a provider, declared in `connector.yaml`. Its
  operations are what a step or call may invoke on it.
- **Test**: a case in a package's `tests/`, run by `runx test` with a scripted
  model and stand-ins for every provider. **Eval**: a case in `evals/evals.json`,
  run with a real model.

## What runs

- **Run**: one execution of a method, with its own id, record and receipt. A
  host chooses its id, and starting a run again by that id returns what it
  sealed.
- **Turn**: an agent's span inside a run: what it was given, the calls it made
  and the answer it ended with.
- **Agent loop**: the model's cycle inside one turn: a model request, the calls
  it asks for, their results, and the next request. A **round** is one pass of
  it, and `agent.rounds`, or `--drive-rounds` for one run, bounds a driven
  turn's rounds.
- **Call**: one thing a turn or a script asks runx to do: a tool, an
  operation, a skill, an ask, a load, a wait or a read. Each is admitted against
  the step's `calls` and the run's authority before it runs, and recorded.
- **Agent harness**: what works a turn: runx's own driver (`--drive`), Claude Code,
  Codex, or any host over `runx mcp --run`.
- **Host**: whatever starts a run and answers what it asks.

## What it may do

- **Authority**: what a run may do, which a host states and a run, method or
  step can only narrow. A **term** grants a call on a person's data.
- **Effects**: what may change, and whether it waits: `none`, `ask` or
  `allow`. **Reach**: where file, git and command tools may go: `workspace` or
  `inputs`.
- **Approval**: a person's decision on one exact request, bound to its digest.

## What it keeps

- **Run record**: a run's events, which recovery and `runx history` read.
- **Receipt**: the signed proof that a run sealed, and how.
- **Artifact**: a retained result, by digest. A **packet** is a typed result,
  named by its schema id, such as `runx.verdict.v1`.
- **Stream**: an owned, named set of events in `store` or `private`, which access
  rules target. A **key** picks one entity in it, with its own order and
  **version**.
- **Memory**: the `runx/memory` skill over `private` streams: what a person said
  and what an agent observed, which a turn sees only when its authority names the
  stream.
- **Verdict**: a judgement of an exact subject revision against a fixed set of
  criteria: `met`, `not_met` or `inconclusive`. An **evaluator** is any step or
  call that returns one.

## Not runx's

A **loop**, a **schedule**, a **trigger** that starts one, a **goal** that ends
one, and a unit like a Work belong to a host: an app, Claude Code's `/loop`,
cron, a script, Temporal. runx has none of them. Each time a loop goes round, the
host starts a run, and runx gives it back a receipt, a verdict if the method
returns one, and streams the next run reads. See
[loop-orchestration.md](loop-orchestration.md).

Two of these words mean something else inside runx: a skill's `should_trigger`
in its evals is whether its description gets it selected, and `stop_conditions`
in a skill's authoring plan is guidance for the agent writing it.
