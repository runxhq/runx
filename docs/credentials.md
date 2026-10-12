# Credentials

Credentials belong to connectors. A connector manifest declares each
credential it accepts; an operator binds one account to the connector as an
instance; a skill only names the connector operations it calls. The kernel
delivers the instance's credential to the operation it admits, and no skill,
agent prompt, input, or receipt ever holds the material. An agent should never
guess an environment name, put a token on argv, or wrap a Runx call with custom
dotenv logic.

## Connector contract

A connector declares its credentials under `auth.credentials`:

```yaml
auth:
  credentials:
    user:
      kind: oauth1_user
      fields:
        - name: consumer_key
        - name: consumer_secret
        - name: access_token
        - name: access_secret
      delivery:
        env: TWITTER_USER_AUTH
      audience: [api.x.com]
    app:
      kind: bearer_token
      fields:
        - name: token
      delivery:
        env: TWITTER_BEARER_TOKEN
      audience: [api.x.com]
```

Each operation lists the credentials it accepts, and its implementation says
how each one is presented: an HTTP implementation or an MCP server over HTTP by
`auth`, keyed by credential name (`{user: {type: oauth1}, app: {type: bearer}}`);
a process tool or a stdio MCP server in the variable `delivery.env` names. A
skill declares none of this; it lists the connector and its operations under
`connectors:` and calls them from `operation` steps.

For a provider requiring a raw or named header, use the same auth map:

```yaml
auth:
  api_key: {type: header, name: x-api-key, field: api_key}
```

This is the implementation's `auth`, not the manifest's field declaration.
`name` is fixed in the manifest; `field` names a declared credential field.
HTTP and streamable-HTTP MCP use the same presentation boundary. Ordinary
request and session headers cannot override it, and protocol/hop headers cannot
carry credentials. Presented material is secret regardless of the header name.
MCP accepts bearer and named headers; OAuth1 remains per-request HTTP signing.
Query credentials and Basic authentication are not implemented.

A provider whose secret is the endpoint itself, such as a webhook, presents
the credential as the request URL:

```yaml
auth:
  catch_hook: {type: url, field: url}
requests:
  - method: POST
    url: https://hooks.zapier.com/hooks/catch/
```

The request's declared `url` is the prefix the credential's URL must start
with, and the URL is redacted wherever it would be echoed. HTTP only; an MCP
server presents its credential as a header.

A connector that takes no credential may only read. An approval alone never
authorizes a write.

`audience` fixes the hosts a request Runx renders may reach: an HTTP request or
an MCP server URL outside it is refused before anything is sent. A process tool
or stdio MCP server is different. It is a trusted program on the operator's
machine that receives the credential in its environment, and Runx does not
confine where it connects. Runx redacts the credential, and each of its fields,
from everything that process prints. Bind a credential to a process connector
only when you trust that program with the account.

The program is the one the connector ships: a process implementation's tool
lives beside the connector's manifest, at `tools/<segment>/.../manifest.json`
for its dotted name (`git.push` is `tools/git/push/`), and is never looked up
from the skill that calls the operation. A connector that names a tool it does
not ship is refused when it loads. Runx runs each tool from a copy in the runx
home named by the digest of its files, checked against that digest every time,
and the operation's implementation digest in the receipt includes it.

## Binding an account

An instance binds a connector to one account. There are three ways to make
one:

```bash
# A stored credential profile, secret on stdin.
printf '%s' "$TWITTER_USER_AUTH" |
  runx credential set twitter --credential user --from-stdin

# Reuse a profile already stored in Runx home without entering its secret again.
runx connector use twitter --profile twitter --scope twitter:read

# The account a local CLI is signed in to (for example `gh auth token`),
# serving every scope its credential serves, or only those --scope names.
runx connector use github --session --scope repo.read

# An active runx Connect grant.
runx connector use github --grant <grant-id>
```

A plugin's own connectors are bound from inside the plugin: run these
commands in the plugin root or below it, and the instance is recorded in the
workspace store as any other. Its `plugin.json` names the plugin's namespace,
and an instance is recorded for the qualified connector, such as `acme/notes`,
at the exact manifest and connector digests it was confirmed for; a changed
connector needs its instance confirmed again.

