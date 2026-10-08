---
name: vault-unseal
description: Plan or execute a scoped, time-bounded vault-unseal request through any compatible vault binding, returning only opaque handle metadata and provider readback.
---

# Vault Unseal

Execute one least-privilege request to make an opaque secret reference available
to one principal for one purpose, scope, and short access window. The default
`execute` runner sends that exact request through a configured vault adapter
after approval and independently reads the resulting handle metadata.
`vault-unseal` remains the explicit plan-only runner. Neither runner retrieves
or reveals secret material.

Use it when a later workload needs a bounded secret handle and the request must
pass a human approval and real vault adapter. Do not use it as a secret store,
credential loader, environment-file parser, or way to put secret values into an
agent context.

## How it works

1. Bind the opaque secret reference to the requesting principal, declared
   purpose, allowed scope, and requested time window.
2. Validate that the reference is opaque and the TTL is between one minute and
   one hour.
3. Reject raw secret-shaped input, ambiguous principals, broad purpose, or
   unbounded scope.
4. Emit an approval-ready request and exact provider handoff.
5. With `execute`, request approval for that exact plan, invoke
   `secret.unseal` under one idempotency key, then call `handle.read` for the
   returned opaque reference. Completion requires both provider operations.

The local plan needs no approval because it exposes no secret and changes no
provider state. The unseal operation is consequential and remains a separate
gate. Credential custody stays in the operator-selected local, self-hosted,
third-party, or Runx-hosted vault binding; the package receives no token and
owns no request client.

## Result and stop conditions

A ready plan contains the bounded request, expiry, approval requirement, and
provider handoff with `provider_status: not_called` and no secret value. A
successful execution additionally carries native mutation and handle-readback
packets. The provider result is required to expose only an opaque `handle_ref`,
expiry, and status metadata; a raw secret is a provider contract violation.
Missing or unsafe inputs return a useful stop packet rather than a guessed
request.

- Never accept or emit raw credentials, key material, tokens, or secret values.
- Refuse TTL below one minute or above one hour.
- Refuse wildcard principals, purposes, scopes, or an opaque ref that cannot be
  distinguished from secret material.
- Refuse a missing, ambiguous, wrong-provider, or under-scoped vault binding;
  never fall back to local environment parsing or a raw vault token.
- Do not claim a handle was issued, mounted, used, or revoked without adapter
  evidence and independent handle readback.

## Example

A deployment runner needs database credentials for fifteen minutes. The skill
can prepare a request binding `vault://prod/db/deployer` to that runner, the
single deployment purpose, and the exact environment scope. `execute` may then
issue an opaque handle after approval and confirm its metadata. It cannot reveal
the password or treat a provider acknowledgement without `handle.read` as
complete.
