# Skill Author Runtime Contract

This document defines the lower-level author-visible v1 subprocess ABI for
process tools, `tool: { command: ... }`. Use one only when a capability
genuinely needs an executable or protocol that cannot use Runx's JavaScript
worker boundary. Ordinary package domain logic should declare a worker step,
`tool: { module: ... }`, export a function from a local module, accept the
resolved input object plus its frozen execution context, and return a
JSON-compatible value; Runx then owns every process detail described below.

Internal receipt IDs, artifact IDs, execution-boundary metadata internals, and
temporary paths are not part of this contract unless named here.

For retained execution evidence, use native `receipt.prove` and `receipt.read`,
not a reader over private Runx files. `receipt.read` selects one exact receipt's
declared claim. When that claim must belong to an original composed run, also
pass `root_receipt_id`: the native owner verifies that complete signed tree and
admits only its root or a descendant. The response preserves this root binding.
An unrelated valid receipt is insufficient. Claim access still requires
`receipt.claim.read`, `reach: inputs` and the current workspace's retained state;
it does not expose raw inputs or transport diagnostics. Membership and signatures
do not themselves establish provider truth or successful payment.

## JavaScript module boundary

A pure package module needs no manifest, wrapper, environment parsing, stdout
serialization, or package dependency:

```yaml
steps:
  - id: assess
    tool: { module: risk-model.mjs, export: assessRisk }
    outputs:
      assessment: object
```

```js
export function assessRisk(inputs) {
  return { assessment: evaluate(inputs) };
}
```

Omit `export` to select the default export. Module paths are portable relative
`.mjs` or `.js` paths contained by the owning skill. The selected export may be
sync or async. Runx owns the dedicated no-host worker, input and output framing,
wall enforcement, and failure semantics. The second export argument is a
frozen `{ environment }` object containing only names the worker declares:

```yaml
    tool:
      module: risk-model.mjs
      export: assessRisk
      environment:
        required: [REGION]
        optional: [TRACE_LABEL]
      timeout_seconds: 10
```

Required values fail before the worker starts when unavailable; absent optional
values are omitted. The worker OS environment remains empty. Use this channel
only for non-secret runtime configuration; credentials remain unavailable and
must flow through native credential or provider capabilities. The wall defaults
to two seconds and may be declared from 1 through 30 seconds.

## Volume and artifact pages

Volume changes transport, not execution authority. `runx skill --inputs` may
read one complete control object from a contained UTF-8 JSON file or stdin, but
it does not widen workflow, deterministic-worker, output, credential, or approval
limits. Do not pass an archive, history, or growing completed-id list through
that object merely because the CLI can read it.

For one large immutable JSON-array export, declare a paged worker:

```yaml
scopes: [fs.read]
tool:
  module: archive-selection.mjs
  export: selectPage
  pages:
    path_from: archive_file
    path_scope_from: archive_base
    media_type: application/json
    framing: json_array
    page_bytes: 524288
```

The runtime removes the path and scope fields before module invocation, admits
the contained file to an immutable snapshot, and calls the export repeatedly.
The step must declare `fs.read`. Native admission preserves the caller's bounds:
agent-bound runs refuse secret files and Runx state, including symlink aliases.
Each call receives `runx_page` with `artifact_uri`, media type, whole digest,
source byte count, page index, exact offsets, range digest, `eof`, complete
encoded records, and the prior continuation `state`. An intermediate result may
contain only `{ runx_page: { state, done? } }`; the final call also returns the
declared domain output. A failed page reports its index and byte offset and
cannot be mistaken for an empty page.

For a retained provider array, reuse its native snapshot instead of deriving a
private artifact path. For example, after a GitHub `commits.compare` step:

```yaml
- id: files
  tool: artifact.store
  scopes: [artifact.write]
  inputs: {value: $steps.compare.provider_operation.result.comparison.files}
- id: open
  tool: artifact.open
  scopes: [artifact.read]
  inputs: {artifact: $steps.files.project_artifact}
- id: select
  scopes: [fs.read]
  inputs: {files_source: $steps.open.local_artifact.artifact_uri}
  tool:
    module: review.mjs
    pages: {artifact_from: files_source, framing: json_array, page_bytes: 524288}
```

`artifact_from` names an input containing the opaque URI from `artifact.admit`
or `artifact.open` in this logical run. Its media type and bytes are already bound;
do not also declare `path_from`, `path_scope_from` or `media_type`. The adapter
looks up that immutable snapshot and uses the same bounded page loop. It never
opens a caller-supplied URI as a path. Unknown references and references from a
different logical run or project refuse.
This has the same `fs.read` requirement as `artifact.read`; `artifact.open`
retains its separate operator, scope, root and descriptor checks.

