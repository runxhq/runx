export function admitFeed(inputs) {
  const objective = text(inputs.objective);
  const supplied = Array.isArray(inputs.feed_snapshot)
    ? inputs.feed_snapshot
    : feedFromPosts(inputs.posts);
  const asOfText = text(inputs.as_of);
  const asOf = Date.parse(asOfText);
  const maxAgeHours = Number(inputs.max_age_hours ?? 168);
  const blockers = [];
  const sources = [];
  const seen = new Set();

  if (!objective) blockers.push("objective is missing");
  if (!Number.isFinite(asOf)) blockers.push("as_of is invalid");
  if (!Number.isFinite(maxAgeHours) || maxAgeHours <= 0 || maxAgeHours > 8760) blockers.push("max_age_hours is invalid");
  if (supplied.length === 0) blockers.push("feed_snapshot is empty");
  if (supplied.length > 100) blockers.push("feed_snapshot exceeds 100 items");

  if (Number.isFinite(asOf) && Number.isFinite(maxAgeHours)) {
    supplied.slice(0, 100).forEach((raw, position) => {
      const item = object(raw);
      const sourceId = text(item.source_id);
      const topic = text(item.topic);
      const summary = text(item.summary);
      const observedText = text(item.observed_at);
      const observedAt = Date.parse(observedText);
      if (!sourceId || seen.has(sourceId) || !topic || !summary || !Number.isFinite(observedAt)) {
        blockers.push(`feed_snapshot[${position}] is incomplete or duplicated`);
        return;
      }
      const age = (asOf - observedAt) / 3_600_000;
      if (age < 0 || age > maxAgeHours) {
        blockers.push(`feed_snapshot[${position}] is stale or future-dated`);
        return;
      }
      seen.add(sourceId);
      sources.push({
        source_id: sourceId,
        topic,
        summary,
        observed_at: observedText,
      });
    });
  }

  return {
    feed_index: {
      decision: blockers.length === 0 ? "ready" : "needs_more_evidence",
      objective,
      sources,
      blockers,
    },
  };
}

export function finalizeScan(inputs) {
  const index = object(inputs.feed_index);
  const draft = object(inputs.scan_draft);
  const sources = Array.isArray(index.sources) ? index.sources : [];
  const known = new Set(sources.map((source) => text(source.source_id)));
  const evidenceDigest = text(inputs.evidence_digest);
  const findings = strings(index.blockers).map((message) => ({ code: "moltbook.feed.invalid", message }));
  const requested = text(draft.decision);

  if (index.decision === "ready" && !["ready", "not_worth_posting"].includes(requested)) {
    findings.push({ code: "moltbook.scan.decision.invalid", message: "scan decision is invalid" });
  }
  if (index.decision === "ready" && !/^sha256:[0-9a-f]{64}$/u.test(evidenceDigest)) {
    findings.push({ code: "moltbook.feed.unbound", message: "native feed evidence digest is missing" });
  }
  if (index.decision === "ready" && requested === "ready") {
    for (const value of [draft.opportunity_report, draft.post_outline]) {
      const cited = strings(value?.source_ids);
      if (cited.length === 0 || cited.some((sourceId) => !known.has(sourceId))) {
        findings.push({ code: "moltbook.scan.unbound", message: "scan artifact cites unknown feed evidence" });
      }
    }
  }
  const ready = index.decision === "ready" && findings.length === 0;
  return {
    moltbook_scan_packet: {
      decision: ready ? requested : "needs_more_evidence",
      opportunity_report: ready ? object(draft.opportunity_report) : {},
      post_outline: ready ? object(draft.post_outline) : {},
      moderation_notes: ready ? object(draft.moderation_notes) : {},
      follow_up_plan: ready ? object(draft.follow_up_plan) : {},
      source_evidence: sources.map((source) => ({
        source_id: text(source.source_id),
        observed_at: text(source.observed_at),
      })),
      evidence_digest: evidenceDigest,
      delivery_status: "not_posted",
      validation: { status: ready ? "pass" : "fail", findings },
    },
  };
}

