# Credentials

Credentials belong to connectors. A connector manifest declares each
credential it accepts and the variable that holds it; an operator binds one
account to the connector as an instance; a skill only names the connector
operations it calls. The kernel delivers the instance's credential to the
operation it admits, and no skill, agent prompt, input, or receipt ever holds
the material. A token never goes on argv, and no Runx call needs custom
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

A provider that reads its key from the request body, such as IndexNow, places
one credential field at a JSON pointer in that body:

```yaml
auth:
  site: {type: body, field: key, pointer: /key}
requests:
  - id: submit
    method: POST
    url: https://api.indexnow.org/indexnow
    body: {host: example.com, urlList: {"$argument": "/urls"}}
```

Every request the operation renders must declare a literal JSON object that
holds the pointer's parent and never sets the pointer itself, and no input the
operation takes may be named by it. A rendered body that sets the member is
refused, never overwritten. The value is redacted wherever the body would be
echoed. Local placements only; an MCP server and a hosted placement present
credentials as headers.

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

An instance binds a connector to one account, and `runx connector use` is the
one place it is made. Six sources can back it:

```bash
# The credential's variable, as the connector declares it, set in the
# environment or the workspace .env. The instance records the variable's
# name, never its value; every call reads it again, so a rotated key needs
# no new binding.
export EXA_API_KEY=...
runx connector use exa

# A secret a secrets manager keeps, read through the manager's own CLI and
# your sign-in to it before every call. The instance records the reference
# and where the read landed, never the value (see Secrets managers and CI).
runx connector use exa --from "aws-sm://team/keys#EXA_API_KEY"

# A secret you give: with its variable unset, use asks for it without
# echoing on a terminal, or reads it from stdin when piped. It is stored as
# a profile, named after the connector unless --profile names it.
runx connector use twitter --credential user --profile work

# An instance, profile or grant confirmed earlier, by its id or name. It
# serves what it was confirmed for; --scope narrows only a new one.
runx connector use twitter work

# The account a local CLI is signed in to (for example `gh auth token`),
# serving every scope its credential serves, or only those --scope names.
# The command comes from the connector's manifest, so binding it takes
# --session; each call through the bound instance runs it again for a token.
runx connector use github --session --scope repo.read

# A new runx Connect grant: prints the authorization link, waits up to two
# minutes, and binds the grant. If it is not authorized yet, use prints the
# command that finishes it. With --json it returns at once with the link and
# that command, for a program to hand on.
runx connector use github --hosted --scope repo.read --target acme/app
```

With nothing named, `runx connector use <connector>` takes the one available
path: an instance it already has, a credential whose variable is set, a stored
profile or a Connect grant. A connector that takes one credential and finds
none of these asks for that credential; one whose credential can only be held
by Connect opens its grant once you are signed in. When there are several paths, it refuses with the
exact form of each. It never picks between accounts for you. An instance bound
to a variable acts as whichever account the variable holds: changing its value
changes the account, as rotating a key does. `--credential <name>` picks among
a connector's credentials; a credential whose variable is set is always bound
to that variable, so `--profile` stores a secret only when the variable is
unset.

A connector that declares a sign-in command is never set up without a choice.
Its manifest can come from the project, so runx neither runs the command to
see whether you are signed in nor takes it by default: `use` lists
`--session` with the command it would run, and only `--session` binds it.
Each call through an instance bound that way runs the command again for a
fresh token. `runx harness` runs the sessions its cases declare, as it runs
the project's other programs.

A variable that the environment and the workspace `.env` set to different
values is refused, when it is bound and on every run: either value could be
another account, and runx never picks one.

A plugin's own connectors are bound from inside the plugin: run these
commands in the plugin root or below it, and the instance is recorded in the
workspace store as any other. A connector names itself, such as `notes` or
`acme/notes`, and an instance is recorded for that name at the exact manifest
and connector digests it was confirmed for; a changed
connector needs its instance confirmed again.

`runx connector use` checks a credential against the declaration's `kind`
and `fields` before it binds it, whether it comes from the variable or is
given, and selects the instance as the connector's. `--scope` narrows what the instance may do, and `--parameter` pins a
value the connector's requests take. A run uses the selected instance and never
falls back to another instance or account.
`runx connector revoke <name>` removes one instance, one profile (and the
instances that hold it) or one grant (and the instances that stand for it).
Prior receipts are kept.

