export default function validateWorkPlan(inputs) {
  const catalogNames = new Set(records(inputs.catalog_skills).map((entry) => optionalString(entry.name)).filter(Boolean));
  const findings = [];
  const draft = record(inputs.work_plan_draft);
  let normalized = emptyPlan();

  try {
    normalized = validateDraft(draft, { inputs, findings, catalogNames });
  } catch (error) {
    findings.push({
      code: "draft.invalid_shape",
      path: "work_plan_draft",
      message: boundedMessage(error),
    });
  }

  if (normalized.open_questions.length > 0) {
    findings.push({
      code: "plan.open_questions",
      path: "open_questions",
      message: "Blocking questions must be resolved before a plan is ready.",
    });
  }

  const ready = draft.decision === "ready" && findings.length === 0;
  return {
    decision: ready ? "ready" : "blocked",
    plan_kind: normalized.plan_kind,
    change_set: normalized.change_set,
    harness_context: isRecord(inputs.harness_context) ? inputs.harness_context : {},
    objective_summary: normalized.objective_summary,
    workspace_change_plan: ready ? normalized.workspace_change_plan : {},
    orchestration_steps: ready ? normalized.orchestration_steps : [],
    required_skills: normalized.required_skills,
    open_questions: normalized.open_questions,
    evidence: {
      source_change_set_preserved: normalized.source_change_set_preserved,
      source_change_set_status: normalized.source_context_available
        ? (normalized.source_change_set_preserved ? "preserved" : "drifted")
        : "not_supplied",
      source_thread_locator_preserved: normalized.source_thread_locator_preserved,
      source_context_available: normalized.source_context_available,
      catalog_skills: normalized.catalog_skills,
    },
    validation: {
      status: ready ? "pass" : "hold",
      findings,
    },
  };
}

