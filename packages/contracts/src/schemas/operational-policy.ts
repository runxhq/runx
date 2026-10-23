import {
  generatedSchema,
  validateContractSchema,
} from "../internal.js";
import type {
  OperationalPolicyActionContract,
  OperationalPolicyContract,
  OperationalPolicyReadbackContract,
  OperationalPolicyValidationFindingContract,
} from "../generated/contract-types.js";

export const operationalPolicySchemaVersion = "runx.operational_policy.v1" as const;

export const operationalPolicyActions = [
  "reply-only",
  "issue-intake",
  "work-plan",
  "issue-to-pr",
  "manual-review",
  "pr-review",
  "pr-fix-up",
  "merge-assist",
] as const;

export const operationalPolicyRunnerStates = [
  "available",
  "disabled",
  "maintenance",
] as const;

export const operationalPolicyDedupeStrategies = [
  "source_digest",
  "provider_search",
  "branch",
] as const;

export const operationalPolicyOutcomeCloseModes = [
  "never",
  "when_verified",
  "when_terminal",
] as const;

export const operationalPolicySchema =
  generatedSchema<OperationalPolicyContract>("operational-policy.schema.json");

/** What a call asks the policy to admit. */
export interface OperationalPolicyAdmissionRequest {
  readonly source_id?: string;
  readonly target_repo?: string;
  readonly action: OperationalPolicyActionContract;
  readonly runner_id?: string;
  readonly source_thread_locator?: string;
}

export interface OperationalPolicyAdmission {
  readonly status: "allow" | "deny";
  readonly findings: readonly OperationalPolicyValidationFindingContract[];
  readonly policy_id: string;
  readonly source_id?: string;
  readonly target_repo?: string;
  readonly runner_id?: string;
  readonly owner_route_id?: string;
  readonly owners?: readonly string[];
  readonly dedupe_strategy: string;
  readonly outcome_close_mode: string;
  readonly source_thread_required: boolean;
  readonly mutate_target_repo: boolean;
  readonly require_human_merge_gate: boolean;
}

export function validateOperationalPolicyContract(
  value: unknown,
  label = "operational_policy",
): OperationalPolicyContract {
  return validateContractSchema(operationalPolicySchema, value, label);
}

export function lintOperationalPolicyContract(
  value: unknown,
): readonly OperationalPolicyValidationFindingContract[] {
  const policy = validateOperationalPolicyContract(value);
  const findings: OperationalPolicyValidationFindingContract[] = [];
  const runnerIds = new Set(policy.runners.map((runner) => runner.runner_id));
  const ownerRouteIds = new Set(policy.owner_routes.map((route) => route.route_id));

  collectDuplicateIds(policy.sources.map((source) => source.source_id), "sources", "source_id", findings);
  collectDuplicateIds(policy.runners.map((runner) => runner.runner_id), "runners", "runner_id", findings);
  collectDuplicateIds(policy.owner_routes.map((route) => route.route_id), "owner_routes", "route_id", findings);
  collectDuplicateIds(policy.targets.map((target) => target.repo), "targets", "repo", findings);

  policy.sources.forEach((source, sourceIndex) => {
    if (
      source.allowed_actions.some((action) => action === "issue-to-pr" || action === "pr-fix-up" || action === "merge-assist") &&
      (!source.source_thread.required || source.source_thread.publish_mode === "none")
    ) {
      findings.push({
        code: "source_thread_required",
        path: `/sources/${sourceIndex}/source_thread`,
        message: `source '${source.source_id}' allows issue/PR automation but does not require source-thread publishing.`,
      });
    }
  });

  policy.targets.forEach((target, targetIndex) => {
    if (!ownerRouteIds.has(target.default_owner_route)) {
      findings.push({
        code: "unknown_owner_route",
        path: `/targets/${targetIndex}/default_owner_route`,
        message: `target '${target.repo}' references unknown owner route '${target.default_owner_route}'.`,
      });
    }
    const ownerRoute = policy.owner_routes.find((route) => route.route_id === target.default_owner_route);
    if (ownerRoute && !ownerRoute.target_repos.includes(target.repo)) {
      findings.push({
        code: "owner_route_target_mismatch",
        path: `/targets/${targetIndex}/default_owner_route`,
        message: `owner route '${ownerRoute.route_id}' does not cover target repo '${target.repo}'.`,
      });
    }

    const targetActionCoverage = new Map<OperationalPolicyActionContract, boolean>(
      target.allowed_actions.map((action) => [action, false]),
    );
    target.runner_ids.forEach((runnerId, runnerIndex) => {
      if (!runnerIds.has(runnerId)) {
        findings.push({
          code: "unknown_runner",
          path: `/targets/${targetIndex}/runner_ids/${runnerIndex}`,
          message: `target '${target.repo}' references unknown runner '${runnerId}'.`,
        });
        return;
      }
      const runner = policy.runners.find((candidate) => candidate.runner_id === runnerId);
      if (!runner) {
        return;
      }
      if (!runner.target_repos.includes(target.repo)) {
        findings.push({
          code: "runner_target_mismatch",
          path: `/targets/${targetIndex}/runner_ids/${runnerIndex}`,
          message: `runner '${runner.runner_id}' does not allow target repo '${target.repo}'.`,
        });
      }
      if (target.scafld_required && !runner.scafld_required) {
        findings.push({
          code: "runner_scafld_mismatch",
          path: `/targets/${targetIndex}/runner_ids/${runnerIndex}`,
          message: `target '${target.repo}' requires scafld but runner '${runner.runner_id}' does not.`,
        });
      }
      if (runner.state === "available") {
        for (const action of target.allowed_actions) {
          if (runner.allowed_actions.includes(action)) {
            targetActionCoverage.set(action, true);
          }
        }
      }
    });

    for (const [action, covered] of targetActionCoverage.entries()) {
      if (!covered) {
        findings.push({
          code: "target_action_without_runner",
          path: `/targets/${targetIndex}/allowed_actions`,
          message: `target '${target.repo}' allows '${action}' but no available runner supports it.`,
        });
      }
    }
  });

  if (policy.outcomes.publish_final_source_thread_update && !policy.sources.some((source) => source.source_thread.required)) {
    findings.push({
      code: "outcome_without_source_thread",
      path: "/outcomes/publish_final_source_thread_update",
      message: "final source-thread updates require at least one source with source_thread.required=true.",
    });
  }
  if (policy.outcomes.close_source_issue === "when_verified" && !policy.outcomes.verification_required) {
    findings.push({
      code: "close_without_verification",
      path: "/outcomes/close_source_issue",
      message: "close_source_issue=when_verified requires verification_required=true.",
    });
  }
  if (policy.permissions.mutate_target_repo && policy.targets.some((target) => !target.scafld_required)) {
    findings.push({
      code: "mutation_without_scafld",
      path: "/permissions/mutate_target_repo",
      message: "mutating target repo policy requires every target to set scafld_required=true.",
    });
  }

  return findings;
}

