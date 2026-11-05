---
name: open-meteo-weather
description: Resolve a place and fetch global forecast and air-quality evidence through governed, keyless Open-Meteo HTTP calls.
---

# Open-Meteo Weather

Fetch global place, forecast, and air-quality evidence through runx's governed
connector boundary. The skill never performs an ad-hoc fetch. Each provider stage is a bounded,
read-only HTTP runner whose endpoint, inputs, status, authority scope, and
output packet are recorded in the sealed receipt. The built-in connector uses
public keyless endpoints. Commercial use needs an appropriate Open-Meteo plan
and connector; keyless access is not a commercial entitlement. This is the worldwide counterpart to the US-only
`nws-weather` provider.

## Procedure

1. Run the default `weather` runner with `name` and any regional `location_context`.
   It reads the geocoder once and asks the operating agent to select an actual
   returned candidate, or preserve unresolved ambiguity.
2. A bounded domain function binds that candidate id to the original provider
   record before any weather request. The record and native geocoding readback
   remain in `location`; request coordinates use six decimal places to match
   the existing connector input contract.
3. Read the requested `kind`: `forecast` (default) or `air_quality`. Already
   resolved coordinates use the explicit `forecast` or `air_quality` runner,
   bypassing geocoding and selection entirely.
4. Require a 2xx response and preserve the provider URL, timezone, generation
   time, current conditions, daily forecast, and receipt references.
5. Treat the result as context only. Any notification, scheduling change, or
   operational mutation requires a separate skill and authority gate.

## Inputs

- `name` (`weather` and `locate`, required): city or place to resolve.
- `location_context` (`weather`, optional): country, administrative area or other intent.
- `kind` (`weather`, optional): `forecast` or `air_quality`, default `forecast`.
- `latitude` (`forecast`, required): decimal latitude.
- `longitude` (`forecast`, required): decimal longitude.
- `forecast_days` (`forecast`, optional): days requested, default `3`.

## Output packets

The default returns `location` plus the requested provider operation evidence.
Ambiguous, unmatched or invalid selection returns its candidates and reason
without a weather request. `locate` retains raw geocoding candidates for graphs.
The explicit `forecast` runner takes a selected latitude and longitude and
returns forecast evidence without geocoding again. `air_quality` likewise takes
coordinates directly. There is no dependency between an unrelated place name
and separately supplied coordinates. Each provider operation and its returned
evidence remain in the sealed receipt.

Preserve provider metadata and receipt references when passing either packet to
another skill. Do not convert provider evidence into high-stakes advice.

## Failure, retry, and stop conditions

- **Ambiguous or missing place:** return `needs_input`; do not silently choose a
  similarly named location.
- **Invalid coordinates or forecast-days value:** return `needs_input`.
- **Non-2xx response, timeout, or rate limit:** return `needs_more_evidence`,
  preserve the failure in the receipt, and retry only under the caller's retry
  policy.
- **Stale or incomplete provider response:** return `needs_more_evidence`.
- **Life-safety, medical, aviation, maritime, or emergency use:** refuse and
  direct the caller to official channels.
- **Mutation requested from forecast evidence:** stop after the sealed read and
  require a separate authority-scoped action skill.

## Worked example

1. Run `locate` with `name: Shanghai`.
2. Select the result for Shanghai, China.
3. Run `forecast` using the sealed latitude and longitude with
   `forecast_days: "3"`.
4. Preserve both sealed receipts with the returned forecast evidence.

## Credits

Combined from two community contributions: the forecast skill (#63, @why123-boop) and the air-quality snapshot (#64, @yanziwei).

## Agent task: open-meteo-select-location

Read the actual geocoding results and the operator's location intent. Return
selection with decision selected, selected_id set to one returned integer id,
and a concise reason only when that candidate matches the intended location.
Never choose the first result merely because it is first, fabricate coordinates
or treat an unqualified common place name as unambiguous. Country and region
context can distinguish candidates; population alone cannot establish intent.
Otherwise return needs_input with selected_id null and name the missing region
or ambiguity. An empty candidate set is not evidence of missing weather.

The domain binding owns candidate identity and coordinate admission; the
connector owns all provider calls and readback. No notification or action is
part of this selection. The operator may call the existing coordinate phases
with a previously selected location instead of geocoding again.

Geocoding fields and commercial access are described in the
[Open-Meteo geocoding documentation](https://open-meteo.com/en/docs/geocoding-api).
