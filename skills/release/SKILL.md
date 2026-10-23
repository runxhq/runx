---
name: release
description: Prepare, approve, execute, and independently verify a versioned release through a project-owned exact-command profile.
---

# Release

A release is a project-owned operation, not a universal shell recipe. This
skill gives Runx one common governance sequence—prepare, describe, approve,
publish, and independently verify—while leaving the actual build and provider
commands in a versioned profile owned by the project.

Use it when a package or project already knows how to prepare, publish, and
verify itself but needs one digest-bound approval and receipt chain around those
commands. Do not use it to invent a release process for an unconfigured project
or to replace the project's tests, packaging, or registry tooling.

A release profile is the project adapter. It declares exact argv commands that
the project already owns; this skill validates their release semantics but does
not recreate the project's release implementation.

Runx core owns bounded profile reads, path and symlink containment, command-plan
digests, sanitized child environments, process-tree supervision, timeouts,
credential redaction, and output evidence. Package JavaScript only validates the
release profile and interprets the three release-specific provider states.

Use `prepare` to execute the profile's preparation command and produce an
evidence-bound release brief. Use the default `release` runner to approve the
exact publish plan, execute it without a shell, then run a separate verification
command. The publish command is refused if its normalized argv plan drifts after
approval. Publication is never inferred from an agent answer or a zero exit code.

## Project profile

`project_root` is resolved inside the current Runx workspace and must be a
relative directory (normally `.` when Runx is invoked from the project root).
`profile_path` must name a JSON file inside that directory:

```json
{
  "schema": "runx/release.profile.v1",
  "id": "my-project/npm",
  "channel": "npm",
  "commands": {
    "prepare": { "argv": ["pnpm", "release:check"], "cwd": ".", "timeout_ms": 300000 },
    "publish": { "argv": ["gh", "workflow", "run", "release.yml"], "cwd": "." },
    "verify": { "argv": ["pnpm", "release:check:live"], "cwd": "." }
  }
}
```

Commands are argv arrays, never shell strings. Paths cannot escape the project.
These commands are trusted host processes: Runx fixes and digests their argv,
working directory, admitted environment, and timeout, but does not claim to
confine their filesystem, network, or syscalls. Keep profiles project-owned,
versioned, and narrowly scoped; the publish command still requires the exact
digest-bound approval before execution.
Credential-shaped arguments and inline environment values are rejected; commands
receive credentials only through Runx credential delivery or existing local CLI
profiles. The release profile is context and topology, not authority.

Every command must print one bounded JSON object. Preparation returns
`status: ready`; publish returns `status: submitted` or `published`;
verification returns `status: verified`, the exact `version` and `channel`,
and at least one stable locator. Optional fields are `release_id`,
`commit`, `checks`, and `locators`. Raw stdout and stderr are not copied
into release artifacts; only parsed bounded fields and runtime evidence digests
are sealed.

## Authority and closure

Preparation and note drafting do not require human approval. The one approval
gate binds the profile digest, version, channel, and native publish command-plan
digest. Only publish crosses the consequential external boundary. Verification
is independent readback and is the only path to `verified`. Failed commands,
identity drift, plan drift, invalid JSON, or missing locators never produce a
success claim.

## Stop conditions and recovery

- Stop when the profile escapes the project root, follows an unsafe symlink,
  uses shell strings, embeds credential-shaped arguments or environment values,
  or omits one of the three command roles.
- Stop when preparation does not return bounded `ready` JSON or when the
  requested version and channel differ from the profile evidence.
- Approval binds the exact publish argv plan. Any normalized plan drift requires
  a new preparation and approval rather than opportunistic execution.
- A zero exit code or `submitted` provider response is not verification. The
  independent verify command must return the exact version, channel, and stable
  locator.
- Preserve a failed or ambiguous publish as recovery evidence; do not rerun a
  potentially consequential command under a different identity without
  inspecting provider state.

## Example

An npm project profile runs a local release check, dispatches a GitHub release
workflow, and verifies the exact version from the registry. Runx can prepare a
brief and notes, bind approval to the workflow-dispatch argv, execute it, and
seal only after registry readback names the requested version. If the publish
command changes between preparation and approval or verification cannot find
the version, the run does not claim a successful release.

## Agent task contracts

### `release-notes`

Draft only consumer-facing release notes from the admitted profile, exact version, preparation
checks, last tag, and operator context. Return headline, summary, changelog with
added/fixed/changed/removed/ breaking arrays, upgrade_guidance, and risks. Do not claim a tag,
registry version, deployment, publication, verification, provider acknowledgement, URL, or side
effect.
