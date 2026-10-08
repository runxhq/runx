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
it does not widen graph, deterministic-worker, output, credential, or approval
limits. Do not pass an archive, history, or growing completed-id list through
that object merely because the CLI can read it.

For one large immutable JSON-array export, declare a paged worker:

```yaml
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
Each call receives `runx_page` with `artifact_ref`, media type, whole digest,
source byte count, page index, exact offsets, range digest, `eof`, complete
encoded records, and the prior continuation `state`. An intermediate result may
contain only `{ runx_page: { state, done? } }`; the final call also returns the
declared domain output. A failed page reports its index and byte offset and
cannot be mistaken for an empty page.

The two artifact identities have different jobs. `artifact_ref` is an opaque,
runtime-local capability for reading the admitted snapshot; it deliberately
changes across runtime instances and belongs in receipt provenance, not in a
domain plan or idempotency digest. `whole_digest` is the stable content identity
to carry into deterministic domain output. Two runs over identical bytes must
produce the same semantic plan even though their `artifact_ref` values differ.

Artifact admission is capped at 512 MiB. Pages default to 1 MiB and may be
configured up to 4 MiB; continuation state is capped at 2 MiB and one execution
at 4,096 pages. The normal 4 MiB deterministic-worker input/output ceilings
still apply to each call, so the runtime may frame fewer source bytes when JSON
encoding and page metadata would otherwise exceed the worker input boundary. A
single record that cannot fit safely is rejected. These are runtime ceilings,
not manifest profiles. Package code must keep continuation proportional to the
bounded result it is building; durable progress belongs in the store, appended
through `data-store` and resumed through `after_version` or a projection rather
than an ever-growing array.

Use `artifact.admit`/`artifact.read` directly when a graph or tool needs exact
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

Bundled local tools use
`tools/<namespace>/<tool>/manifest.json`; the dotted manifest name must match
that path. Skill-package admission parses each manifest and binds its complete
static local source closure. Registry publication consumes that admitted set—it
does not rescan or reinterpret tool source—so a missing import, uncompiled Node
TypeScript entrypoint, or path/name mismatch fails before execution or publish.

## Permission proof

Runner and tool scopes are opaque exact strings. Native capabilities and
provider operations declare the scopes they own, and the production dispatcher
refuses invocation when the selected step does not declare them. The package
harness uses that same catalog and dispatcher; a permission-bearing path should
therefore include an admitted case and, where the owner is harnessable, a
withheld-scope or withheld-grant case. There is no harness-only permission
evaluator or scope vocabulary.

This is positive enforcement evidence for native capabilities and provider
boundaries. It is not negative syscall evidence for a process tool or a process
MCP server. Those are trusted host code: Runx can prove
the exact invocation and authority it delivered, but not that the process
avoided other host filesystem or network access.

## Process

The runtime starts the declared command with `shell: false` semantics. Arguments
are resolved before spawn. The skill process runs with piped stdin, stdout, and
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
host-interoperability baseline from these names when present:

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

When `input_mode` is `stdin`, stdin receives the full input object as JSON and
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

## Output

A zero exit code without timeout or abort maps to a sealed/success status.
Timeout, abort, spawn failure, or non-zero exit maps to failure. Structured JSON
stdout remains author output; the step parses object stdout into its declared
outputs, but raw stdout and stderr remain visible.

Output ownership is exact. Every result-producing step declares its typed
outputs and packets. A runner declares neither runner-level `outputs` nor
runner-level `artifacts`: the graph receipt proves the composition, while the
runner's result steps (its last step unless `result_from` names others) own the
result and its packet schema. If a runner needs one reusable result, add an
explicit package/finalize step instead of wrapping the graph trace in a second
contract.

## Fixture Gate

`pnpm fixtures:skill-author-runtime:check` runs the shared fixture entrypoint
through the Rust runtime's process tool. The gate compares only
author-visible behavior: status, stdout/stderr, exit code where relevant,
parsed stdout JSON, cwd relation, input delivery mode, output truncation, and
descendant timeout cleanup.
