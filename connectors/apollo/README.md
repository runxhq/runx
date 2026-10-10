# Apollo

The initial connector inspects the exact selected key through Apollo's
`auth/health` endpoint. Bind an API key locally through native credential
custody or select a stored key through Connect. The shared header presentation
contract supplies `x-api-key`; neither the caller nor a skill handles the key.

Treat the key as authenticated only when both `healthy` and `is_logged_in` are
true. HTTP 200 alone is insufficient. This probe does not establish account
identity, API permissions, enrichment entitlement or credit balance.

Paid person enrichment remains unimplemented until operation cost and retry
admission are available. A generic read retry must not spend enrichment credits
again after an ambiguous response. No live account has been connected or tested
by this source addition.

Provider reference: [Apollo API authentication](https://docs.apollo.io/docs/api-authentication).