An instance pins what it was confirmed for: the stored material or the
variable, where the credential may be sent, what each operation means, and,
for a plugin's connector, that plugin. When any of these changes (the profile
is set again elsewhere, the connector's credential declaration or an
operation changes, or another plugin ships a connector with the same id) the
run refuses until the account is bound again. An instance bound to a variable
refuses a run in which the variable is not set.

Instances live in the project's kernel store (`RUNX_PROJECT_DIR`, else the
workspace's `.runx`), together with the custody of operations in flight. Bind
the account in the same project the runs use.

See every authority source, or remove one, without exposing material:

```bash
runx connector list --json
runx connector revoke twitter
```

`list` shows each connector in reach with the instance it uses, then every
instance, the local profiles and the signed-in account's Connect grants.

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
`.env` fills only missing names. A name `.env` sets to a different value than
the environment keeps the environment's value, and a connector credential or
model key read from that name is refused until one of them is unset. One
immutable snapshot is used for the whole
command or MCP server session. Runx's own `RUNX_` variables never come from
`.env`, nor do `HOME` and `USERPROFILE`, which locate the runx home: a file
that sets one is refused, so a workspace cannot redirect Runx's state, Connect
or receipt signing, nor supply authority a host admits.

The snapshot supplies a runner's declared `environment.required` and
`environment.optional` names: ordinary, non-secret configuration. It also
holds the variables connector credentials are read from, and those reach an
operation only through an instance that `runx connector use` bound. A runner
that declares a variable any connector in its reach holds a credential in is
refused before it starts, so a skill's declared environment never carries a
connector's credential. That closes the declared route only: a process tool
is a trusted program on the operator's machine and can read what the operator
can, a workspace `.env` included.

## Storage and threat model

Profiles are recorded under the Runx home (`RUNX_HOME`, else `~/.runx`).
Metadata lives in `config.json`; material is encrypted in private local key
files, and the key that encrypts it is a private file in the same home.

This protects against accidental disclosure through config output, repository
files, command history, logs, and receipts. It does not protect against a
process running as you, and it is not a substitute for an OS account boundary
or a secrets manager: to keep a key out of the Runx home entirely, inject it
through the environment (see below). Do not commit `.env`, Runx key files, or
credential material.

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

## Secrets managers and CI

runx reads a secret from where you already keep it. A reference names the
manager and where the secret is; runx keeps the reference, never the value:

```bash
runx connector use exa --from "op://Private/Exa API/credential"
runx connector use exa --from "aws-sm://team/keys#EXA_API_KEY"
runx connector use exa --from "aws-ps:///team/exa-api-key"
runx connector use exa --from "vault://secret/team/exa#key"
runx connector use exa --from "keychain://exa/work"
runx connector use exa --from "file:///run/secrets/exa_api_key"
runx config set api-key --from "aws-sm://team/env#ANTHROPIC_API_KEY"
```

| Reference | Read with | Pinned at bind |
| --- | --- | --- |
| `op://<vault>/<item>[/<section>]/<field>[#field]` | `op read`, 1Password's CLI | the 1Password account (`OP_ACCOUNT`, else `op`'s default) |
| `aws-sm://<name or ARN>[#field]` | `aws secretsmanager get-secret-value` | the secret's ARN (account and region) |
| `aws-ps://<parameter>[#field]` | `aws ssm get-parameter --with-decryption` | the parameter's ARN |
| `vault://<mount>/<path>[#field]` | `vault kv get` (KV version 1 or 2) | the Vault address and namespace |
| `keychain://<service>/<account>` | macOS `security`; Linux `secret-tool` | nothing: one machine |
| `file:///<path>[#field]` | read directly: Docker, Kubernetes and systemd secret mounts | nothing: the path |

- runx runs the manager's own CLI with fixed arguments that no shell
  interprets, found on an absolute `PATH` entry outside the workspace (a
  project cannot stand in for `aws` and receive its sign-in), and gives it
  only the baseline environment plus its own variables (`OP_*`, `AWS_*`,
  `VAULT_*`). Whatever sign-in that CLI supports works: the 1Password app's
  unlock, profiles, SSO, instance and workload roles, GitHub OIDC, Vault
  tokens. No part of a reference a CLI reads may begin with `-`, so none
  reaches the CLI as a flag; `op` takes the whole reference as one argument.
