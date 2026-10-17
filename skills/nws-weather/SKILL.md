---
name: nws-weather
description: Fetch National Weather Service forecast evidence through the governed HTTP front, producing a sealed provider packet for downstream weather planning.
---

# NWS Weather

Fetch public National Weather Service forecast evidence through runx HTTP.

It uses the governed HTTP front, not an ad hoc client: the receipt records the NWS
endpoint, response status, graph step receipts, and the exact authority surface.
No API key is required. The default runner fetches the actual NWS gridpoint
forecast; the `locate` runner discovers the gridpoint for a latitude/longitude.

## What this skill does

`nws-weather` makes read-only calls to `api.weather.gov`. The `locate`
runner calls `/points/{lat},{lon}` to discover `gridId`, `gridX`, `gridY`, and
forecast URLs. The default `forecast` runner calls
`/gridpoints/{office}/{grid_x},{grid_y}/forecast` to fetch forecast periods and
shapes them into the forecast it returns.

## When to use this skill

- You need real public weather evidence with no credentials.
- A graph needs to prove a weather data read went through runx's governed HTTP
  path.
- You have an NWS office/gridpoint and need the current public forecast.
- You have latitude and longitude in the United States and need the NWS gridpoint
  before running the forecast path.

## When not to use this skill

- For locations outside NWS coverage. Return `needs_input` with the coverage
  limitation.
- For emergency, medical, aviation, maritime, evacuation, or life-safety
  decisions.
- To notify users, reschedule events, deploy changes, or mutate operations based
  on weather. Those actions need their own authority gate.
- To call private networks, untrusted hosts, or non-NWS endpoints.

## Procedure

1. If the caller has only coordinates, run `locate` with `lat` and `lon`.
2. Extract `gridId`, `gridX`, and `gridY` from the sealed locate output.
3. Run the default `forecast` runner with `office`, `grid_x`, and `grid_y`.
4. Confirm the HTTP status is 2xx and the response contains forecast periods.
5. Preserve the NWS source URL, generated timestamp, gridpoint, and receipt refs.
6. Return the shaped forecast; planning prose belongs to the agent that uses
   it. Do not invent guidance inside this provider fetch.
7. Return `needs_input` for malformed coordinates or gridpoints; return
   `needs_more_evidence` for NWS outages, missing periods, or stale data.

## Edge cases and stop conditions

- **Invalid coordinates:** return `needs_input`; NWS point lookup requires
  decimal latitude and longitude.
- **Unsupported location:** return `needs_input`; NWS coverage is not global.
- **Provider outage or non-2xx response:** return `needs_more_evidence` and
  preserve the HTTP status in the receipt.
- **Missing forecast periods:** return `needs_more_evidence`; do not summarize a
  forecast that is not present.
- **Life-safety use:** return `refused` and direct the user to official weather
  or emergency channels.
- **Action requested from weather:** stop at provider evidence and require the
  downstream action skill with its own gate and receipt.

## Output schema

```yaml
decision: ready | needs_input | needs_more_evidence | refused
runtime_path: http
provider: national-weather-service
provider_evidence:
  endpoint: string
  http_status: string
  gridpoint:
    office: string
    grid_x: string
    grid_y: string
  generated_at: string
  forecast_periods: array
receipt_refs: array
stop_conditions: array
```

## Worked example

1. Run `locate` for `38.8894,-77.0352`.
2. The sealed NWS points response returns `gridId: LWX`, `gridX: 97`,
   `gridY: 71`, and a forecast URL.
3. Run the default `forecast` runner with `office: LWX`, `grid_x: "97"`,
   `grid_y: "71"`.
4. Hand the sealed forecast JSON to the downstream agent that plans with it.

## Inputs

- `office` (default runner, required): NWS office id such as `LWX`.
- `grid_x` (default runner, required): NWS grid X coordinate.
- `grid_y` (default runner, required): NWS grid Y coordinate.
- `lat` (`locate` runner, required): decimal latitude for point lookup.
- `lon` (`locate` runner, required): decimal longitude for point lookup.
