# Apollo

The connector inspects the exact selected key through Apollo's `auth/health`
endpoint and declares person and company enrichment. Bind an API key locally through native credential
custody or select a stored key through Connect. The shared header presentation
contract supplies `x-api-key`; neither the caller nor a skill handles the key.

Treat the key as authenticated only when both `healthy` and `is_logged_in` are
true. HTTP 200 alone is insufficient. This probe does not establish account
identity, API permissions, enrichment entitlement or credit balance.

`person.enrich` matches a professional by an Apollo ID, email, LinkedIn profile,
or name plus employer evidence. It does not request personal-email, phone or
waterfall expansion. `company.enrich` identifies an organization by domain,
LinkedIn profile or website; a name alone is insufficient identity. Both return
the provider's evidence, including no-match or incomplete data; HTTP 200 alone
does not establish a confident match or usable email address.

These operations may consume account credits. Their cost declarations preserve
the unknown USD quote instead of inventing a conversion. Native admission needs
bounded-use authority or one exact approval; a strict dollar budget requires an
actual monetary quote. Ambiguous responses remain in native custody and cannot
trigger another unapproved charge. They require the runtime cost/charge-custody
contract; do not publish them ahead of its verified CLI distribution.

Nitrosend's platform key has its own signup-enrichment purpose policy. This
connector does not expand that policy or grant other tenants access to it.
Operators bind their own appropriately authorized account. Source implementation
and offline fixtures do not establish live API permissions, credits or results.

Provider references: [authentication](https://docs.apollo.io/reference/authentication),
[person enrichment](https://docs.apollo.io/reference/people-enrichment),
[company enrichment](https://docs.apollo.io/reference/organization-enrichment).