export function validateOperationalPolicySemantics(
  value: unknown,
  label = "operational_policy",
): OperationalPolicyContract {
  const policy = validateOperationalPolicyContract(value, label);
  const findings = lintOperationalPolicyContract(policy);
  if (findings.length > 0) {
    const first = findings[0];
    throw new Error(`${label}${first.path} failed semantic validation (${first.code}): ${first.message}`);
  }
  return policy;
}

export function admitOperationalPolicyRequest(
  value: unknown,
  request: OperationalPolicyAdmissionRequest,
): OperationalPolicyAdmission {
  const policy = validateOperationalPolicyContract(value);
  const findings: OperationalPolicyValidationFindingContract[] = [...lintOperationalPolicyContract(policy)];
  const source = selectRequestSource(policy, request, findings);
  const target = selectRequestTarget(policy, request, findings);
  const runner = selectRequestRunner(policy, request, target, findings);
  const ownerRoute = target
    ? policy.owner_routes.find((route) => route.route_id === target.default_owner_route)
    : undefined;

  if (source && !source.allowed_actions.includes(request.action)) {
    findings.push({
      code: "source_action_not_allowed",
      path: "/request/action",
      message: `source '${source.source_id}' does not allow action '${request.action}'.`,
    });
  }
  if (source?.source_thread.required && !nonEmptyString(request.source_thread_locator)) {
    findings.push({
      code: "source_thread_locator_required",
      path: "/request/source_thread_locator",
      message: `source '${source.source_id}' requires recoverable source-thread routing.`,
    });
  }
  if (target && !target.allowed_actions.includes(request.action)) {
    findings.push({
      code: "target_action_not_allowed",
      path: "/request/action",
      message: `target '${target.repo}' does not allow action '${request.action}'.`,
    });
  }
  if (runner) {
    if (runner.state !== "available") {
      findings.push({
        code: "runner_unavailable",
        path: "/request/runner_id",
        message: `runner '${runner.runner_id}' is '${runner.state}', not available.`,
      });
    }
    if (!runner.allowed_actions.includes(request.action)) {
      findings.push({
        code: "runner_action_not_allowed",
        path: "/request/action",
        message: `runner '${runner.runner_id}' does not allow action '${request.action}'.`,
      });
    }
    if (target && !runner.target_repos.includes(target.repo)) {
      findings.push({
        code: "runner_target_not_allowed",
        path: "/request/target_repo",
        message: `runner '${runner.runner_id}' does not allow target repo '${target.repo}'.`,
      });
    }
  }

  return {
    status: findings.length === 0 ? "allow" : "deny",
    findings,
    policy_id: policy.policy_id,
    source_id: source?.source_id,
    target_repo: target?.repo,
    runner_id: runner?.runner_id,
    owner_route_id: ownerRoute?.route_id,
    owners: ownerRoute?.owners,
    dedupe_strategy: policy.dedupe.strategy,
    outcome_close_mode: policy.outcomes.close_source_issue,
    source_thread_required: source?.source_thread.required ?? false,
    mutate_target_repo: policy.permissions.mutate_target_repo,
    require_human_merge_gate: policy.permissions.require_human_merge_gate,
  };
}

