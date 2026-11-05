export function bindLocation({geocoding, selection, kind}) {
  const candidates = Array.isArray(geocoding.result?.locate?.results) ? geocoding.result.locate.results : [];
  const matches = candidates.filter(candidate => candidate.id === selection.selected_id);
  const selected = selection.decision === "selected" && matches.length === 1 ? matches[0] : null;
  const valid = selected && typeof selected.name === "string" && Number.isInteger(selected.id)
    && Number.isFinite(selected.latitude) && selected.latitude >= -90 && selected.latitude <= 90
    && Number.isFinite(selected.longitude) && selected.longitude >= -180 && selected.longitude <= 180;
  const decision = !candidates.length ? "not_found" : valid ? "selected" : "needs_input";
  const reason = !candidates.length ? "The provider returned no place candidates."
    : valid ? selection.reason : "No valid returned candidate was selected. " + selection.reason;
  return {
    location: {decision, candidates, selected: valid ? selected : null, reason,
      readback_uri: geocoding.readback_uri, result_digest: geocoding.result_digest,
      requested_operation: valid ? kind + ".read" : null,
      request_coordinates: valid ? {latitude: selected.latitude.toFixed(6), longitude: selected.longitude.toFixed(6)} : null},
  };
}
