# Delegation Reference

Use this reference when the objective is about release, registry publication,
deploy, hosted operations, project profiles, CLI handoff, or dogfooding runx.

## Rule

`operations-review` reasons and routes. It does not implement the operation.

The execution surface is one of:

- a governed skill runner;
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
  lane_ref: string
  profile_ref: string | null
  command_ref: string | null
  workflow_ref: string | null
  approval_gate: string | null
  verifier_ref: string | null
```

`command_ref` is descriptive, not a shell transcript. The deterministic graph
step or human operator runs the command with real inputs after approval.

## Release Example

Good:

```yaml
lane: release
execution:
  interface: workflow
  lane_ref: release
  profile_ref: project.release.yaml
  workflow_ref: .github/workflows/release-cli.yml
  approval_gate: release.publish.approval
  verifier_ref: release.verify
verification:
  expected_receipt: project.release.report.v1
  readback: registry package, GitHub release, Docker image, website version, service health
```

Bad:

```yaml
reason: Build every platform, publish npm packages, update the website, deploy
  the service, and check health by following these hand-written shell steps...
```

That duplicates the release lane and will drift.

## Stop Conditions

- No known execution surface for a consequential action.
- Profile asks the operator skill to perform hidden implementation logic.
- Profile or context contains raw credentials or private keys.
- Requested action depends on a CLI/API feature that does not exist yet.
- Verification cannot be stated without trusting the same system that performed
  the action.
