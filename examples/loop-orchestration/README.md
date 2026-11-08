# Loop Orchestration

A demo of the pattern the `loop-orchestration` package describes. The package
is in `skill/`. Beside it, `loop-host.mjs` is the outer loop host, an
application script that submits governed runs of that package, and `run.sh`
runs it. The host is not part of the package.

## Run It

```sh
sh examples/loop-orchestration/run.sh
```

The script runs three demonstrations:

- **success:** two governed turns complete a tiny build-review loop;
- **refusal:** a requested tool is outside the loop policy, so the loop stops
  before another turn is submitted;
- **context gate:** an agent turn pauses with digest-bound skill context
  and explicit `allowed_tools`, showing what a host or managed provider would
  receive without requiring a model key.

The output prints each run id, receipt id, decision, and next-turn reason.

## Check The Tests

For the package's repeatable tests, use a fresh receipt store, so its
receipts are not mixed with earlier runs'.

```sh
tmpdir="$(mktemp -d)"
"${RUNX_BIN:-crates/target/debug/runx}" test examples/loop-orchestration/skill \
  --receipt-dir "$tmpdir" \
  --json
```

## Adapting The Example

Replace `loop-host.mjs` with your real orchestrator and keep the same
shape:

```text
load projection -> submit runx run -> read receipt/result -> check stop policy
```

Do not move product-specific scheduling or state into the kernel. If a loop
needs to wake up later, persist the loop state in the host and submit another
runx run when policy allows.