`runx credential set` checks the secret against the declaration's `kind` and
`fields`, stores it as a profile (named after the connector unless `--profile`
says otherwise), and selects it as the connector's instance. `--scope` narrows
what the instance may do. A run uses the selected instance and never falls back
to another instance or account.
`runx connector revoke <instance>` disables an instance without deleting the
credential profile or its prior receipts. Reconfirm the profile explicitly to
use it again.

An instance pins what it was confirmed for: the stored material, where the
credential may be sent, what each operation means, and, for a plugin's
connector, that plugin. When any of these changes (the profile is set again
elsewhere, the connector's credential declaration or an operation changes, or
another plugin ships a connector with the same id) the run refuses until the
account is bound again.

Instances live in the project's kernel store (`RUNX_PROJECT_DIR`, else the
workspace's `.runx`), together with the custody of operations in flight. Bind
the account in the same project the runs use.

Inspect or remove profiles without exposing material:

```bash
runx credential list --json
runx credential remove twitter
runx connector instances --json
```

## Readiness

`runx skill inspect ... --json` reports `connector_readiness` for each
operation the selected runner calls: the instance it would use, or the setup
it needs. A runner whose connectors are not bound reports
`readiness.status: needs_connector_setup`, and a run refuses at admission,
before any provider call.

## Workspace environment

Every executing Runx CLI command discovers the workspace root and parses its
exact `.env` file as data; help and version rendering do not depend on
workspace state. Runx does not source a shell. Exported process values win;
`.env` fills only missing names. One immutable snapshot is used for the whole
command or MCP server session. Runx's own `RUNX_` variables never come from
`.env`: a file that sets one is refused, so a workspace cannot redirect Runx's
state, Connect or receipt signing, nor supply authority a host admits.

The snapshot supplies a runner's declared `environment.required` and
`environment.optional` names: ordinary, non-secret configuration. It is not a
credential source; credentials reach operations only through an instance.

## Storage and threat model

Profiles are recorded under the Runx home (`RUNX_HOME`, else `~/.runx`).
Metadata lives in `config.json`; material is encrypted in private local key
files.

This protects against accidental disclosure through config output, repository
files, command history, logs, and receipts. It is not a substitute for an OS
account boundary or a managed secrets service on a compromised machine. Do not
commit `.env`, Runx key files, or credential material.

Secrets never appear in:

- command arguments;
- skill inputs or agent prompts;
- inspect/readiness output;
- receipts or captured stdout/stderr;
- pause checkpoints or resume answers.

A replaced profile has a new material reference. Its previous binding is refused
until the profile is explicitly reconfirmed in that workspace. Reconfirmation
does not authorize a paused or in-flight operation to change its pinned binding;
normal admission and recovery checks still apply.

## Hosted provider grants

A Connect grant keeps provider material in Cloud. An operation on the hosted
placement sends the call to runx Connect, which renders the connector's own
declared request for it and presents the grant's token; it renders nothing a
manifest does not declare. The `runx connect` command manages grant setup,
status, listing, and revocation, and `runx connector use <connector> --grant`
makes an active grant an instance. Delegated OAuth uses the configured gateway;
a stored-key placement loads the exact grant's encrypted material and presents
the manifest's bearer or named header through the existing bounded HTTP path.
Neither path searches for a replacement account when the binding is unavailable.
Connect credential forms for manifest-backed providers are projections of the
same field declarations. Provider-specific verification does not imply account
identity, paid entitlement or permissions the probe did not establish.

A grant is bound to one connector identity: its publisher and id, its manifest
digest and its connector digest. `runx connector use` refuses a grant bound to
any other, and admission reads the grant's status, binding, principal and
scopes from Connect again for every call. Provider tokens never enter `.env`,
skill inputs, command arguments, or receipts.

An operation step may declare `expected_result` to require exact top-level
resource identity fields and `result_fields` to allow only named provider
result fields into the receipt. This is mandatory for secret-adjacent
operations and useful whenever provider evidence must bind to one exact
repository, message, post, charge, or other resource.

## Model and API tokens

Managed-agent and public API tokens use the same stdin rule:

```bash
printf '%s' "$ANTHROPIC_API_KEY" |
  runx config set agent.api_key --from-stdin
```

No Runx secret-setting command accepts raw material on argv.
