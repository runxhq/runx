---
name: domain-connect
description: Connect an owned domain to a service by applying an exact TXT verification record or a DNS-only CNAME in a selected DNS zone, then check whether it is publicly visible.
---

# Domain Connect

Use this when a service supplies an exact TXT challenge or CNAME for an owned domain. Ask the service or use its connector for the required record and the service's final verification. Run the default `connect` method once per record. The method reads the Cloudflare zone and public DNS, checks for conflicts, and pauses for one review of a new record. The review includes the FQDN, type, value, TTL and zone ID.

Bind the DNS connector before running. For hosted sign-in, use the normal Connect authorization and a grant targeted to `cloudflare://zones/<zone-id>`. For local operation, supply a token issued for the selected zone through the existing credential binding. The skill has no access to credential material. It uses the public DNS connector separately for unauthenticated reads.

Use a direct child of the zone for this method. Supply TXT content as the service gives it, without DNS quoting. Use an exact destination hostname for CNAME. If a record already matches, the method reads DNS without writing. An occupied name, truncated record inventory, incompatible delegation or zone mismatch blocks the change. The returned record can be used for manual setup.

Runx reads the new record back separately from the create response. `pending_dns` means the public resolver has yet to return the exact record. Use `verify` to check it again without write authority. `dns_visible` names a public DNS observation. Continue through the destination service's own verifier before reporting the domain as connected. If the write response was lost, inspect exact Cloudflare state before resolving Runx's retry decision.
