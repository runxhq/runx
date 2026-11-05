export function finalizeAttention(inputs) {
  const {draft, evidence, current_actions: currentActions} = inputs;
  const workRoutes = inputs.work_routes ?? [];
  const sources = new Map(evidence.map(item => [item.source_id, item]));
  const states = new Map(currentActions.map(item => [item.source_id, item]));
  const routes = new Map(workRoutes.map(route => [route.route_id, route]));
  const allowed = new Set(inputs.allowed_action_ids);
  const findings = [];
  const asOf = Date.parse(inputs.as_of.replace(/\.(\d{3})\d+(Z|[+-]\d{2}:\d{2})$/, ".$1$2"));
  if (!Number.isFinite(asOf)) findings.push("evaluation time is invalid");
  if (sources.size !== evidence.length) findings.push("source references contain duplicates");
  if (states.size !== currentActions.length) findings.push("current action states contain duplicate source references");
  if (routes.size !== workRoutes.length) findings.push("work routes contain duplicates");
  const min = inputs.min_check_minutes;
  const max = inputs.max_check_minutes;
  if (min > max) findings.push("next-check bounds are invalid");

  const selected = [];
  const seen = new Set();
  for (const item of draft.items) {
    const source = sources.get(item.source_id);
    if (!source || seen.has(item.source_id)) {
      findings.push("draft item cites an unknown or repeated source");
      continue;
    }
    if (states.has(item.source_id) && states.get(item.source_id).state !== "open") {
      findings.push("draft item selects an action already handled");
      continue;
    }
    seen.add(item.source_id);
    selected.push({source_id: item.source_id, source_digest: source.source_digest,
      source_kind: source.source_kind, priority: item.priority,
      summary: source.summary.trim().replace(/\s+/g, " ")});
  }

  const proposedWork = draft.work_proposals ?? [];
  const admittedWork = proposedWork.filter(item => routes.has(item.route_id));
  const work = [];
  const seenWork = new Set();
  for (const item of admittedWork) {
    const route = routes.get(item.route_id);
    const source = sources.get(item.source_id);
    const key = JSON.stringify([item.source_id, item.route_id, item.target_locator]);
    const candidateMatches = source &&
      (route.source_id === undefined || route.source_id === source.source_id) &&
      (route.source_digest === undefined || route.source_digest === source.source_digest) &&
      (route.target_locator === undefined || route.target_locator === item.target_locator) &&
      (!route.source_kinds?.length || route.source_kinds.includes(source.source_kind));
    if (!candidateMatches || item.target_locator !== (source.thread_locator ?? source.source_id) ||
        !seen.has(item.source_id) || seenWork.has(key)) {
      findings.push("work proposal is not an exact candidate for an open selected source");
      continue;
    }
    seenWork.add(key);
    const fixed = route.fixed_inputs ?? {};
    const workInputs = Object.fromEntries(Object.entries(item.inputs)
      .filter(([name]) => !Object.hasOwn(fixed, name) && name !== route.source_context_input));
    work.push({source_id: item.source_id, source_digest: source.source_digest, route_id: item.route_id,
      target_locator: item.target_locator, inputs: workInputs, due_at_unix_seconds: item.due_at_unix_seconds});
  }

  const actions = draft.recommended_action_ids.filter(id => allowed.has(id));
  if (new Set(actions).size !== actions.length) findings.push("draft repeats an admitted action");
  if (!selected.length && (actions.length || work.length)) findings.push("draft proposes action without an admitted item");
  const valid = findings.length === 0;
  const decision = !valid ? "held" : selected.length ? "ready" : "idle";
  return {attention_packet: {
    decision, items: valid ? selected : [], recommended_action_ids: valid ? actions : [],
    work_proposals: valid ? work : [],
    brief: decision === "ready" ? selected.map(item => item.priority.toUpperCase() + ": " + item.summary + " (" + item.source_id + ")").join("\n") : "",
    next_check_minutes: min <= max ? Math.min(max, Math.max(min, draft.next_check_minutes)) : null,
    delivery_status: "not_sent",
    validation: {status: valid ? "pass" : "fail", findings,
      omitted_unconfigured_actions: draft.recommended_action_ids.length - actions.length,
      omitted_unconfigured_work: proposedWork.length - admittedWork.length},
  }};
}
