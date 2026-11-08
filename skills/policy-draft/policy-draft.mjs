export function preparePolicy(inputs) {
  const draft = inputs.policy_proposal;
  const decision = draft.decision;
  const existingPolicy = inputs.existing_policy ?? null;
  const policy = draft.policy ?? {};
  const attenuationFindings = decision === "ready" && existingPolicy
    ? wideningFindings(existingPolicy, policy)
    : [];
  return {
    policy_context: {
      path: decision === "ready" && attenuationFindings.length === 0 ? "lint" : "stop",
      decision: attenuationFindings.length > 0 ? "reject" : decision,
      policy: Object.keys(policy).length > 0 ? policy : null,
      rationale: stringValue(draft.rationale) || "",
      blockers: stringArray(draft.blockers),
      needs_input: stringArray(draft.needs_input),
      success_checkpoint: record(draft.success_checkpoint),
      attenuation_findings: attenuationFindings,
      fixed_existing: existingPolicy ? fixedSettings(existingPolicy, policy) : {},
      fixed_proposed: existingPolicy ? fixedSettings(policy, policy) : {},
    },
  };
}

export function finalizePolicy(inputs) {
  const context = inputs.policy_context;
  const lint = record(inputs.policy_lint);
  if (context.path === "lint") {
    const fixed = inputs.fixed_comparison.equal;
    const passed = lint.status === "pass" && fixed;
    return {
      policy_proposal: {
        ...baseProposal(context, passed ? "ready" : "reject"),
        policy: context.policy,
        validation: {
          status: passed ? "pass" : "fail",
          engine: "runx policy",
          findings: [...records(lint.findings).map(projectFinding), ...(!fixed ? [{code:"policy.attenuation.fixed_settings_changed",path:"$",message:"Policy identity, provider/kind, owner routing, branch, adapter or dedupe settings changed; this generic tightening lane cannot establish that change is safe."}] : [])],
          readback: Object.keys(record(lint.readback)).length > 0 ? lint.readback : null,
          reason: passed ? "Native policy lint and fixed-setting comparison passed." : "Native policy lint or fixed-setting comparison rejected the proposal.",
        },
      },
    };
  }
  const widened = records(context.attenuation_findings);
  return {
    policy_proposal: {
      ...baseProposal(context, context.decision),
      policy: context.policy,
      validation: {
        status: widened.length > 0 ? "fail" : "not_run",
        engine: "runx policy",
        findings: widened,
        readback: null,
        reason: widened.length > 0
          ? "The proposed change widens existing authority and cannot use the tightening lane."
          : "The draft stopped before native lint because required governance inputs were unresolved.",
      },
    },
  };
}

function wideningFindings(existing, proposed) {
  const findings = [];
  requireSubset(findings, "owner_routes", ids(existing.owner_routes, "route_id"), ids(proposed.owner_routes, "route_id"));
  compareControls(findings, existing, proposed);
  requireSubset(findings, "targets", ids(existing.targets, "repo"), ids(proposed.targets, "repo"));
  requireSubset(findings, "sources", ids(existing.sources, "source_id"), ids(proposed.sources, "source_id"));
  requireSubset(findings, "runners", ids(existing.runners, "method_id"), ids(proposed.runners, "method_id"));
  compareRules(findings, existing.sources, proposed.sources, "source_id", ["allowed_locators", "allowed_actions"]);
  compareRules(findings, existing.runners, proposed.runners, "method_id", ["allowed_actions", "target_repos"]);
  compareRules(findings, existing.targets, proposed.targets, "repo", ["allowed_actions", "method_ids"]);
  compareConfidence(findings, existing.sources, proposed.sources);
  comparePermissions(findings, record(existing.permissions), record(proposed.permissions));
  return findings;
}

function requireSubset(findings, field, existing, proposed) {
  for (const value of proposed) if (!existing.has(value)) addWidening(findings, `${field}.${value}`);
}

function compareRules(findings, existingRules, proposedRules, key, fields) {
  const existing = indexBy(existingRules, key);
  for (const proposed of records(proposedRules)) {
    const id = stringValue(proposed[key]);
    const prior = existing.get(id);
    if (!prior) continue;
    for (const field of fields) requireSubset(findings, `${key}.${id}.${field}`, new Set(stringArray(prior[field])), new Set(stringArray(proposed[field])));
  }
}

function compareConfidence(findings, existingSources, proposedSources) {
  const existing = indexBy(existingSources, "source_id");
  for (const proposed of records(proposedSources)) {
    const prior = existing.get(stringValue(proposed.source_id));
    if (!prior) continue;
    const previous = numberValue(prior.minimum_confidence);
    const next = numberValue(proposed.minimum_confidence);
    if (previous !== null && (next === null || next < previous)) addWidening(findings, `source.${proposed.source_id}.minimum_confidence`);
  }
}