The admitted snapshot survives process exit and pause/resume within the same
logical run, including moving its project state to another path. Continue with
the original reference and offset. Use the retained ProjectArtifact descriptor
for a separate run, where `artifact.open` admits that content under the new run.

The two artifact identities have different jobs. `artifact_uri` is an opaque,
logical-run-scoped reference for reading the admitted snapshot; it survives
runtime restarts and belongs in receipt provenance, not in a
domain plan or idempotency digest. `whole_digest` is the stable content identity
to carry into deterministic domain output. Two runs over identical bytes must
produce the same semantic plan even though their `artifact_uri` values differ.

Artifact admission is capped at 512 MiB. Pages default to 1 MiB and may be
configured up to 4 MiB; continuation state is capped at 2 MiB and one execution
at 4,096 pages. The normal 4 MiB deterministic-worker input/output ceilings
still apply to each call, so the runtime may frame fewer source bytes when JSON
encoding and page metadata would otherwise exceed the worker input boundary. A
single record that cannot fit safely is rejected. These are runtime ceilings,
not manifest profiles. Package code must keep continuation proportional to the
bounded result it is building; durable progress belongs in the store, appended
through the `store` connector's operations and resumed through `after_version` or a projection rather
than an ever-growing array.

Use `artifact.admit`/`artifact.read` directly when a workflow or tool needs exact
byte pages instead of a domain transform. Use `fs.read` and `fs.read_bundle`
only for bounded text. If a format cannot be framed safely or needs genuine
streaming protocol behavior, use one declared trusted-host process tool; never
add ambient filesystem access to a deterministic module or duplicate the
runtime's hashing and page loop in package JavaScript.

## Managed-agent tools

An `agent` step may call only the tools its skill lists in `tools:`; each runs
under the scopes it declares. Before the model runs, Runx resolves every name
through the same native-and-local catalog used for execution. The model receives
the catalog description and exact input schema, including required fields, typed
defaults, and `additionalProperties: false`; Runx does not substitute a
permissive guessed schema. An unresolved tool fails before a provider call.

Invocation then returns through that same catalog path, so a tool cannot be
described from one implementation and executed by another. Native tools retain
their runtime effect, credential, artifact, and receipt boundaries. Local tool
manifests retain the subprocess ABI below. The owning `SKILL.md` and any
declared context-skill manuals provide operating judgment; tool schemas provide
mechanics, not duplicate instructions.

Provider calls need an explicit method `operations: [requirement/operation]`
list. Each entry must belong to the package's connector requirements. An omitted
or empty list offers and admits no direct provider calls in that method's agent
turns. Connector requirements describe workflow dependencies and the validation
ceiling; they do not grant agent permissions. Static `operation` steps retain
their declared behavior, and every provider call retains its existing grant,
scope, credential, approval and readback requirements.

Bundled local tools use
`tools/<namespace>/<tool>/manifest.json`; the dotted manifest name must match
that path. Skill-package admission parses each manifest and binds its complete
static local source closure. Registry publication consumes that admitted set and
does not rescan or reinterpret tool source, so a missing import, uncompiled Node
TypeScript entrypoint, or path/name mismatch fails before execution or publish.

## Permission proof

Method and tool scopes are opaque exact strings. Native capabilities and
provider operations declare the scopes they own, and the production dispatcher
refuses invocation when the selected step does not declare them. The package
test uses that same catalog and dispatcher; a permission-bearing path should
therefore include an admitted case and, where the owner is harnessable, a
withheld-scope or withheld-grant case. There is no test-only permission
evaluator or scope vocabulary.

This is positive enforcement evidence for native capabilities and provider
boundaries. It is not negative syscall evidence for a process tool or a process
MCP server. Those are trusted host code: Runx can prove
the exact invocation and authority it delivered, but not that the process
avoided other host filesystem or network access.

## Process

The runtime starts the declared command with `shell: false` semantics. Arguments
are resolved before spawn, in one pass: `{{name}}` is the input `name` and
`{{env.NAME}}` the declared environment value `NAME`. A filled value is never
read again, and a placeholder that names neither refuses the call before spawn.
The skill process runs with piped stdin, stdout, and
stderr. Stdout and stderr are drained completely while the process runs; each
stream retains a bounded 8 MiB prefix without emitting broken UTF-8. The process
supervisor also counts and hashes the complete stream, so a digest-mode native
command can preserve evidence without retaining an unbounded body. Text and JSON
contracts fail closed when their retained body is truncated.

## Environment

The child environment is deny-by-default. The process tool's
`environment.required` and `environment.optional` declaration admits exact
non-secret host variables. Values are resolved at execution time; a missing
required name stops before spawn and an absent optional name is omitted.
Credential delivery is a separate channel, and a credential variable may not
collide with this environment.

