# Credential Resolution

Runx skills declare what credential they need. Operators configure material
once, and every entry surface resolves the same contract before execution. An
agent should never guess an environment name, put a token on argv, or wrap a
Runx call with custom dotenv logic.

## Skill contract

Declare named requirements in `X.yaml`, then reference one from each runner
that reaches the provider:

```yaml
credentials:
  nitrosend:
    provider: nitrosend
    audience: https://api.nitrosend.com
    auth:
      api_key:
        delivery:
          env: NITROSEND_API_KEY

runners:
  status:
    default: true
    type: graph
    credential: nitrosend
```

The delivery name is part of the skill contract, not operator setup. A runner's
`environment.required` and `environment.optional` lists are for ordinary,
non-secret runtime configuration. The runtime carries credential delivery
separately and injects it only at the adapter boundary.

A provider may expose more than one auth mode:

```yaml
credentials:
  twitter-read:
    provider: twitter
    auth:
      oauth1_user:
        delivery:
          env: TWITTER_USER_AUTH
      bearer:
        delivery:
          env: TWITTER_BEARER_TOKEN
```

The selected profile's `auth_mode` chooses one declared delivery. If more than
one declared environment value is present without a profile, resolution fails
as ambiguous rather than choosing silently.

## Operator setup

Store a durable local profile by sending the secret on stdin:

```bash
printf '%s' "$NITROSEND_API_KEY" |
  runx credential set nitrosend --profile account-one --from-stdin

printf '%s' "$TWITTER_BEARER_TOKEN" |
  runx credential set twitter \
    --profile twitter-app \
    --auth-mode bearer \
    --from-stdin

printf '%s' "$N8N_WEBHOOK_TOKEN" |
  runx credential set n8n \
    --profile workflow \
    --auth-mode bearer \
    --audience https://n8n.example.com \
    --from-stdin
```

Then run explicitly:

```bash
runx skill ./skills/nitrosend status --profile account-one --json
```

`runx credential set` makes the stored profile the provider's global default.
Reusing a profile name for another provider removes the previous provider's
default only when it still points at that profile. Other defaults are preserved.
Explicit profile selections and project bindings are not rewritten; a provider
mismatch in either remains an error. Without a provider default, resolution
continues through the remaining sources in the order documented below.
`--audience` binds material to one canonical HTTPS host. Use it for providers
whose destination is selected at runtime, such as a self-hosted n8n instance.
When both a skill and a profile declare an audience, their normalized hosts
must match; a profile can narrow an undeclared skill audience but can never
widen a skill-owned binding.
Use a project binding when a workspace should choose a different profile
without repeating `--profile`:

```bash
runx credential bind account-one --provider nitrosend
runx credential bind account-one --skill nitrosend --credential nitrosend
```

The first binding covers every matching provider requirement in the project.
The second is narrower and wins for that named skill requirement.

Inspect or remove configuration without exposing material:

```bash
runx credential list --json
runx credential remove account-one
```

## Resolution order

For a declared runner requirement, Runx resolves exactly once per command in
this order:

1. Explicit `--profile`.
2. Project binding in `<workspace>/.runx/credentials.json`.
3. Provider default in `~/.runx/config.json` (or `RUNX_HOME`).
4. A pre-resolved hosted credential-handle set supplied by the runtime host.
5. The requirement's declared environment name from the workspace snapshot.

Profile, project, and global configuration contain selectors and encrypted
references, not plaintext material. Hosted Connect grants remain provider
execution authority; OSS consumes only pre-resolved opaque handles and never
extracts hosted provider tokens.

## Workspace environment

Every executing Runx CLI command discovers the workspace root and parses its
exact `.env` file as data; help and version rendering do not depend on
workspace state. Runx does not source a shell. Exported process values win;
`.env` fills only missing names. One immutable snapshot is used for the whole
command or MCP server session.

This makes an ignored project `.env` a useful zero-setup development path:

```dotenv
NITROSEND_API_KEY=nskey_live_redacted
```

