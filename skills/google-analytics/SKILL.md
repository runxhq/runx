---
name: google-analytics
description: Read and normalize GA4 properties, metadata, standard reports, and realtime reports through any compatible Runx connector, preserving the measurement caveats an operator needs before turning analytics into action.
---

# Google Analytics

Use this skill for bounded GA4 evidence: finding accessible properties, checking
the dimension and metric catalog, measuring acquisition and on-site outcomes, or
reading a short realtime window after a deployment or campaign.

The portable skill owns the measurement procedure and evidence contract. It does
not own OAuth, credentials, tenants, connector selection, or provider routing.
Operators may bring a local, self-hosted, third-party, or Runx-hosted connector.
Any of those is compatible when it implements the operations and scopes below.
Never place a connection id, credential handle, provider-config key, service URL,
or tenant id in a skill input, fixture, or evidence packet; those bindings belong
in the operator's Runx grant and connector configuration.

## Read the right thing

Use `properties` when the property is unknown. Use `metadata` before composing
an unfamiliar report: GA4 dimension and metric compatibility is part of the
query contract, not a formatting concern. Use `report` for date-bounded
analysis and `realtime` only for a current operational pulse.

`normalize-report` accepts a GA4-shaped result already supplied by a trusted
import or connector test. It performs the same semantic checks, but its output
says `provider_status: not_called` and `source_status: supplied_result`. A
supplied packet is useful evidence with external provenance; it is never proof
that Runx reached Google.

Use an explicit empty `dimensions` list for a metric-only total. Its response
must have empty dimension headers and row dimension values; metric headers and
values still match the exact ordered request.

A report should name the exact property, date ranges, ordered dimensions, ordered
metrics, filters, ordering, currency, row limit, and offset needed for the
decision. Keep it narrow. Wide reports are more likely to combine incompatible
fields, hit quotas, hide detail in an `(other)` row, or give an agent an
attractive but irrelevant aggregate.

The normalized packet maps each ordered value to its header name, retains row
count and pagination, and carries timezone, currency, quota, and response
metadata. Header drift is a hard failure because a row cannot be interpreted
safely when the requested and returned fields differ.

## Measurement caveats

Google Analytics is behavior and outcome evidence, not search-ranking evidence.
The `Organic Search` channel can include search engines other than Google and
depends on the property's channel definitions. Landing-page sessions and key
events help quantify outcomes, but they do not explain why visibility changed.
Join GA4 with Search Console on a deliberate page and date boundary, then inspect
content, releases, attribution settings, and business context before making a
causal claim.

Treat thresholding, sampling metadata, schema restrictions, and
`data_loss_from_other_row` as material. The skill returns
`decision: usable_with_caveats` when any is present. Do not silently compare a
thresholded segment with an unthresholded total, add rows across overlapping date
ranges, or treat a paginated page as the complete property.

Timezone, currency, reporting identity, key-event configuration, consent mode,
and attribution settings can change interpretation without changing the query.
Carry those assumptions into any comparison. Realtime data is provisional and
suited to detection, not settled performance reporting.

## Connector capability contract

A compatible connector exposes provider `google-analytics` with:

- `properties.list` under `properties.read`, projecting `properties`.
- `metadata.get` under `metadata.read`, projecting the property, dimensions,
  metrics, and fetch time.
- `reports.run` under `reports.read`, projecting property, request,
  dimension and metric headers, rows, row count, pagination, response metadata,
  property quota, and fetch time.
- `reports.run_realtime` under `reports.realtime.read`, using the same
  projected report shape and an explicit realtime request.

The connector decides how credentials are acquired and refreshed. It must enforce
the caller's grant, bind the requested property, preserve Google errors, project
only declared fields, and keep secret material out of results and receipts.
This package is read-only; analytics administration, audience changes, event
configuration, and deletion requests require separate governed capabilities.

## Operator handoff

A sealed report packet is ready for review or for a higher-level
`organic-growth` comparison. Preserve the evidence digest and the exact query
when deriving trends. A useful organic acquisition slice commonly includes a
landing-page dimension, a channel or source dimension, and only the outcome
metrics needed for the decision, such as sessions, engaged sessions, key events,
or revenue.

Stop when the property is ambiguous, requested fields are incompatible, response
headers drift, pagination is incomplete for a whole-property claim, privacy
metadata undermines the comparison, timezone or currency is unknown where it
matters, the supplied result lacks provenance, or the configured connector lacks
the exact capability.

The deterministic module exists for GA4 domain semantics that generic Runx tools
cannot infer: binding ordered headers to values, validating report identity,
parsing numeric metrics, and carrying privacy and completeness caveats. Runx
owns provider execution, grants, digesting, projection, and receipts.
