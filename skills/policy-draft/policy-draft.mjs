export function preparePolicy(inputs) {
  const draft = record(inputs.policy_proposal);
  const decision = enumValue(draft.decision, ["ready", "needs_input", "reject"], "decision");
  const existingPolicy = optionalRecord(inputs.existing_policy, "existing_policy");
  const policy = decision === "ready" ? requiredRecord(draft.policy, "policy") : record(draft.policy);
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
    },
  };
}

export function finalizePolicy(inputs) {
  const context = requiredRecord(inputs.policy_context, "policy_context");
  const lint = record(inputs.policy_lint);
  if (context.path === "lint") {
    const passed = lint.status === "pass";
    return {
      policy_proposal: {
        ...baseProposal(context, passed ? "ready" : "reject"),
        policy: context.policy,
        validation: {
          status: passed ? "pass" : "fail",
          engine: "runx policy",
          findings: records(lint.findings).map(projectFinding),
          readback: Object.keys(record(lint.readback)).length > 0 ? lint.readback : null,
          reason: passed ? "Native policy lint passed." : "Native policy lint rejected the proposal.",
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
  requireSubset(findings, "targets", ids(existing.targets, "repo"), ids(proposed.targets, "repo"));
  requireSubset(findings, "sources", ids(existing.sources, "source_id"), ids(proposed.sources, "source_id"));
  requireSubset(findings, "runners", ids(existing.runners, "runner_id"), ids(proposed.runners, "runner_id"));
  compareRules(findings, existing.sources, proposed.sources, "source_id", ["allowed_locators", "allowed_actions"]);
  compareRules(findings, existing.runners, proposed.runners, "runner_id", ["allowed_actions", "target_repos"]);
  compareRules(findings, existing.targets, proposed.targets, "repo", ["allowed_actions", "runner_ids"]);
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

function requiredRecord(value, field) {
  const parsed = record(value);
  if (Object.keys(parsed).length === 0) throw new Error(`${field} must be a non-empty object`);
  return parsed;
}

function optionalRecord(value, field) {
  if (value === undefined || value === null) return null;
  return requiredRecord(value, field);
}

function enumValue(value, allowed, field) {
  if (!allowed.includes(value)) throw new Error(`${field} must be one of ${allowed.join(", ")}`);
  return value;
}