function comparePermissions(findings, existing, proposed) {
  if (existing.auto_merge !== true && proposed.auto_merge === true) addWidening(findings, "permissions.auto_merge");
  if (existing.mutate_target_repo !== true && proposed.mutate_target_repo === true) addWidening(findings, "permissions.mutate_target_repo");
  if (existing.require_human_merge_gate === true && proposed.require_human_merge_gate !== true) addWidening(findings, "permissions.require_human_merge_gate");
}

function addWidening(findings, path) {
  findings.push({ code: "policy.attenuation.widened", path, message: "The tightening lane cannot add or widen this authority." });
}

function baseProposal(context, decision) {
  return {
    decision,
    rationale: context.rationale,
    blockers: stringArray(context.blockers),
    needs_input: stringArray(context.needs_input),
    success_checkpoint: record(context.success_checkpoint),
  };
}

function projectFinding(value) {
  const finding = record(value);
  return {
    code: stringValue(finding.code) || "policy.native_lint.finding",
    path: stringValue(finding.path) || "$",
    message: stringValue(finding.message) || "Native policy validation finding.",
  };
}

function indexBy(values, key) {
  return new Map(records(values).map((value) => [stringValue(value[key]), value]).filter(([id]) => id));
}

function ids(values, key) {
  return new Set(records(values).map((value) => stringValue(value[key])).filter(Boolean));
}

function records(value) {
  return Array.isArray(value) ? value.map(record) : [];
}

function stringArray(value) {
  return Array.isArray(value) ? [...new Set(value.map(stringValue).filter(Boolean))] : [];
}

function stringValue(value) {
  return typeof value === "string" && value.trim() ? value.trim() : null;
}

function numberValue(value) {
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

function record(value) {
  return value && typeof value === "object" && !Array.isArray(value) ? value : {};
}



// Only these known fields have monotone policy semantics. Everything else,
// including opaque provider settings, is compared by native data.compare.
function compareControls(findings, existing, proposed) {
  const check = (bad, path) => { if (bad) addWidening(findings, path); };
  const sourceBefore = indexBy(existing.sources, "source_id");
  for (const source of records(proposed.sources)) {
    const before = sourceBefore.get(source.source_id);
    if (!before) continue;
    const oldThread = record(before.source_thread), nextThread = record(source.source_thread);
    check(oldThread.required === true && nextThread.required !== true, "source."+source.source_id+".source_thread.required");
    check(oldThread.publish_mode !== nextThread.publish_mode && nextThread.publish_mode !== "none", "source."+source.source_id+".source_thread.publish_mode");
  }
  for (const [name,key] of [["runners","method_id"],["targets","repo"]]) {
    const prior = indexBy(existing[name],key);
    for (const item of records(proposed[name])) {
      const before = prior.get(item[key]); if (!before) continue;
      check(before.scafld_required === true && item.scafld_required !== true, name+"."+item[key]+".scafld_required");
      if (name === "runners") check(before.state !== "available" && item.state === "available", "runner."+item[key]+".state");
    }
  }
  const before = record(existing.outcomes), next = record(proposed.outcomes);
  for (const field of ["observe_provider","verification_required"]) check(before[field] === true && next[field] !== true, "outcomes."+field);
  check(before.publish_final_source_thread_update !== true && next.publish_final_source_thread_update === true, "outcomes.publish_final_source_thread_update");
  const closeRank={never:0,when_verified:1,when_terminal:2};
  check(closeRank[next.close_source_issue] > closeRank[before.close_source_issue], "outcomes.close_source_issue");
}

function fixedSettings(policy, selected) {
  const omit = (value, fields) => Object.fromEntries(Object.entries(record(value)).filter(([key]) => !fields.includes(key)));
  const group = (name,key,allowedChanges) => {
    const index = indexBy(policy[name],key);
    return Object.fromEntries(records(selected[name]).map(item => {
      const row = {...record(index.get(item[key]))};
      if (name === "sources") { row.adapter_policy ??= {}; row.source_thread = omit(row.source_thread,["required","publish_mode"]); }
      if (name === "owner_routes") { row.labels ??= []; row.project ??= null; }
      if (name === "targets") row.base_branch ??= null;
      for (const field of ["owners","target_repos","labels"]) if (Array.isArray(row[field])) row[field]=Object.fromEntries(row[field].map(value=>[value,true]));
      return [item[key],omit(row,allowedChanges)];
    }));
  };
  return {
    ...omit(policy,["created_at","sources","runners","targets","owner_routes","outcomes","permissions"]),
    sources:group("sources","source_id",["allowed_locators","allowed_actions","minimum_confidence"]),
    runners:group("runners","method_id",["allowed_actions","target_repos","scafld_required","state"]),
    targets:group("targets","repo",["method_ids","allowed_actions","scafld_required"]),
    owner_routes:group("owner_routes","route_id",[]),
    outcomes:omit(policy.outcomes,["observe_provider","verification_required","close_source_issue","publish_final_source_thread_update"]),
    permissions:omit(policy.permissions,["auto_merge","mutate_target_repo","require_human_merge_gate"]),
  };
}