export function admitScan(inputs) {
  const packet = unwrap(inputs.scan_packet);
  const outline = object(inputs.outline);
  const evidence = Array.isArray(packet.source_evidence) ? packet.source_evidence : [];
  const sourceIds = evidence.map((item) => text(item.source_id)).filter(Boolean);
  const blockers = [];
  if (packet.decision !== "ready" || packet.validation?.status !== "pass") {
    blockers.push("scan packet is not validated and ready");
  }
  if (packet.delivery_status !== "not_posted") blockers.push("scan delivery status is invalid");
  if (!text(outline.headline) || !Array.isArray(outline.beats)) blockers.push("outline is incomplete");
  if (text(packet.post_outline?.headline) !== text(outline.headline)) {
    blockers.push("outline does not match the scan packet");
  }
  return {
    post_context: {
      decision: blockers.length === 0 ? "ready" : "needs_more_evidence",
      outline,
      source_ids: sourceIds,
      source_evidence: evidence,
      blockers,
    },
  };
}

export function finalizePost(inputs) {
  const context = object(inputs.post_context);
  const draft = object(inputs.post_draft);
  const submolt = text(inputs.submolt);
  const known = new Set(strings(context.source_ids));
  const findings = strings(context.blockers).map((message) => ({
    code: "moltbook.post.context.invalid",
    message,
  }));

  if (context.decision === "ready") {
    if (text(draft.decision) !== "ready_for_handoff") {
      findings.push({ code: "moltbook.post.not_ready", message: "post decision is not ready_for_handoff" });
    }
    const payload = object(draft.post_payload);
    if (text(payload.channel) !== "moltbook" || !text(payload.title) || !text(payload.body)) {
      findings.push({ code: "moltbook.post.payload.invalid", message: "post payload is incomplete" });
    }
    if (!submolt || payload.submolt !== submolt) {
      findings.push({ code: "moltbook.post.submolt.mismatch", message: "post payload does not name the selected submolt" });
    }
    if (characters(payload.title) > 300 || characters(payload.body) > 40000) {
      findings.push({ code: "moltbook.post.payload.too_long", message: "post title or body exceeds Moltbook's limits" });
    }
    const claims = Array.isArray(payload.claims) ? payload.claims : [];
    if (claims.length === 0 || claims.some((entry) => {
      const sourceIds = strings(entry?.source_ids);
      return sourceIds.length === 0 || sourceIds.some((sourceId) => !known.has(sourceId));
    })) {
      findings.push({ code: "moltbook.post.claim.unbound", message: "post claims cite unknown evidence" });
    }
  }
  const payloadDigest = text(inputs.payload_digest);
  if (context.decision === "ready" && !/^sha256:[0-9a-f]{64}$/u.test(payloadDigest)) {
    findings.push({ code: "moltbook.post.unbound", message: "native post payload digest is missing" });
  }
  const ready = context.decision === "ready" && findings.length === 0;
  return {
    moltbook_post_packet: {
      decision: ready ? "ready_for_handoff" : "needs_more_evidence",
      post_payload: ready ? object(draft.post_payload) : {},
      payload_digest: ready ? payloadDigest : "",
      moderation_notes: ready ? object(draft.moderation_notes) : {},
      follow_up_plan: ready ? object(draft.follow_up_plan) : {},
      source_evidence: Array.isArray(context.source_evidence) ? context.source_evidence : [],
      delivery_status: "not_posted",
      validation: { status: ready ? "pass" : "fail", findings },
    },
  };
}

// A Moltbook post as feed evidence: its id is the source reference and its
// creation time the observation time.
function feedFromPosts(posts) {
  return (Array.isArray(posts) ? posts : []).map((raw) => {
    const post = object(raw);
    const id = text(post.id);
    return {
      source_id: id ? `moltbook:post:${id}` : "",
      topic: text(post.title),
      summary: text(post.content) || text(post.url) || text(post.title),
      observed_at: text(post.created_at),
    };
  });
}

// Characters as JSON Schema counts them: code points, not UTF-16 units.
function characters(value) {
  return typeof value === "string" ? [...value].length : 0;
}

function unwrap(value) {
  const item = object(value);
  return item.data && typeof item.data === "object" ? item.data : item;
}

function object(value) {
  return value && typeof value === "object" && !Array.isArray(value) ? value : {};
}

function text(value) {
  return typeof value === "string" ? value.trim() : "";
}

function strings(value) {
  return Array.isArray(value)
    ? value.filter((entry) => typeof entry === "string").map((entry) => entry.trim()).filter(Boolean)
    : [];
}