Runx clears the inherited environment and reconstructs a documented
host-interoperability baseline from these names when present. Baseline names
match case-insensitively on Windows and case-sensitively on Unix:

- process launch and user paths: `PATH`, `HOME`, `TMPDIR`, `TMP`, `TEMP`,
  `SystemRoot`, `WINDIR`, `COMSPEC`, `PATHEXT`, `USER`, `LOGNAME`;
- certificates: `CURL_CA_BUNDLE`, `GIT_SSL_CAINFO`, `NODE_EXTRA_CA_CERTS`,
  `REQUESTS_CA_BUNDLE`, `SSL_CERT_DIR`, `SSL_CERT_FILE`;
- locale and terminal behavior: `LANG`, `LANGUAGE`, `LC_ALL`, `LC_CTYPE`,
  `LC_MESSAGES`, `TERM`, `COLORTERM`, `TZ`.

Proxy variables are not part of this baseline because proxy URLs may contain
credentials. A skill that needs a non-secret proxy setting declares its exact
name explicitly. Runtime transport may additionally include `RUNX_CWD`,
`RUNX_RECEIPT_DIR`, and the receipt-verification key id and public key. Private
receipt-signing material and other undeclared secret variables are never
admissible.

Guaranteed variables:

- `RUNX_CWD`: the workspace root, resolved as `RUNX_CWD`, else the nearest workspace root, else the current directory.
- `RUNX_INPUTS_PATH`: path to the complete UTF-8 JSON input object on every invocation.
- `RUNX_INPUTS_JSON`: convenience mirror when the full payload is at most 48 KiB.
- `RUNX_INPUT_<NAME>_PATH`: path to each complete serialized input value on every invocation.
- `RUNX_INPUT_<NAME>`: convenience mirror when that value is at most 8 KiB.

Input env names are normalized by replacing non-alphanumeric runs with `_`,
trimming separators, and uppercasing. For example, `thread.title` becomes
`RUNX_INPUT_THREAD_TITLE`.

The path variables are the stable ABI. Inline variables are optional
conveniences only; payload size never removes the complete path transport.

## Stdin

When `inputs_via` is `stdin`, stdin receives the full input object as JSON and
then closes. Otherwise stdin closes without input.

## Working directory

Relative source cwd values resolve from the skill directory; absolute values
remain absolute. Runx records and supervises that exact directory but does not
claim it confines a trusted host process. Filesystem containment belongs to
native `fs.*` capabilities, not to a subprocess cwd declaration.

Use the exact `cwd: "{{env.RUNX_CWD}}"` runtime token when a subprocess must
operate on the caller's admitted workspace. No other ambient environment value
is interpolated into `cwd`; package-relative paths remain package-relative.

## Timeout

Timeout is terminal. On Unix, the runtime starts the skill in a new process
group, sends `SIGTERM` to the group, then sends `SIGKILL` after a short grace
period. On Windows, the runtime owns the process tree through a per-execution
Job Object and terminates that job on timeout or cancellation.

## Resource limits

On Unix every process tool runs under these limits, each clamped to what the
host already allows:

- 256 open files;
- 512 MiB for any one file the process writes;
- 60 seconds of CPU time;
- on Linux, 16 GiB of data for each process (`ulimit -d`): its heap and the
  private memory it maps for writing, counted when mapped. Address space a
  program only reserves, as V8 does for every WebAssembly memory, does not
  count against it.

Past a limit the operating system refuses the open, write or allocation, or
stops the process when its CPU time runs out; a tool that then exits non-zero
fails its step. Each limit applies to each process on its own. Memory shared
between mappings or spread across child processes is not bounded by them, so a
host that must cap a tool's whole footprint sets that limit itself, with a
container or cgroup.

## Output

A zero exit code without timeout or abort maps to a sealed/success status.
Timeout, abort, spawn failure, or non-zero exit maps to failure. Structured JSON
stdout remains author output; the step parses object stdout into its declared
outputs, but raw stdout and stderr remain visible.

Output ownership is exact. Every result-producing step declares its typed
outputs and packets. A method declares neither method-level `outputs` nor
method-level `artifacts`: the workflow receipt proves the composition, while the
method's result steps (its last step unless `result_from` names others) own the
result and its packet schema. If a method needs one reusable result, add an
explicit package/finalize step instead of wrapping the workflow trace in a second
contract.

## Fixture Gate

`pnpm fixtures:skill-author-runtime:check` runs the shared fixture entrypoint
through the Rust runtime's process tool. The gate compares only
author-visible behavior: status, stdout/stderr, exit code where relevant,
parsed stdout JSON, cwd relation, input delivery mode, output truncation, and
descendant timeout cleanup.