function validateDraft(value, state) {
  const { inputs, findings, catalogNames } = state;
  const objective = requiredString(inputs.objective, "objective");
  const decision = enumValue(value.decision, ["ready", "blocked"], "decision");
  const planKind = enumValue(value.plan_kind, ["workspace_change", "skill_package"], "plan_kind");
  const changeSet = requiredRecord(value.change_set, "change_set");
  const sourceChangeSet = sourceChangeSetFromInputs(inputs);
  const sourceContextExpected = isRecord(inputs.change_set)
    || isRecord(inputs.thread)
    || Boolean(optionalString(inputs.thread_locator));
  if (sourceContextExpected && !sourceChangeSet) {
    findings.push({
      code: "change_set.source_missing",
      path: "change_set",
      message: "The caller supplied source context, but no source change set was available to preserve.",
    });
  }
  const sourceChangeSetPreserved = sourceChangeSet ? equalJson(changeSet, sourceChangeSet) : false;
  if (sourceChangeSet && !sourceChangeSetPreserved) {
    findings.push({ code: "change_set.drift", path: "change_set", message: "A supplied code-intake change set must be preserved exactly." });
  }
  if (sourceChangeSet && sourceChangeSet.action_decision !== "proceed_to_plan") {
    findings.push({ code: "change_set.not_plannable", path: "change_set.action_decision", message: "The supplied change set does not authorize a planning lane." });
  }
  if (sourceChangeSet && sourceChangeSet.commence_decision !== "approve") {
    findings.push({ code: "change_set.not_commenced", path: "change_set.commence_decision", message: "The supplied change set has not approved commencement." });
  }

  const inputThread = optionalString(inputs.thread_locator)
    || optionalString(record(inputs.thread).thread_locator)
    || optionalString(sourceChangeSet?.thread_locator);
  const changeSetThread = optionalString(changeSet.thread_locator);
  const sourceThreadLocatorPreserved = !sourceChangeSet || !inputThread || inputThread === changeSetThread;
  if (!sourceThreadLocatorPreserved) {
    findings.push({ code: "thread_locator.drift", path: "change_set.thread_locator", message: "The source thread locator changed during planning." });
  }

  const objectiveSummary = requiredString(value.objective_summary, "objective_summary");
  const plan = validateWorkspacePlan(value.workspace_change_plan, changeSet, objectiveSummary, findings, {
    sourceChangeSet,
    projectContext: optionalString(inputs.project_context),
  });
  const requiredSkills = validateRequiredSkills(value.required_skills, catalogNames, findings);
  const steps = validateOrchestration(value.orchestration_steps, requiredSkills, catalogNames, findings);
  const routedSkills = new Set(steps.map((step) => normalizedSkillName(step.skill)));
  for (const required of requiredSkills) {
    if (required.name !== "approval" && !routedSkills.has(required.name)) {
      findings.push({
        code: "catalog.required_skill_unrouted",
        path: "required_skills",
        message: `Required skill '${required.name}' is declared but no orchestration step routes to it.`,
      });
    }
  }
  if (planKind === "skill_package" && !steps.some((step) => normalizedSkillName(step.skill) === "skill-build")) {
    findings.push({ code: "skill_package.skill_build_required", path: "orchestration_steps", message: "Runx skill-package authoring must use the canonical skill-build lane." });
  }

  const openQuestions = strings(value.open_questions, "open_questions");
  if (!equalJson(openQuestions, plan.open_questions)) {
    findings.push({ code: "open_questions.drift", path: "open_questions", message: "Top-level and workspace-plan open questions must match." });
  }
  if (decision === "blocked" && openQuestions.length === 0) {
    findings.push({ code: "blocked.reason_missing", path: "open_questions", message: "A blocked plan must name the question that prevents readiness." });
  }

  return {
    plan_kind: planKind,
    change_set: changeSet,
    objective_summary: objectiveSummary,
    workspace_change_plan: plan,
    orchestration_steps: steps,
    required_skills: requiredSkills,
    open_questions: openQuestions,
    source_change_set_preserved: sourceChangeSet ? sourceChangeSetPreserved : false,
    source_thread_locator_preserved: sourceThreadLocatorPreserved,
    source_context_available: Boolean(sourceChangeSet),
    catalog_skills: [...new Set(steps.map((step) => normalizedSkillName(step.skill)).filter((name) => name && name !== "approval"))].sort(),
    objective,
  };
}

