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

`audience` fixes the hosts a request Runx renders may reach: an HTTP request or
an MCP server URL outside it is refused before anything is sent. A process tool
or stdio MCP server is different. It is a trusted program on the operator's
machine that receives the credential in its environment, and Runx does not
confine where it connects. Runx redacts the credential, and each of its fields,
from everything that process prints. Bind a credential to a process connector
only when you trust that program with the account.

## Binding an account

An instance binds a connector to one account. There are three ways to make
one:

```bash
# A stored credential profile, secret on stdin.
printf '%s' "$TWITTER_USER_AUTH" |
  runx credential set twitter --credential user --from-stdin

# Reuse a profile already stored in Runx home without entering its secret again.
runx connector use twitter --profile twitter --scope twitter:read

# The account a local CLI is signed in to (for example `gh auth token`).
runx connector use github --session

# An active runx Connect grant.
runx connector use github --grant <grant-id>
```

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

Instances live with the receipt root (`RUNX_RECEIPT_DIR`, else
`.runx/receipts`), together with the custody of operations in flight. Bind the
account under the same receipt root the runs use.

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
command or MCP server session.

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

A resumed run reads the instance's current material, so profile rotation takes
effect immediately.

## Hosted provider grants

A Connect grant keeps provider material in Cloud. An operation on the hosted
placement sends the call to runx Connect, which renders the connector's own
declared request for it and presents the grant's token; it renders nothing a
manifest does not declare. The `runx connect` command manages grant setup,
status, listing, and revocation, and `runx connector use <connector> --grant`
makes an active grant an instance.

A host may inject a grant instead: a complete `RUNX_PROVIDER_PERMISSION_GRANT_ID`,
`RUNX_PROVIDER_PERMISSION_GRANTED_SCOPES` (a JSON array), and
`RUNX_PROVIDER_PERMISSION_PRINCIPAL_REF` triplet stands for a hosted instance
of every connector with a hosted placement. A partial triplet is refused.
Provider tokens never enter `.env`, skill inputs, command arguments, or
receipts.

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
