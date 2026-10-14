# Security Policy

## Security model

runx keeps local execution state and receipts on your machine. It makes no
telemetry call. Network access occurs only through an explicit surface: registry
fetch/publish, a declared network-capable skill adapter, or an opted-in hosted
connector. The receipt crate itself has no network access by design.

Connectors declare the credentials they accept and the variable each is read
from; operators bind an account to a connector as an instance, from that
variable, a stored profile, a signed-in local session, or a runx Connect grant.
An instance bound to a variable records its name, never its value. A secret
given to `runx connector use` is read without echo at a terminal or from stdin,
never from argv; encrypted local material is never written to manifests, argv,
receipts, or resume checkpoints. A connector's sign-in command comes from its
manifest, which can come from the project, so setup never runs it to look:
binding it takes `--session`, and calls through that binding run it again for
a token. Where two sources supply one
credential with different values (the environment and the workspace `.env`, or a
model key from more than one place), runx refuses rather than pick one. See
[Credentials](docs/credentials.md).

Authority narrows at every hop. A hop's scopes are a subset of the grant it inherits, and widening is denied by construction, so a skill deep in a graph cannot reach past the authority its caller held. Every act produces a signed, reproducible receipt.

Hosted brokerage and the browser connect flow are opt-in and never sit between you and a local run. The result is a small attack surface: most of what runx does has no network edge to attack, and the parts that do are bounded grants you choose to make.

## Supported versions

runx ships from one rolling `cli-vX.Y.Z` release line. Security fixes land on the latest released CLI. There is no separate LTS line yet.

## Reporting a vulnerability

Do not open a public issue for a vulnerability.

Report privately through GitHub's private vulnerability reporting on the repository: open the **Security** tab and choose **Report a vulnerability**. That keeps the report confidential and routes it to the maintainers.

Include enough for us to confirm and fix the issue:

- The affected version (the `cli-vX.Y.Z` you are running).
- Steps to reproduce, with a minimal case if you can.
- The impact: what an attacker gains and under what conditions.

Disclosure is coordinated. We prepare a fix privately, then disclose the issue and the fix together.

## Scope

This policy covers the open-source CLI, the trusted local Rust runtime, and the generated contracts in this repository. The hosted runx service is governed separately and is not covered here.