function validateWorkspacePlan(value, changeSet, objectiveSummary, findings, context) {
  const plan = requiredRecord(value, "workspace_change_plan");
  requiredString(plan.plan_id, "workspace_change_plan.plan_id");
  const changeSetId = requiredString(changeSet.change_set_id, "change_set.change_set_id");
  if (requiredString(plan.change_set_id, "workspace_change_plan.change_set_id") !== changeSetId) {
    findings.push({ code: "plan.change_set_mismatch", path: "workspace_change_plan.change_set_id", message: "The workspace plan must bind to the parent change set." });
  }
  if (requiredString(plan.objective_summary, "workspace_change_plan.objective_summary") !== objectiveSummary) {
    findings.push({ code: "plan.objective_mismatch", path: "workspace_change_plan.objective_summary", message: "The workspace plan objective must match the top-level summary." });
  }
  const invariants = strings(plan.shared_invariants, "workspace_change_plan.shared_invariants");
  const criteria = strings(plan.success_criteria, "workspace_change_plan.success_criteria");
  if (!equalJson(invariants, strings(changeSet.shared_invariants, "change_set.shared_invariants"))) {
    findings.push({ code: "plan.invariants_drift", path: "workspace_change_plan.shared_invariants", message: "The plan changed the parent invariants." });
  }
  if (!equalJson(criteria, strings(changeSet.success_criteria, "change_set.success_criteria"))) {
    findings.push({ code: "plan.criteria_drift", path: "workspace_change_plan.success_criteria", message: "The plan changed the parent success criteria." });
  }

  const phases = records(plan.phases, "workspace_change_plan.phases");
  const targetSurfaces = new Set(
    records(changeSet.target_surfaces, "change_set.target_surfaces")
      .map((surface, index) => requiredString(surface.surface, `change_set.target_surfaces[${index}].surface`)),
  );
  const phaseIds = new Set();
  const requestIds = new Set();
  for (const [phaseIndex, phase] of phases.entries()) {
    const phasePath = `workspace_change_plan.phases[${phaseIndex}]`;
    const id = identifier(phase.id, `${phasePath}.id`);
    if (phaseIds.has(id)) findings.push({ code: "phase.duplicate_id", path: `${phasePath}.id`, message: "Phase ids must be unique." });
    for (const [dependencyIndex, dependency] of strings(phase.depends_on, `${phasePath}.depends_on`).entries()) {
      if (!phaseIds.has(dependency)) findings.push({ code: "phase.unknown_dependency", path: `${phasePath}.depends_on[${dependencyIndex}]`, message: "A phase may depend only on an earlier phase." });
    }
    phaseIds.add(id);
    requiredString(phase.name, `${phasePath}.name`);
    requiredBoolean(phase.parallelizable, `${phasePath}.parallelizable`);
    const repos = new Set();
    for (const [requestIndex, request] of records(phase.repo_change_requests, `${phasePath}.repo_change_requests`).entries()) {
      const requestPath = `${phasePath}.repo_change_requests[${requestIndex}]`;
      const requestId = identifier(request.id, `${requestPath}.id`);
      if (requestIds.has(requestId)) findings.push({ code: "repo_request.duplicate_id", path: `${requestPath}.id`, message: "Repo request ids must be unique across the plan." });
      for (const [dependencyIndex, dependency] of strings(request.depends_on, `${requestPath}.depends_on`).entries()) {
        if (!requestIds.has(dependency)) findings.push({ code: "repo_request.unknown_dependency", path: `${requestPath}.depends_on[${dependencyIndex}]`, message: "A repo request may depend only on an earlier request." });
      }
      const repo = requiredString(request.repo, `${requestPath}.repo`);
      if (targetSurfaces.size > 0 && !targetSurfaces.has(repo)) {
        findings.push({ code: "repo_request.target_not_declared", path: `${requestPath}.repo`, message: "A repo change request must target a surface declared by the source change set." });
      }
      if (!context.sourceChangeSet && isSyntheticTarget(repo)) {
        findings.push({ code: "repo_request.synthetic_target", path: `${requestPath}.repo`, message: "A repo change request may not invent a placeholder or synthetic repository." });
      }
      requiredString(request.task_id, `${requestPath}.task_id`);
      requiredString(request.objective, `${requestPath}.objective`);
      strings(request.shared_context_paths, `${requestPath}.shared_context_paths`);
      const validationCommands = strings(request.validation_commands, `${requestPath}.validation_commands`);
      for (const [commandIndex, command] of validationCommands.entries()) {
        if (isPlaceholderCommand(command)) {
          findings.push({ code: "repo_request.placeholder_validation", path: `${requestPath}.validation_commands[${commandIndex}]`, message: "Validation commands must be concrete executable commands, not placeholders." });
        }
      }
      const mutating = requiredBoolean(request.mutating, `${requestPath}.mutating`);
      if (phase.parallelizable === true && mutating && repos.has(repo)) {
        findings.push({ code: "phase.shared_mutation_target", path: requestPath, message: "Parallel requests may not mutate the same repo." });
      }
      if (mutating) repos.add(repo);
      requestIds.add(requestId);
    }
  }
  if (phases.length === 0) findings.push({ code: "plan.phases_empty", path: "workspace_change_plan.phases", message: "A ready plan needs at least one phase." });

  const integrationChecks = strings(plan.integration_checks, "workspace_change_plan.integration_checks");
  if (integrationChecks.length === 0) findings.push({ code: "plan.integration_checks_empty", path: "workspace_change_plan.integration_checks", message: "A ready plan needs at least one integration check." });
  validateCanonicalEndpoints(plan, context.sourceChangeSet, findings);
  return {
    ...plan,
    shared_invariants: invariants,
    success_criteria: criteria,
    phases,
    integration_checks: integrationChecks,
    open_questions: strings(plan.open_questions, "workspace_change_plan.open_questions"),
  };
}

