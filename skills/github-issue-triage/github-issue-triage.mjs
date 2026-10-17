export function indexIssues(inputs) {
  const snapshots = inputs.issue_snapshots;
  const seen = new Set();
  const issues = snapshots.map((value) => {
    const issue = normalizeIssue(value);
    if (seen.has(issue.id)) throw new Error(`duplicate issue id: ${issue.id}`);
    seen.add(issue.id);
    return issue;
  });
  const query = inputs.query.trim();
  const snapshotDigest = requiredDigest(inputs.snapshot_digest, "snapshot_digest");
  return {
    issue_index: {
      query,
      issues,
      source_count: issues.length,
      snapshot_digest: snapshotDigest,
    },
  };
}

export function finalizeDiscovery(inputs) {
  const index = object(inputs.issue_index);
  const known = new Map((Array.isArray(index.issues) ? index.issues : []).map((issue) => [text(issue.id), issue]));
  const candidates = Array.isArray(inputs.issue_candidates) ? inputs.issue_candidates : [];
  if (candidates.length === 0 || candidates.length > known.size) {
    throw new Error("issue_candidates must select a bounded non-empty subset");
  }
  const seen = new Set();
  const selected = candidates.map((value) => {
    const candidate = object(value);
    const id = text(candidate.id);
    if (!known.has(id)) throw new Error(`unknown issue candidate: ${id}`);
    if (seen.has(id)) throw new Error(`duplicate issue candidate: ${id}`);
    seen.add(id);
    return {
      id,
      reason: required(candidate.reason, `candidate ${id} reason`),
      source_ref: id,
    };
  });
  return {
    issue_triage_queue: {
      schema: "runx.issue.triage_queue.v1",
      decision: "ready",
      query: index.query,
      issue_candidates: selected,
      selection_rationale: object(inputs.selection_rationale),
      evidence_refs: [requiredDigest(index.snapshot_digest, "issue_index.snapshot_digest")],
      provider_status: "supplied_snapshot",
    },
  };
}

export function admitIssue(inputs) {
  const issue = object(inputs.issue_snapshot);
  const providerOperation = object(inputs.provider_operation);
  const repository = text(inputs.repository) || text(issue.repository);
  const number = text(issue.number || issue.id);
  const title = text(issue.title);
  const state = text(issue.state);
  const body = text(issue.body);
  if (!repository || !number || !title || !state) throw new Error("issue_snapshot is incomplete");
  if (body.length > 20_000) throw new Error("issue_snapshot.body exceeds 20000 characters");
  const evidence = { repository, number, title, state, body };
  const snapshotDigest = requiredDigest(inputs.snapshot_digest, "snapshot_digest");
  return {
    issue_evidence: {
      ...evidence,
      source_ref: text(issue.source_ref)
        || text(providerOperation.readback_ref)
        || text(providerOperation.operation_id)
        || "supplied:issue_snapshot",
      digest: snapshotDigest,
      provider_status: providerOperation.status === "success" ? "readback" : "supplied_snapshot",
    },
  };
}

export function finalizeResponse(inputs) {
  const evidence = object(inputs.issue_evidence);
  const profile = object(inputs.issue_profile);
  const findings = [];
  for (const field of ["repository", "number", "title", "state"]) {
    if (text(profile[field]) !== text(evidence[field])) {
      findings.push({
        code: "issue_triage.identity_mismatch",
        field,
        message: `issue_profile.${field} does not match supplied evidence`,
      });
    }
  }
  const draft = object(inputs.response_draft);
  const channel = text(draft.channel);
  const body = text(draft.body);
  if (channel !== "github_issue_comment") {
    findings.push({
      code: "issue_triage.channel_invalid",
      field: "response_draft.channel",
      message: "response_draft.channel must be github_issue_comment",
    });
  }
  if (!body) {
    findings.push({
      code: "issue_triage.body_missing",
      field: "response_draft.body",
      message: "response_draft.body is required",
    });
  }
  if (body.length > 10_000) {
    findings.push({
      code: "issue_triage.body_too_large",
      field: "response_draft.body",
      message: "response_draft.body exceeds 10000 characters",
    });
  }
  const actions = Array.isArray(inputs.follow_up_actions)
    ? inputs.follow_up_actions.map((value) => text(value)).filter(Boolean)
    : [];
  const ready = findings.length === 0;
  return {
    issue_triage_packet: {
      schema: "runx.issue.triage.v1",
      decision: ready ? "draft_ready" : "blocked",
      issue_profile: profile,
      response_strategy: object(inputs.response_strategy),
      response_draft: { channel, body },
      follow_up_actions: actions,
      evidence_refs: text(evidence.digest) ? [evidence.digest] : [],
      delivery_status: "not_sent",
      provider_status: text(evidence.provider_status) || "supplied_snapshot",
      validation: { status: ready ? "pass" : "fail", findings },
    },
  };
}

function normalizeIssue(value) {
  const issue = object(value);
  const repository = text(issue.repository);
  const number = text(issue.number);
  const title = text(issue.title);
  const state = text(issue.state);
  const body = text(issue.body);
  const id = `${repository}#${number}`;
  return { id, repository, number, title, state, body };
}

function object(value) {
  return value && typeof value === "object" && !Array.isArray(value) ? value : {};
}

function text(value) {
  return typeof value === "string" || typeof value === "number" ? String(value).trim() : "";
}

function required(value, label) {
  const result = text(value);
  if (!result) throw new Error(`${label} is required`);
  return result;
}

function requiredDigest(value, label) {
  const result = text(value);
  if (!/^sha256:[0-9a-f]{64}$/u.test(result)) throw new Error(`${label} must be a native sha256 digest`);
  return result;
}
