const object = (value) => value && typeof value === "object" && !Array.isArray(value) ? value : {};
const text = (value) => typeof value === "string" ? value.trim() : "";
const array = (value) => Array.isArray(value) ? value : [];
const parseTime = (value) => Date.parse(text(value).replace(/\.(\d{3})\d+(Z|[+-]\d{2}:\d{2})$/, ".$1$2"));

export function finalizeAttention(inputs) {
  const draft = object(inputs.draft);
  const evidence = array(inputs.evidence);
  const asOf = parseTime(inputs.as_of);
  const refs = new Map(evidence.map((item) => [text(item.source_ref), item]));
  const dispositions = new Map(array(inputs.current_actions).map((item) => [text(item.source_ref), item]));
  const allowed = new Set(array(inputs.allowed_action_ids).map(text));
  const candidates = new Map(array(inputs.work_candidates).map((item) => [JSON.stringify([text(item.source_ref), text(item.route_id), text(item.target_ref)]), item]));
  const findings = [];
  if (candidates.size !== array(inputs.work_candidates).length) findings.push("work candidates contain duplicates");
  if (!Number.isFinite(asOf) || refs.size !== evidence.length) findings.push("evaluation time or source references are invalid");
  const min = Number(inputs.min_check_minutes);
  const max = Number(inputs.max_check_minutes);
  if (!Number.isInteger(min) || !Number.isInteger(max) || min > max) findings.push("next-check bounds are invalid");
  const selected = [];
  const seen = new Set();
  for (const raw of array(draft.items)) {
    const ref = text(raw?.source_ref);
    const priority = text(raw?.priority);
    const source = refs.get(ref);
    const state = object(dispositions.get(ref));
    if (!source || seen.has(ref) || !["high", "medium", "low"].includes(priority)) {
      findings.push("draft item cites an unknown, repeated or invalid source");
      continue;
    }
    if (state.disposition && state.disposition !== "open") {
      findings.push("draft item selects an action already dispositioned by operator-inbox");
      continue;
    }
    seen.add(ref);
    selected.push({
      source_ref: ref,
      source_digest: text(source.source_digest),
      source_kind: text(source.source_kind),
      priority,
      summary: text(source.summary).replace(/\s+/g, " "),
    });
  }
  if (selected.length > 5 || array(draft.items).length !== selected.length) findings.push("draft items exceed the limit or contain an invalid selection");
  const work = [];
  const seenWork = new Set();
  const proposedWork = array(draft.work_proposals);
  const candidateRoutes = new Set(array(inputs.work_candidates).map((item) => text(item.route_id)));
  const admittedWork = proposedWork.filter((item) => candidateRoutes.has(text(item?.route_id)));
  for (const raw of admittedWork) {
    const sourceRef = text(raw?.source_ref);
    const routeId = text(raw?.route_id);
    const targetRef = text(raw?.target_ref);
    const key = JSON.stringify([sourceRef, routeId, targetRef]);
    const candidate = candidates.get(key);
    const source = refs.get(sourceRef);
    const state = object(dispositions.get(sourceRef));
    if (!candidate || !source || text(candidate.source_digest) !== text(source.source_digest)
        || !seen.has(sourceRef) || (state.disposition && state.disposition !== "open") || seenWork.has(key)) {
      findings.push("work proposal is not an exact candidate for an open selected source");
      continue;
    }
    seenWork.add(key);
    work.push({ source_ref: sourceRef, source_digest: text(source.source_digest), route_id: routeId, target_ref: targetRef });
  }
  if (work.length > 3 || work.length !== admittedWork.length) findings.push("work proposals exceed the limit or contain an invalid selection");
  const proposedActions = array(draft.recommended_action_ids).map(text);
  const actions = proposedActions.filter((id) => allowed.has(id));
  if (proposedActions.length > 10 || new Set(actions).size !== actions.length) {
    findings.push("draft action proposal exceeds the limit or repeats an admitted action");
  }
  const requested = Number(draft.next_check_minutes);
  if (!Number.isInteger(requested)) findings.push("next-check proposal is invalid");
  const minutes = Number.isFinite(min) && Number.isFinite(max) && min <= max
    ? Math.min(max, Math.max(min, Number.isInteger(requested) ? requested : max)) : null;
  if (draft.decision !== "brief" && draft.decision !== "idle") findings.push("draft decision is invalid");
  if (selected.length === 0 && (actions.length > 0 || work.length > 0)) findings.push("draft proposes action without an admitted item");
  const valid = findings.length === 0;
  const decision = !valid ? "held" : selected.length > 0 ? "ready" : "idle";
  const brief = decision === "ready"
    ? selected.map((item) => `${item.priority.toUpperCase()}: ${item.summary} (${item.source_ref})`).join("\n")
    : "";
  return {
    attention_packet: {
      decision,
      items: valid ? selected : [],
      recommended_action_ids: valid ? actions : [],
      work_proposals: valid ? work : [],
      brief,
      next_check_minutes: minutes,
      delivery_status: "not_sent",
      validation: {
        status: valid ? "pass" : "fail",
        findings,
        omitted_unconfigured_actions: proposedActions.length - actions.length,
        omitted_unconfigured_work: proposedWork.length - admittedWork.length,
      },
    },
  };
}