function validateRequiredSkills(value, catalogNames, findings) {
  return records(value, "required_skills").map((entry, index) => {
    const name = normalizedSkillName(requiredString(entry.name, `required_skills[${index}].name`));
    const exists = requiredBoolean(entry.exists, `required_skills[${index}].exists`);
    const actual = catalogNames.has(name);
    if (exists !== actual) findings.push({ code: "catalog.existence_mismatch", path: `required_skills[${index}].exists`, message: "The declared skill existence does not match the local catalog." });
    return { name, exists };
  });
}

function validateOrchestration(value, requiredSkills, catalogNames, findings) {
  const requiredNames = new Set(requiredSkills.map((entry) => entry.name));
  const seen = new Set();
  return records(value, "orchestration_steps").map((step, index) => {
    const stepPath = `orchestration_steps[${index}]`;
    const id = identifier(step.id, `${stepPath}.id`);
    if (seen.has(id)) findings.push({ code: "step.duplicate_id", path: `${stepPath}.id`, message: "Orchestration step ids must be unique." });
    const skill = requiredString(step.skill, `${stepPath}.skill`);
    const skillName = normalizedSkillName(skill);
    if (skillName !== "approval" && !catalogNames.has(skillName)) findings.push({ code: "catalog.unknown_skill", path: `${stepPath}.skill`, message: "The referenced skill is absent from the local catalog." });
    if (skillName !== "approval" && !requiredNames.has(skillName)) findings.push({ code: "catalog.undeclared_skill", path: `${stepPath}.skill`, message: "Every referenced skill must appear in required_skills." });
    const scopes = strings(step.scopes, `${stepPath}.scopes`);
    const mutating = requiredBoolean(step.mutating, `${stepPath}.mutating`);
    if (mutating && scopes.length === 0) findings.push({ code: "step.mutation_scope_missing", path: `${stepPath}.scopes`, message: "A future mutating step must declare its required scopes." });
    for (const [referenceIndex, reference] of strings(step.context_from, `${stepPath}.context_from`).entries()) {
      const source = reference.split(".", 1)[0];
      if (!new Set(["change_set", "thread", "input"]).has(source) && !seen.has(source)) {
        findings.push({ code: "step.unknown_context_source", path: `${stepPath}.context_from[${referenceIndex}]`, message: "Context may reference only inputs or an earlier step." });
      }
    }
    seen.add(id);
    return {
      id,
      skill,
      scopes,
      mutating,
      inputs: record(step.inputs),
      context_from: strings(step.context_from, `${stepPath}.context_from`),
      description: requiredString(step.description, `${stepPath}.description`),
    };
  });
}

function emptyPlan() {
  return {
    plan_kind: "workspace_change",
    change_set: {},
    objective_summary: "",
    workspace_change_plan: {},
    orchestration_steps: [],
    required_skills: [],
    open_questions: [],
    source_change_set_preserved: false,
    source_thread_locator_preserved: false,
    catalog_skills: [],
    source_context_available: false,
  };
}

function sourceChangeSetFromInputs(inputs) {
  if (isRecord(inputs.change_set) && Object.keys(inputs.change_set).length > 0) return inputs.change_set;
  const thread = record(inputs.thread);
  if (isRecord(thread.change_set) && Object.keys(thread.change_set).length > 0) return thread.change_set;
  return null;
}

function isSyntheticTarget(value) {
  return /^(?:repo|repository|repo[-_][a-z0-9]+|example|synthetic)(?:[-_].*)?$/iu.test(value.trim());
}

function isPlaceholderCommand(value) {
  return /<[^>]+>|\b(?:TODO|TBD|CHANGEME|PLACEHOLDER)\b|example\.com|\.\.\./iu.test(value);
}

