export function preparePoint({latitude, longitude}) {
  return {point: {lat: latitude.toFixed(4), lon: longitude.toFixed(4)}};
}
export function bindGrid({points, point}) {
  const properties = points.result?.points?.properties || {};
  const ready = typeof properties.gridId === "string" && /^[A-Z]{3}$/u.test(properties.gridId)
    && Number.isSafeInteger(properties.gridX) && properties.gridX >= 0
    && Number.isSafeInteger(properties.gridY) && properties.gridY >= 0;
  return {location: {
    decision: ready ? "ready" : "needs_more_evidence", point,
    grid: ready ? {office: properties.gridId, grid_x: String(properties.gridX), grid_y: String(properties.gridY)} : null,
    timezone: typeof properties.timeZone === "string" ? properties.timeZone : null,
    forecast_url: typeof properties.forecast === "string" ? properties.forecast : null,
    readback_uri: points.readback_uri, result_digest: points.result_digest,
    reason: ready ? "The NWS points response supplies this grid mapping." : "The NWS points response has no usable office/grid mapping.",
  }};
}