It is a fallback, not the preferred durable operator setup. Use stored profiles
when one machine operates multiple accounts, project bindings when a repo needs
a stable selection, and hosted handles when a provider grant is managed by a
runtime host.

## Storage and threat model

Local profiles are recorded under the Runx home. Metadata and defaults live in
`config.json`; material is encrypted in private local key files. Project
bindings contain profile names only and may be reviewed or committed when the
names are not sensitive.

This protects against accidental disclosure through config output, repository
files, command history, logs, and receipts. It is not a substitute for an OS
account boundary or a managed secrets service on a compromised machine. Do not
commit `.env`, Runx key files, or credential material.

Secrets never appear in:

- command arguments;
- skill inputs or agent prompts;
- project bindings;
- inspect/readiness output;
- receipts or captured stdout/stderr;
- pause checkpoints or resume answers.

Resume checkpoints persist only the selected profile name. `runx resume`
captures a new workspace snapshot and re-resolves current material, so profile
rotation takes effect immediately.

## Hosted provider grants

Local credential profiles and hosted Connect grants solve different problems.
A profile delivers operator-owned material to a declared local adapter. A
Connect grant keeps provider material in Cloud and authorizes only named
provider operations.

Skills use native `provider.read` and `provider.mutate` tools for Connect. The
runtime authenticates the operator, lists bounded active grant metadata, and
selects the unique grant matching the skill's `expected_provider` and declared
scopes. Reads do not need human approval. A mutation uses that admitted grant
directly unless its typed `provider.mutate.inputs.approval` request asks for an
exact human decision. In that case the native provider effect suspends the run,
binds the decision to the provider plan digest, and resumes only from
host-attested approval. Cloud checks the tool's expected read/mutate
classification against the provider driver's authoritative descriptor before
dispatch; it does not own a second approval policy.

The `runx connect` command manages grant setup, status, listing, and revocation;
it does not expose raw provider invocation. Provider operations stay inside a
skill run so admission, approval when required, projection, and receipt sealing
remain one path.

`runx connect bind <provider> <transport>` records the project's transport
preference. `RUNX_PROVIDER_PERMISSION_TRANSPORT` overrides that project binding.
A complete host-injected `RUNX_PROVIDER_PERMISSION_GRANT_ID`,
`RUNX_PROVIDER_PERMISSION_GRANTED_SCOPES`, and
`RUNX_PROVIDER_PERMISSION_PRINCIPAL_REF` triplet represents hosted authority;
Runx fails preflight when that triplet conflicts with `local:github`, or when its
grant id differs from an exact `hosted:<grant-id>` binding. Resolve the conflict
explicitly instead of allowing ambient CI credentials to silently change the
operator's selected transport.

A skill may also declare `expected_result` to require exact top-level resource
identity fields and `result_fields` to allow only named provider result fields
into the receipt. This is mandatory for secret-adjacent operations and useful
whenever provider evidence must bind to one exact repository, message, post,
charge, or other resource.

`provider.mutate` additionally requires one native `idempotency_key`. Runx
injects that key into the credential-free provider payload and rejects skills
that duplicate it inside `input`, leaving one auditable source for retry
identity.

No grant id is needed when exactly one active grant matches. If multiple grants
match, select one with `RUNX_PROVIDER_PERMISSION_GRANT_ID`; Runx resolves its
current scopes from Connect rather than asking the skill or agent to attest
them. Provider tokens never enter `.env`, skill inputs, command arguments, or
receipts.

## Readiness on every surface

`runx skill inspect ... --json` reports the requirement, supported auth modes,
selected non-secret source/profile, and readiness. A run with no match returns
`status: needs_credential` plus exact setup commands before any provider process
starts.

`runx mcp serve` performs the same check for every served skill at startup and
holds the resulting delivery from its single workspace snapshot. Exported
Claude and Codex shims call `runx skill` directly, so they inherit this behavior
without credential-specific wrapper commands.

Managed-agent and public API tokens use the same stdin rule:

```bash
printf '%s' "$ANTHROPIC_API_KEY" |
  runx config set agent.api_key --from-stdin
```

No Runx secret-setting command accepts raw material on argv.