function validateCanonicalEndpoints(plan, sourceChangeSet, findings) {
  if (!sourceChangeSet) return;
  const sourceUrls = collectStrings(sourceChangeSet)
    .map(parseUrl)
    .filter(Boolean);
  if (sourceUrls.length === 0) return;
  const sourceHosts = new Set(sourceUrls.map((url) => url.hostname.toLowerCase()));
  const planUrls = collectStrings(plan)
    .map(parseUrl)
    .filter(Boolean);
  for (const url of planUrls) {
    const hostname = url.hostname.toLowerCase();
    const sameDomain = sourceUrls.some((source) => registrableSuffix(source.hostname) === registrableSuffix(hostname));
    if (sameDomain && !sourceHosts.has(hostname)) {
      findings.push({ code: "endpoint.canonical_drift", path: "workspace_change_plan", message: `Endpoint host '${hostname}' differs from the source canonical host.` });
      return;
    }
  }
}

function collectStrings(value) {
  if (typeof value === "string") return [value];
  if (Array.isArray(value)) return value.flatMap(collectStrings);
  if (isRecord(value)) return Object.values(value).flatMap(collectStrings);
  return [];
}

function parseUrl(value) {
  try {
    return /^https?:\/\//iu.test(value) ? new URL(value) : null;
  } catch {
    return null;
  }
}

function registrableSuffix(hostname) {
  const parts = hostname.toLowerCase().split(".").filter(Boolean);
  return parts.slice(-2).join(".");
}

function normalizedSkillName(value) {
  return String(value || "").trim().replace(/^\.\.\//u, "").replace(/^skills\//u, "").split("/").at(-1);
}

function identifier(value, field) {
  const parsed = requiredString(value, field);
  if (!/^[a-z0-9][a-z0-9-]*$/u.test(parsed)) throw new Error(`${field} must be kebab-case`);
  return parsed;
}

function strings(value, field) {
  if (!Array.isArray(value)) throw new Error(`${field} must be an array`);
  return value.map((entry, index) => requiredString(entry, `${field}[${index}]`));
}

function records(value, field) {
  if (!Array.isArray(value)) throw new Error(`${field} must be an array`);
  return value.map((entry, index) => requiredRecord(entry, `${field}[${index}]`));
}

function requiredBoolean(value, field) {
  if (typeof value !== "boolean") throw new Error(`${field} must be boolean`);
  return value;
}

function enumValue(value, allowed, field) {
  if (!allowed.includes(value)) throw new Error(`${field} must be one of ${allowed.join(", ")}`);
  return value;
}

function requiredString(value, field) {
  const parsed = optionalString(value);
  if (!parsed) throw new Error(`${field} must be a non-empty string`);
  return parsed;
}

function optionalString(value) {
  return typeof value === "string" && value.trim() ? value.trim() : null;
}

function isRecord(value) {
  return value && typeof value === "object" && !Array.isArray(value);
}

function record(value) {
  return isRecord(value) ? value : {};
}

function requiredRecord(value, field) {
  const parsed = record(value);
  if (Object.keys(parsed).length === 0) throw new Error(`${field} must be a non-empty object`);
  return parsed;
}

function boundedMessage(error) {
  return (error instanceof Error ? error.message : "Invalid work plan").replace(/\s+/gu, " ").slice(0, 180);
}

function equalJson(left, right) {
  if (left === right) return true;
  if (Array.isArray(left) || Array.isArray(right)) {
    return Array.isArray(left)
      && Array.isArray(right)
      && left.length === right.length
      && left.every((value, index) => equalJson(value, right[index]));
  }
  if (!isRecord(left) || !isRecord(right)) return false;
  const leftKeys = Object.keys(left).sort();
  const rightKeys = Object.keys(right).sort();
  return leftKeys.length === rightKeys.length
    && leftKeys.every((key, index) => key === rightKeys[index] && equalJson(left[key], right[key]));
}
