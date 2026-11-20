# Domain Connect

This track starts at Runx OSS `a6187abfa50b` (0.12.0). The skill pins its
connector manifests. Stage those exact bytes together. The hosted OAuth
provider entry and staging settings live in the Cloud worktree's
`deploy/cloudflare-dns` directory.

## Skill and connector responsibilities

`domain-connect` owns the workflow. It checks the requested DNS change,
reads existing records, obtains approval for the exact write, and checks
published DNS. A product can supply its TXT or CNAME challenge and use the
result to decide when to run its own verification step.

`cloudflare/dns` owns the Cloudflare API operations and their permission
mapping. The local placement takes a zone-scoped API token. The hosted
placement uses Runx Connect with a grant for the exact zone and connector
revision. Nango handles the Cloudflare OAuth exchange, token storage and
refresh. Runx checks the grant, approval and readback when executing the
skill. For a staged connector and a signed-in Runx operator, the existing
CLI opens authorization with:

```sh
runx connector use cloudflare/dns --hosted \
  --target "cloudflare://zones/${ZONE_ID}" \
  --scope dns.zone.read --scope dns.records.read --scope dns.records.create
```

Cloudflare's consent selects an account and grants Zone Read, DNS Read,
DNS Write and User Details Read for this workflow. Connect records the
signed-in Cloudflare user's stable ID, and the Runx grant narrows DNS access
to one zone and the declared operations. Creating the OAuth application
requires a Cloudflare administrator or an OAuth Client Write credential.

A future Route 53 connector would use the same DNS workflow with its own
operations and authentication. A scoped IAM role with temporary credentials
would bind a hosted zone and its allowed record changes. The Runx grant and
approval flow would still apply. This worktree implements Cloudflare.

## Test the contract

Build `runx-cli` and `runx-js-worker` together from this worktree. Run
`runx test ./skills/domain-connect --json` in an isolated project and Runx
home. The fixture suite uses the real run, approval, connector and receipt
paths. Its hosted provider responses simulate Connect and Cloudflare.

## Qualify on a disposable zone

Use one active zone in a disposable Cloudflare account and two fresh direct
children. Obtain the exact challenge at a test destination for each child.
Record the test zone ID, account ID and authoritative nameservers before
binding a credential.

1. Bind a token scoped to this zone through the local `cloudflare/dns`
   connector. Inject it as `CLOUDFLARE_DNS_TOKEN` in the isolated runner.
   At the terminal, run `runx skill ./skills/domain-connect --inputs input.json`
   and use its native resume prompt for the displayed write. Inspect the
   method for its input fields.
2. On staging, authorize the same connector revision through Runx Connect
   using the private Cloudflare OAuth app. Use a grant targeted to
   `cloudflare://zones/<zone-id>` under the staging hosted runtime. Repeat
   the skill on the second child with its own stable idempotency key.
3. For each run, record the receipt ID, DNS record ID, exact Cloudflare
   readback, independent answer from the authoritative nameservers, and
   the test destination's verification. Check that the repeat run has no
   write and the pre-existing control record is unchanged. Check that
   a revoked grant stops before the provider.
4. If the provider may have accepted a request whose answer was lost,
   inspect the exact name and value in Cloudflare. Keep the run's original
   attempt and idempotency key while resolving the retry decision. Use
   `domain-connect#verify` for propagation checks without write authority.
5. Through the designated test operator, remove only the two known test
   records and revoke the disposable grant and token.

The skill returns DNS state. The destination owns its separate verification
step. Mark the live qualification complete only after both runs include that
evidence.

The live token trial and the staging Connect trial each created and read
back a disposable TXT record on a user-controlled zone. In staging, Cloudflare
consent and Nango's signed callback produced an active grant for the exact
zone. The hosted worker paused at the native approval and resumed with the
approved payload. Both authoritative nameservers returned the TXT. A repeat
returned `dns_visible` and `unchanged`. Cleanup removed the record and revoked
the grant. The hosted proof used an isolated runtime and signer, so retain
the signer's public key alongside its receipts for independent verification.
