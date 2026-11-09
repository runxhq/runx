# Delegation Reference

Use this reference when the objective is about release, registry publication,
deploy, hosted operations, project profiles, CLI handoff, or dogfooding runx.

## Rule

`operations-review` reasons and routes. It does not implement the operation.

The execution surface is one of:

- a governed skill method;
- an existing CLI command;
- a hosted API route;
- a GitHub Actions workflow;
- a provider tool;
- a manual human gate.

If a command, workflow, or API already exists, reference it. Do not restate its
logic in the ops desk packet.

## Project Profiles

Project profiles are topology and policy context. They may name:

- existing commands;
- workflow ids;
- environment names;
- deploy targets;
- package names;
- registry ids;
- health and verification URLs;
- approval gate ids;
- receipt classes expected after the run.

They must not carry raw secrets, private keys, copied provider state, or a
second implementation of a release/deploy/publish flow.

## Handoff Fields

Each consequential proposal should include:

```yaml
execution:
  interface: skill | cli | hosted_api | workflow | provider_tool | manual
  lane: string
  profile: string | null
  command: string | null
  workflow: string | null
  approval_gate: string | null
  verifier: string | null
```

`command` is descriptive, not a shell transcript. The deterministic workflow
step or human operator runs the command with real inputs after approval.

## Release handoff

Inspect `release` and the project's existing release profile. Reuse its
preparation, exact approval, built artifact and independent channel readbacks.
Keep the actual profile path and workflow identity from that project; this
review supplies neither a guessed path nor an adjacent release script.
A submitted workflow is not a completed publication.

## Stop Conditions

- No known execution surface for a consequential action.
- Profile asks the operator skill to perform hidden implementation logic.
- Profile or context contains raw credentials or private keys.
- Requested action depends on a CLI/API feature that does not exist yet.
- Verification cannot be stated without trusting the same system that performed
  the action.