export function projectOperationalPolicyReadback(
  value: unknown,
): OperationalPolicyReadbackContract {
  const policy = validateOperationalPolicyContract(value);
  const findings = lintOperationalPolicyContract(policy);
  return {
    policy_id: policy.policy_id,
    schema_version: policy.schema_version,
    valid: findings.length === 0,
    findings,
    sources: policy.sources.map((source) => ({
      source_id: source.source_id,
      provider: source.provider,
      locator_count: source.allowed_locators.length,
      allowed_actions: source.allowed_actions,
      source_thread_required: source.source_thread.required,
      publish_mode: source.source_thread.publish_mode,
    })),
    runners: policy.runners.map((runner) => ({
      runner_id: runner.runner_id,
      kind: runner.kind,
      state: runner.state,
      target_repos: runner.target_repos,
      allowed_actions: runner.allowed_actions,
      scafld_required: runner.scafld_required,
    })),
    targets: policy.targets.map((target) => {
      const ownerRoute = policy.owner_routes.find((route) => route.route_id === target.default_owner_route);
      return {
        repo: target.repo,
        runner_ids: target.runner_ids,
        default_owner_route: target.default_owner_route,
        owner_count: ownerRoute?.owners.length ?? 0,
        allowed_actions: target.allowed_actions,
        scafld_required: target.scafld_required,
        available_runner_count: target.runner_ids
          .map((runnerId) => policy.runners.find((runner) => runner.runner_id === runnerId))
          .filter((runner) => runner?.state === "available")
          .length,
      };
    }),
    outcomes: policy.outcomes,
    permissions: policy.permissions,
  };
}

function selectRequestSource(
  policy: OperationalPolicyContract,
  request: OperationalPolicyAdmissionRequest,
  findings: OperationalPolicyValidationFindingContract[],
): OperationalPolicyContract["sources"][number] | undefined {
  if (request.source_id) {
    const source = policy.sources.find((candidate) => candidate.source_id === request.source_id);
    if (!source) {
      findings.push({
        code: "unknown_source",
        path: "/request/source_id",
        message: `request references unknown source '${request.source_id}'.`,
      });
    }
    return source;
  }
  if (policy.sources.length === 1) {
    return policy.sources[0];
  }
  findings.push({
    code: "source_required",
    path: "/request/source_id",
    message: "request must identify a source when policy contains multiple sources.",
  });
  return undefined;
}

function selectRequestTarget(
  policy: OperationalPolicyContract,
  request: OperationalPolicyAdmissionRequest,
  findings: OperationalPolicyValidationFindingContract[],
): OperationalPolicyContract["targets"][number] | undefined {
  const targetRepo = nonEmptyString(request.target_repo);
  if (!targetRepo) {
    findings.push({
      code: "target_repo_required",
      path: "/request/target_repo",
      message: "request must identify a target repo.",
    });
    return undefined;
  }
  const target = policy.targets.find((candidate) => candidate.repo === targetRepo);
  if (!target) {
    findings.push({
      code: "unknown_target_repo",
      path: "/request/target_repo",
      message: `request references unknown target repo '${targetRepo}'.`,
    });
  }
  return target;
}

function selectRequestRunner(
  policy: OperationalPolicyContract,
  request: OperationalPolicyAdmissionRequest,
  target: OperationalPolicyContract["targets"][number] | undefined,
  findings: OperationalPolicyValidationFindingContract[],
): OperationalPolicyContract["runners"][number] | undefined {
  if (request.runner_id) {
    const runner = policy.runners.find((candidate) => candidate.runner_id === request.runner_id);
    if (!runner) {
      findings.push({
        code: "unknown_runner",
        path: "/request/runner_id",
        message: `request references unknown runner '${request.runner_id}'.`,
      });
    } else if (target && !target.runner_ids.includes(runner.runner_id)) {
      findings.push({
        code: "runner_not_allowed_for_target",
        path: "/request/runner_id",
        message: `target '${target.repo}' does not allow runner '${runner.runner_id}'.`,
      });
    }
    return runner;
  }
  if (!target) {
    return undefined;
  }
  const runner = target.runner_ids
    .map((runnerId) => policy.runners.find((candidate) => candidate.runner_id === runnerId))
    .find((candidate) => candidate?.state === "available" && candidate.allowed_actions.includes(request.action));
  if (!runner) {
    findings.push({
      code: "runner_required",
      path: "/request/runner_id",
      message: `request needs an available runner for target '${target.repo}' and action '${request.action}'.`,
    });
  }
  return runner;
}

function nonEmptyString(value: unknown): string | undefined {
  return typeof value === "string" && value.trim().length > 0 ? value.trim() : undefined;
}

function collectDuplicateIds(
  ids: readonly string[],
  collectionName: string,
  fieldName: string,
  findings: OperationalPolicyValidationFindingContract[],
): void {
  const seen = new Set<string>();
  ids.forEach((id, index) => {
    if (!seen.has(id)) {
      seen.add(id);
      return;
    }
    findings.push({
      code: "duplicate_id",
      path: `/${collectionName}/${index}/${fieldName}`,
      message: `${collectionName}.${fieldName} '${id}' must be unique.`,
    });
  });
}
