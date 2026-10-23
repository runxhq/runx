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

Declared HTTP operations can exchange a client ID and secret with
`type: oauth2_client_credentials`. `token_url` accepts a literal or a template
over admitted operation arguments and local instance parameters. The rendered
endpoint must use HTTPS on an exact declared host and credential audience,
without user information, a query, a fragment or a nonstandard port. This is
checked before any app credential is sent. Hosted operations cannot bind local
parameters. Exchange responses and other runtime outputs cannot select a token
endpoint. Tokens remain private, have bounded cache lifetimes, and are forgotten
after a provider rejects authentication; a failed mutation is never retried
automatically with a fresh token.

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

A CLI that keeps its own login can declare `auth.session.kind:
managed_command`. Its fixed command returns public JSON; `id` and `login` name
dot-separated paths in that answer, including numeric array segments. Binding
with `--session` pins the stable principal and scopes. Native admission probes
it again before every operation without reading, exporting or refreshing its
token. Only process implementations may use this session. Their kernel-supplied
`principal_uri` identifies the pinned account and cannot come from operation
inputs. The trusted process must enforce that identity at credential use,
including account ownership of any opaque provider reference. A preliminary
account probe alone does not prevent a later account switch.

Managed sessions do not make process output safe automatically: their process
must project public results and suppress raw credential-bearing errors. Link's
process leaves SPT retrieval inside its official CLI and returns only public
spend and transaction references. Runx does not claim an OS sandbox around the
CLI or manage a second copy of its session.

The program is the one the connector ships: a process implementation's tool
lives beside the connector's manifest, at `tools/<segment>/.../manifest.json`
for its dotted name (`git.push` is `tools/git/push/`), and is never looked up
from the skill that calls the operation. A connector that names a tool it does
not ship is refused when it loads. Runx runs each tool from a copy in the runx
home named by the digest of its files, checked against that digest every time,
and the operation's implementation digest in the receipt includes it.

## Binding an account

An instance binds a connector to one account. A connector whose credential's
variable is set needs none made: with no instance selected, a run is bound
to that variable exactly as `runx connector use` would bind it, and nothing
is stored. Two of a connector's credentials set at once could be two
accounts, so that run refuses and names both, and `--credential` picks one;
a connector that pins parameters is set up once with them. Otherwise `runx
connector use` is the one place an instance is made. Six sources can back it:

```bash
# The credential's variable, as the connector declares it, set in the
# environment or the workspace .env. No secret and no setup: a run reads it
# on every call, so a rotated key needs nothing. `use` records the same
# binding when you want it selected and listed.
export EXA_API_KEY=...
runx skill ...                 # exa runs as EXA_API_KEY
runx connector use exa         # the same binding, selected and listed

# A secret you keep in runx (see Secrets), sealed in the runx home or read
# through a secrets manager. The instance records the secret's name, never
# its value; every call reads it again.
runx secret set exa --from "aws-sm://team/keys#EXA_API_KEY"
runx connector use exa --secret exa

# A value you give: with its variable unset, use asks for it without echoing
# on a terminal, or reads it from stdin when piped, and keeps it as a secret
# named after the connector (`<connector>-<credential>` for a connector of
# several credentials). A secret already set under that name is never taken:
# bind it with --secret.
runx connector use twitter --credential user

# An instance or grant confirmed earlier, by its id. It serves what it was
# confirmed for; --scope narrows only a new one.
runx connector use twitter secret.4f2a...

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
path: an instance it already has, a credential whose variable is set, or a
Connect grant. A connector that takes one credential and finds none of these
asks for that credential; one whose credential can only be held by Connect
opens its grant once you are signed in. When there are several paths, it
refuses with the exact form of each. It never picks between accounts for you,
and never takes a secret by its name: `--secret` names it. An instance bound to
a variable or a secret acts as whichever account it holds: changing its value
changes the account, as rotating a key does. `--credential <name>` picks among
a connector's credentials; without `--secret`, a credential whose variable is
set is bound to that variable.

A connector that declares a sign-in command is never set up without a choice.
Its manifest can come from the project, so runx neither runs the command to
see whether you are signed in nor takes it by default: `use` lists
`--session` with the command it would run, and only `--session` binds it.
Each call through an instance bound that way runs the command again for a
fresh token, or the public identity of a managed session. `runx harness` runs the sessions its cases declare, as it runs
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
and `fields` before it binds it, whether it comes from the variable, a secret
or is given, and selects the instance as the connector's. `--scope` narrows
what the instance may do, and `--parameter` pins a value the connector's
requests take. A run uses the selected instance and never falls back to
another instance or account. `runx connector revoke <id>` removes one instance
or one grant (and the instances that stand for it); a secret is removed with
`runx secret rm`. Prior receipts are kept.

An instance pins what it was confirmed for: the variable or the secret's
name, where the credential may be sent, what each operation means, and, for a
plugin's connector, that plugin. When any of these changes (the connector's
credential declaration or an operation changes, or another plugin ships a
connector with the same id) the run refuses until the account is bound again.
An instance bound to a variable refuses a run in which the variable is not
set, and one bound to a secret refuses once the secret is removed. A secret
set again serves its instances from their next call, without binding again.

runx 0.11 keeps its kernel store in `.runx/kernel-v2.sqlite` and reads
nothing runx 0.10 recorded in `.runx/kernel.sqlite`, so each project binds its
connectors again once. While the 0.10 store still holds a provider effect in
flight, 0.11 refuses to start the new store: finish or reconcile that run with
runx 0.10, or move the old file aside to discard it.

Instances live in the project's kernel store (`RUNX_PROJECT_DIR`, else the
workspace's `.runx`), together with the custody of operations in flight. Bind
the account in the same project the runs use.

A run a host starts carries its instances in the host's run authority instead:
`grants` names, for each connector, the runx Connect grant its calls run under,
as in `--run-authority '{"grants":{"acme/notes":"grant_..."}}'`. Runx reads the
grant through the host, so it must bind the connector's exact revision, be
active and be held by the run's principal; the host names it and never asserts
what it covers. A stated grant stands only for a hosted placement, and a
different instance selected for the same connector is refused, never chosen.
`runx skill inspect <skill> <runner> --run-authority` projects readiness under
the same statement, so a host knows whether a run can be admitted before it
starts one.

See every authority source, or remove one, without exposing material:

```bash
runx connector list --json
runx connector revoke secret.4f2a...
runx secret list
```

`connector list` shows each connector in reach with the instance it uses, then
every instance and the signed-in account's Connect grants; `secret list` shows
the secrets the instances name.

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
operation only through an instance: the one `runx connector use` bound, or,
with none selected, the variable's own binding. A runner
that declares a variable any connector in its reach holds a credential in is
refused before it starts, so a skill's declared environment never carries a
connector's credential. That closes the declared route only: a process tool
is a trusted program on the operator's machine and can read what the operator
can, a workspace `.env` included.

## Secrets

`runx secret` is the one place a secret is kept: by name, in the runx home.
A connector binding, the managed agent and a program you run each name it,
and none of them holds its value.

```bash
runx secret set github                 # asks without echo, or reads stdin
runx secret set exa --from "op://Private/Exa API/credential"
runx secret list                       # names and where each value is
runx secret read gemini                # prints the value, for a program
runx secret rm github
```

`set` takes the value from a prompt that does not echo it, or from stdin when
piped, never from an argument; or, with `--from`, a reference to where a
secrets manager keeps it (see Secrets managers and CI). Setting a secret again
serves everything that names it from its next call: rotation is one command,
in `runx mcp` too. A secret acts as the account its value holds, as a variable
does. `rm` removes it; whatever still names it, in any project or the agent's
config, refuses until it is set again. `read` prints the value, as `gh auth
token` and `op read` print theirs, for a host that sends it itself, such as
Fieldwork's model keys through `@runxhq/client`'s `secrets.read`.

A variable set in the environment or the workspace `.env` needs no secret and
no setup: a connector whose credential it holds runs as it with no instance
selected, reading it on every call, and the managed agent reads its
provider's variable. Keep a key in a secret when it should live in one place
you rotate, or in a secrets manager.

## Storage and threat model

Secrets are recorded under the Runx home (`RUNX_HOME`, else `~/.runx`). Their
names live in `config.json`; a stored value is sealed in a private local key
file, and the key that seals it is a private file in the same home. A
referenced value never touches the home.

This protects against accidental disclosure through config output, repository
files, command history, logs, and receipts. It does not protect against a
process running as you, and it is not a substitute for an OS account boundary
or a secrets manager. A project's files can never choose, bind, read or
configure a secret: a binding carries a proof only your runx home can make,
and a manager's CLI never sees what a workspace `.env` sets. A program you run
is another matter. A skill's process tool runs as you, with `HOME` and `PATH`,
so it can read what you can, the runx home, `~/.aws` and `gh auth token`
included; only an OS sandbox around process tools would change that. Do not
commit `.env`, Runx key files, or credential material.

Secrets never appear in:

- command arguments;
- skill inputs or agent prompts;
- inspect/readiness output;
- receipts or captured stdout/stderr;
- pause checkpoints or resume answers.

Only `runx secret read` prints one, when you ask it to.

## Secrets managers and CI

runx reads a secret from where you already keep it. A reference names the
manager and where the secret is; the runx home keeps the reference and what
reading it pinned, never the value:

```bash
runx secret set exa --from "op://Private/Exa API/credential"
runx secret set exa --from "aws-sm://team/keys#EXA_API_KEY"
runx secret set exa --from "aws-ps:///team/exa-api-key"
runx secret set exa --from "vault://secret/team/exa#key"
runx secret set exa --from "keychain://exa/work"
runx secret set exa --from "file:///run/secrets/exa_api_key"
runx secret set anthropic --from "aws-sm://team/env#ANTHROPIC_API_KEY"
```

| Reference | Read with | Pinned when set |
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
  `VAULT_*`). Both come from the environment runx was started with, never
  from a name the workspace `.env` filled, and the CLI runs in the runx home,
  not the workspace: a project's files never sign a manager in, configure it
  (an `AWS_CONFIG_FILE` whose `credential_process` runs a command) or choose
  what it trusts (`SSL_CERT_FILE`). Set `AWS_PROFILE`, `OP_ACCOUNT` or
  `VAULT_ADDR` in your shell, not in a project's `.env`. Whatever sign-in that
  CLI supports works: the 1Password app's
  unlock, profiles, SSO, instance and workload roles, GitHub OIDC, Vault
  tokens. No part of a reference a CLI reads may begin with `-`, so none
  reaches the CLI as a flag; `op` takes the whole reference as one argument.
- A reference may hold spaces, as 1Password names items; quote it.
- With the 1Password app's CLI integration, 1Password asks you to approve each
  new terminal session. A runx started without a terminal, as an agent host
  starts it, is asked on each run.
- Setting the secret reads it once and pins where it landed; binding it reads
  it again and checks the value against the connector's declaration. Every
  call reads again, at exactly that resource or server, so a rotated secret
  needs no new binding and a switched profile cannot serve another account's
  secret. Nothing is cached, so a `runx mcp` session never serves an expired
  value.
- `#field` selects one field of a JSON secret. A JSON secret without `#field`
  fills a credential of several fields (aws's region, account and keys).
- A failed read refuses. Setting or binding the secret quotes the manager's
  own message to you; a call, whose refusal is recorded, keeps only a form
  from a fixed list (an AWS error code such as `AccessDeniedException`, a
  Vault HTTP status, a 1Password form such as `isn't an item`), never the
  manager's words. `secret list` prints the reference and its pin; `secret
  list`, `connector list` and `connector inspect` read nothing.
- runx writes nothing to a manager. Only you set a reference: `runx secret
  set --from` on the command line. An instance names the secret and carries a
  proof made with your runx home's key over the whole binding (connector
  revision, secret, scopes, parameters); a project store that names a secret
  you did not bind, or widens a binding you made, is refused at admission,
  before anything is read. A secret binding therefore needs a runx home you
  own: `RUNX_HOME` or `HOME`. A variable or `.env` value that holds a
  reference (`op://...`, `aws-sm://...`), for a connector or the model key, is
  refused rather than sent to a provider as a key.
- `--hosted --secret <name>` seeds a Connect credential grant from the secret.
  Connect keeps its own copy: the secret stays as it is here, and setting it
  again later does not change Connect's.

Platforms that inject variables work through the environment binding with no
setup: GitHub Actions secrets, ECS and Kubernetes environment, `op run`,
`doppler run`. A fresh CI runner has no instances, so each connector runs as
its credential's variable:

```yaml
env:
  GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
steps:
  - run: runx skill ...
```

An injected variable is read from the environment `runx mcp` started with, so
it is not refreshed during the session; a secret is read again on every call.
Runx runs no OAuth client of its own: refresh belongs to the tool you are
signed in to, through `--session`, or to Connect.

## Hosted provider grants

A Connect grant keeps provider material in Cloud. An operation on the hosted
placement sends the call to runx Connect, which renders the connector's own
declared request for it and presents the grant's token; it renders nothing a
manifest does not declare. `runx connector use <connector> --hosted` opens a
grant (sending the key its hosted placement names, from `--secret`, that
key's variable or asked for, when the placement takes your own), `runx connector use
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

The managed agent's model key is a secret the agent names, or a variable:

```bash
runx secret set anthropic
runx config set agent.secret anthropic
```

The key comes from `RUNX_AGENT_API_KEY`, the provider's own variable
(`ANTHROPIC_API_KEY`, `OPENAI_API_KEY` or `GEMINI_API_KEY`), or the secret
`agent.secret` names, read on every run; `runx config unset agent.secret`
stops naming it. When more than one is set and they differ, runx refuses and
names them.

A runx account credential is stored by `runx login`, bound to its principal and
API; `RUNX_PUBLIC_API_TOKEN` supplies one to a single process. When both are
present for the same API and differ, runx refuses. No Runx secret-setting
command accepts raw material on argv.
