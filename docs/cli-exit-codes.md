# CLI Exit Codes

Runx uses a small exit-code surface so scripts can branch without parsing
human output.

For `runx skill` and `runx resume`, the signed closure remains available on
stdout regardless of process status. The complete closure matrix is:

| Closure disposition | Exit code |
| --- | ---: |
| `closed` | 0 |
| `deferred` | 2 |
| `superseded` | 1 |
| `declined` | 1 |
| `blocked` | 1 |
| `failed` | 1 |
| `killed` | 1 |
| `timed_out` | 1 |

## Exit Code 0: Sealed

The command completed successfully. A sealed skill result exits 0 only when its
closure disposition is `closed`; Runx preserves every other terminal result as
a signed receipt while returning the process status shown above.

Common follow-up:

```bash
runx history <receipt-id> --json
```

## Exit Code 1: Failure

The command ran but failed, was denied by policy, hit an invalid operation,
found invalid requested output, or sealed a terminal `superseded`, `declined`,
`blocked`, `failed`, `killed`, or `timed_out` closure.

After package and runner resolution, blocked skill preparation writes a refusal
receipt before returning exit code 1. With `--json`, the error includes its
`receipt_id` and prepared-context digest; inspect it with
`runx history <receipt-id>`.

Common fixes:

- Read the stderr message first; it should name the failing command or policy.
- Re-run with `--json` when the command supports it.
- For harness failures, inspect `assertionErrors` in the JSON output.
- For `runx skill owner/name@version`, unsigned manifests, unknown trust keys,
  digest mismatches, and profile digest mismatches fail here before execution.

## Exit Code 2: Resolution Required

The run needs input, approval, or an agent act before it can continue. CLI JSON
uses `needs_approval` when every pending request is an approval and
`needs_agent` for other request sets; both use exit code 2 and the same resume
protocol. In production mode (`RUNX_PRODUCTION=1`), unresolved cognitive work
is treated as a terminal failure but keeps exit code 2 so automation can
distinguish it from ordinary command failure.

Common fixes:

```bash
runx resume <run-id> answers.json
```

Use the pending run's `answers_template`, whose entries are already bound to
each request's `request_digest`. Put agent or task responses in each
`answers` entry's `value`, and explicit human decisions under `approvals`. Only the latter
carries host-attested human approval provenance and can resolve an approval
gate. An agent response under `answers` is rejected at that boundary. The host
must authenticate the human decision; an agent must never author an
`approvals` entry.

For required input, pass the missing `--input` value or the corresponding
kebab-case CLI flag.

## Exit Code 3: Verification Failed

The verification command completed, but the receipt, receipt tree, signature,
or notary evidence did not verify. This is distinct from exit code 1: the
verification machinery ran successfully and produced a verdict.

Common fixes:

- Read the verification findings in stdout, or re-run with `--json`.
- Check the trusted key, receipt lineage, and body/signature digests.
- Use exit code 1 for I/O, parsing, or runtime failures before a verdict exists.

## Exit Code 64: Usage

The command shape is not supported. This usually means the first positional
argument is not a known command or the command is missing its required action.

Common fixes:

```bash
runx --help
runx skill <skill-ref>
runx harness <fixture.yaml>
```

## Exit Code 70: Runtime Setup Failed

Runx could not establish a required process-wide runtime control before
dispatch. Today this means the terminal interrupt handler could not be
installed, so Runx refused to execute without a reliable cancellation path.

Retry from a fresh process. If the failure persists, report the platform and
the complete stderr message.

## Exit Code 130: Interrupted

The operator interrupted the active Runx context with Ctrl-C (or the terminal's
configured interrupt shortcut). Runx terminates supervised tool, JavaScript,
adapter, and MCP child process groups, allows at most two seconds for receipt
and output cleanup, and then exits 130. A second interrupt exits immediately.
On macOS, Cmd-C is normally copy; Ctrl-C is the interrupt.
