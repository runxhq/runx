---
name: nws-weather
description: Read the actual National Weather Service forecast for a US geographic point, or reuse an already selected NWS grid through the existing provider phases.
---

# NWS Weather

Fetch the National Weather Service forecast for an intended US location through
the selected NWS connector. The default `at-point` method accepts numeric
`latitude` and `longitude`, reads the provider's point-to-grid mapping and then
fetches that grid's forecast. No API key, office-code lookup or hand-built
grid request is needed for this route.

Use this for read-only forecast evidence. Global place-name discovery and
Open-Meteo data belong to `open-meteo-weather`; resolve the intended place first
when coordinates are unknown. Do not guess a city or assume that a US place
shares a grid with a nearby point. Notification, scheduling and operational
actions need their own skill and authority.

## How it works

1. Supply the intended latitude and longitude. The NWS request uses four
   decimal places, matching the existing points contract.
2. `points.read` returns the provider's `gridId`, `gridX` and `gridY`. A
   bounded domain function admits those exact fields and keeps the point,
   timezone, forecast URL and native readback binding in `location`. It never
   derives a grid from geography or follows an arbitrary returned URL.
3. Only a usable mapping proceeds through the existing `forecast.read`
   operation. Its original provider packet retains forecast periods, units,
   generated/update times and native evidence.
4. Return the observed data. The calling agent interprets the forecast and
   its coverage; this reader does not invent planning advice or publication.

The [NWS API documentation](https://www.weather.gov/documentation/services-web-api)
explains point-to-grid discovery. Mappings can change, so reuse a saved grid
only while its freshness is appropriate to the caller's job. The default reads
the current mapping; it does not maintain a cache or background loop.

## Reuse in workflows

`forecast` still accepts `office`, `grid_x` and `grid_y` as strings and reads
that exact grid once. It never calls points or geocodes again. `locate` still
accepts string `lat` and `lon` for a points-only read. Preserve the native
provider packet and readback when handing either result downstream.

For the Washington Monument, the direct call is
`{latitude: 38.8894, longitude: -77.0352}`. A parent workflow that already has the
NWS mapping can instead call `forecast` with its current office/grid values.

## Results and failure

The actual result is the existing `runx.provider.operation.v1` evidence packet.
The default also returns `location`, with the admitted mapping and its readback.
A missing/malformed mapping returns `needs_more_evidence` in that location
result and makes no forecast call. It does not mean there is no weather.

Provider refusal, non-success status, timeout and uncertain outcome retain the
native failure or pending-call envelope and receipt state. This skill does not
replace them with a fictional normalized forecast or automatically retry a
possibly charged request. Unsupported NWS coverage is distinct from a forecast
containing no rain. Missing periods, stale timestamps or incomplete data must
remain evidence gaps when the calling agent summarizes the result.

This is not an emergency, aviation, maritime or medical decision service. Use
the relevant official operational channel for such decisions. A successful read
is evidence of the returned forecast, not authority to change an operation.
