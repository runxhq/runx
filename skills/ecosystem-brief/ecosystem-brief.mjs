export default function filterFreshSources(inputs) {
  const packets = Array.isArray(inputs.source_packets) ? inputs.source_packets : [];
  const asOfText = text(inputs.as_of);
  const asOf = Date.parse(asOfText);
  const maxAgeHours = Number(inputs.max_age_hours ?? 168);
  const rejected = [];
  const accepted = [];

  if (!Number.isFinite(asOf)) {
    rejected.push({ index: -1, reason: "as_of must be an ISO-8601 timestamp" });
  }
  if (!Number.isFinite(maxAgeHours) || maxAgeHours <= 0 || maxAgeHours > 8760) {
    rejected.push({ index: -1, reason: "max_age_hours must be greater than 0 and at most 8760" });
  }

  if (Number.isFinite(asOf) && Number.isFinite(maxAgeHours) && maxAgeHours > 0 && maxAgeHours <= 8760) {
    packets.forEach((packet, index) => {
      const source = unwrap(packet);
      const fetchedText = text(source?.provenance?.fetched_at);
      const fetchedAt = Date.parse(fetchedText);
      if (!Number.isFinite(fetchedAt)) {
        rejected.push({ index, reason: "provenance.fetched_at is invalid" });
        return;
      }
      const ageHours = (asOf - fetchedAt) / 3_600_000;
      if (ageHours < 0) {
        rejected.push({ index, reason: "source is future-dated" });
      } else if (ageHours > maxAgeHours) {
        rejected.push({ index, reason: "source is stale" });
      } else {
        accepted.push(packet);
      }
    });
  }

  return {
    freshness_report: {
      decision: accepted.length > 0 ? "ready" : "needs_more_evidence",
      as_of: asOfText,
      max_age_hours: maxAgeHours,
      source_packets: accepted,
      accepted_count: accepted.length,
      rejected,
    },
  };
}

function unwrap(value) {
  const outer = record(value) ?? {};
  const packet = record(outer.provider_operation) ?? record(outer.file_read) ?? outer;
  const data = record(packet.data) ?? packet;
  return record(data.result) ?? data;
}

function record(value) {
  return value && typeof value === "object" && !Array.isArray(value) ? value : undefined;
}

function text(value) {
  return typeof value === "string" ? value.trim() : "";
}
