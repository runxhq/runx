# Agent Skills: runx Inside Claude and Codex

runx skills are governed by the runx runtime, not by a model following prose.
`runx export` makes those governed skills available *inside* an agent such as
Claude Code or Codex: each runx skill becomes a native agent skill whose only
job is to call the runx binary. The agent brings the judgment; runx admits the
authority, performs the act, and returns a signed receipt.

## How It Works

`runx export <claude|codex>` generates orchestrator-native files that delegate
back to runx:

- **Claude** gets one `SKILL.md` shim per skill under `~/.claude/skills/<name>/`
  (or `./.claude/skills/` with `--project`). Each shim declares `allowed-tools`
  locked to the runx binary and carries a package- and execution-closure-bound
  `runx skill ... --json` command. The exact runner schema stays inspectable on
  demand instead of being copied into every agent context.
- **Codex** gets the same per-skill shims under `~/.agents/skills/<name>/` plus an
  idempotent managed allow block in `~/.codex/rules/default.rules`. Project exports
  use `<repo>/.agents/skills` and do not create approval rules.

The repository-root `runx` guide is the one exception to delegation: exporting
it copies its native operator instructions instead of recursively calling
`runx skill <runx-repository>`. Provider/domain skills still delegate through
the runtime.

When the agent runs the skill it shells out to runx. Execution, authority
admission, approvals, and the signed receipt all happen inside the runtime, so
the governance is real rather than narrated.

## Export

```bash
runx export claude                          # current workspace's public skills -> ~/.claude/skills
runx export claude --project                # -> ./.claude/skills (checked into a repo)
runx export claude nws-weather pay   # only the named skills
runx export codex                           # ~/.agents/skills plus managed rules
runx export codex --project                 # ./.agents/skills; no approval rules
pnpm codex-export:check                     # verify the active managed Codex surface
```

Add `--json` for machine-readable output. A governed package must be public and
have a default runner with a validated standalone operator-journey contract and
an exactly bound execution closure. Public visibility alone is insufficient.
Internal runtime paths, harness fixtures and composed-only stages are skipped;
explicitly naming an ineligible package returns an error.

A full export must run from a skill workspace containing a root `SKILL.md` or
`skills/`. It synchronizes that workspace's managed catalog and removes only
stale generated shims sourced from the same workspace; exports owned by other
repositories remain intact. An unrelated directory fails closed without
changing existing shims. Supplying one or more skill refs is additive: Runx
updates those shims without removing any other managed skills.

## What A Claude Shim Looks Like

`runx export claude` writes a compact shim whose important structure looks like
this for the `pay` skill:

````markdown
---
name: pay
description: Pay one external payment requirement within exact parent authority, through a payment method the payer holds, and read the paid resource back.
allowed-tools: Bash(/path/to/runx skill *) Bash(/path/to/runx resume *)
---
# pay - governed by runx

Run the declared runner through runx; do not bypass it by independently reproducing work that runner owns.

<!-- runx-source-manual-begin digest=sha256:<manual> package-digest=sha256:<package> bytes=<n> -->
<the exact canonical SKILL.md body>
<!-- runx-source-manual-end -->

## Default runner: `pay`

Inspect the exact input and effect contract on demand:

```bash
/path/to/runx skill inspect /path/to/skills/pay pay --json
```

```bash
printf '%s' "$RUNX_INPUTS_JSON" | /path/to/runx skill /path/to/skills/pay pay \
  --inputs - \
  --act "$RUNX_ACT_JSON" \
  --package-digest sha256:<package> \
  --execution-closure-digest sha256:<closure> \
  --json
```

Build the input object from the user's task and the inspected input_schema.
The act states the requested purpose. Harness inputs are test data, not a job
to copy. Authored runner examples remain available through inspection.

<!-- runx-export:claude source=/path/to/skills/pay package-digest=sha256:<package> - generated, do not edit -->
````

For Claude, the `allowed-tools` line limits the shim to the runx binary. Codex
uses the generated rules block. Run and resume commands carry both exact
digests, so edited package bytes or a changed execution closure fail before
execution. Runx remains the authority boundary on both surfaces.

## Continuing a workflow

A `needs_agent` result is an open workflow, not a failed command. The shim
instructs the host to read the returned digest-bound request artifact and match
its exact output schema. `runx resume --help` documents the bound answer
envelope: each full request id maps to its `request_digest` and `value`.
Human questions and approvals remain human decisions; an agent must not invent
their answers or approve its own work.

A non-default runner uses its own inspected closure digest on invocation and
resume.

Only a sealed run with `outcome: completed` establishes completion. Provider
effects additionally require the returned provider evidence. A blocked or
failed receipt must be reported as such.

## Skills whose agent makes calls

Some agent turns do more than answer: they read files, call a connector or run
a child skill, and each of those calls has to be admitted and recorded. Those
turns run on an MCP connection, so `runx export` registers one local server,
`runx-local`, naming every exported skill that needs it. Claude Code registers it
through its own `claude mcp add`; Codex gets it in `config.toml`, with startup and
tool timeouts long enough for this server. Start a new host session after
exporting, since hosts load servers when a session starts. Codex reads a project's
`.codex/config.toml` only once that project is trusted. If the host's CLI is not
installed, the export report prints the command to run.

Such a skill's generated instructions name its tool rather than a shell command.
A default runner's tool has the skill's name, and another runner's is
`<skill>__<runner>`. The host calls the tool, then works the run's open turns on
the same connection with `turn_list`, `turn_status`, `turn_read`, `turn_call` and
`turn_complete`. The server binds each package and closure when it starts, and it
works only the runs its own calls opened. Codex asks for approval of each MCP tool
unless you allow it in `config.toml`.

A host that opens its own connections, such as an app that hands an agent one
paused run, uses `runx mcp --run <run-id>` instead. Keep the same working project
and Runx home throughout a run, and use one driver at a time.

## Requirements

- **The runx binary.** The shim calls runx by path, so that binary must be
  present.
- **Receipt identity.** Local runs sign with the runx home's own key, created
  on the first run. Hosted and CI execution must configure the complete
  `RUNX_RECEIPT_SIGN_*` tuple; a partial tuple fails closed. See
  [Getting Started](./getting-started.md#production-receipt-signing).
- **Provider accounts.** Bind them once as connector instances with
  `runx connector use`. Exported shims do not add
  credential wrappers; the invoked Runx skill performs the canonical readiness
  check. See [Credentials](./credentials.md).

## Regenerating

Rerun `runx export` after you add, rename, or remove skills; the shims are
generated, so do not hand-edit them. If a shim's source skill moves, the stale
shim fails closed and instructs you to rerun the export, so a renamed skill
never silently runs the wrong thing. A full export also replaces digest-stale
Runx-managed shims from the same source workspace and prunes only obsolete
entries bearing that workspace's managed marker.

## Portability

A shim bakes the runx binary path resolved at export time. Exported from a
source checkout it points at the local debug build; export with the published
CLI on your `PATH` (the `@runxhq/cli` global) so the shim is portable across
machines.

## The General Agent Bridge

Per-skill exports are the right call for governed skills. The exported root
`runx` guide is the looser discovery bridge: it teaches the agent to find skills
in the [catalog](https://runx.ai/x), run them, and read the receipts without
wrapping the runtime in itself.

Generated shims bind the complete source package. Calling an exported shim
directly resolves its source through the current Runx workspace and refuses a
stale package digest; regenerate the export after changing source bytes. Global
exports use an absolute local source path; project exports use a path relative
to the project when the source is inside it. Exporting a local cache does not
publish that package or make the cache available on another machine.