- A reference may hold spaces, as 1Password names items; quote it.
- With the 1Password app's CLI integration, 1Password asks you to approve each
  new terminal session. A runx started without a terminal, as an agent host
  starts it, is asked on each run.
- Binding reads once, checks the value against the connector's declaration,
  and pins where it landed. Every call reads again, at exactly that resource
  or server, so a rotated secret needs no new binding and a switched profile
  cannot serve another account's secret. Nothing is cached, so a `runx mcp`
  session never serves an expired value.
- `#field` selects one field of a JSON secret. A JSON secret without `#field`
  fills a credential of several fields (aws's region, account and keys).
- A failed read refuses. Binding quotes the manager's own message to you; a
  call, whose refusal is recorded, keeps only a form from a fixed list (an AWS
  error code such as `AccessDeniedException`, a Vault HTTP status, a 1Password
  form such as `isn't an item`), never the manager's words.
  `connector list` prints the reference; `list` and `inspect` read nothing.
- runx writes nothing to a manager. Only the operator binds a reference:
  `--from` on the command line. The instance carries a proof made with your
  runx home's key over the whole binding (connector revision, reference, pin,
  scopes, parameters); a project store that names a reference you did not
  bind, or widens one you did, is refused at admission, before anything is
  read. References therefore need a runx home you own: `RUNX_HOME` or `HOME`. A variable or `.env` value that holds a reference
  (`op://...`, `aws-sm://...`), for a connector or the model key, is refused
  rather than sent to a provider as a key.
- `--hosted --from <reference>` seeds a Connect credential grant from the
  manager; Connect then holds it (custody moves to Cloud).

Platforms that inject variables still work through the environment binding:
GitHub Actions secrets, ECS and Kubernetes environment, `op run`, `doppler
run`. A fresh CI runner has no instances, so a job binds its source before it
runs anything, by reference or by variable:

```yaml
env:
  GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
steps:
  - run: runx connector use github --credential token
  - run: runx skill ...
```

An injected variable is read from the environment `runx mcp` started with, so
it is not refreshed during the session; a reference is read again on every
call. Runx runs no OAuth client of its own: refresh belongs to the tool you are
signed in to, through `--session`, or to Connect.

## Hosted provider grants

A Connect grant keeps provider material in Cloud. An operation on the hosted
placement sends the call to runx Connect, which renders the connector's own
declared request for it and presents the grant's token; it renders nothing a
manifest does not declare. `runx connector use <connector> --hosted` opens a
grant (sending the key its hosted placement names, from that key's variable
or asked for, when the placement takes your own), `runx connector use
<connector> <grant-id>` makes an active grant an
instance, `runx connector list` shows the account's grants, and
`runx connector revoke <grant-id>` removes one. Delegated OAuth uses the
configured gateway;
a stored-key placement loads the exact grant's encrypted material and presents
the manifest's bearer or named header through the existing bounded HTTP path.
Neither path searches for a replacement account when the binding is unavailable.
Connect credential forms for manifest-backed providers are projections of the
same field declarations. Provider-specific verification does not imply account
identity, paid entitlement or permissions the probe did not establish.

A grant is bound to one connector revision: its name, its manifest digest and
its connector digest. `runx connector use` refuses a grant bound to
any other, and admission reads the grant's status, binding, principal and
scopes from Connect again for every call. Provider tokens never enter `.env`,
skill inputs, command arguments, or receipts.

An operation step may declare `expected_result` to require exact top-level
resource identity fields and `result_fields` to allow only named provider
result fields into the receipt. This is mandatory for secret-adjacent
operations and useful whenever provider evidence must bind to one exact
repository, message, post, charge, or other resource.

## Model keys and the runx account

The managed agent's model key uses the same stdin rule:

```bash
printf '%s' "$ANTHROPIC_API_KEY" |
  runx config set agent.api_key --from-stdin
```

The key comes from `RUNX_AGENT_API_KEY`, the provider's own variable
(`ANTHROPIC_API_KEY`, `OPENAI_API_KEY` or `GEMINI_API_KEY`), the stored key, or
a reference set with `runx config set api-key --from <reference>`, read on
every run. When more than one is set and they differ, runx refuses and names
them.

A runx account credential is stored by `runx login`, bound to its principal and
API; `RUNX_PUBLIC_API_TOKEN` supplies one to a single process. When both are
present for the same API and differ, runx refuses. No Runx secret-setting
command accepts raw material on argv.
