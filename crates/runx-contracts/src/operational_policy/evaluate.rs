// Module rationale: operational-policy validation, admission,
// and readback projection share a single fixture-driven contract surface that
// stays adjacent until the cross-language oracle splits validation tracks.
use std::collections::{BTreeMap, BTreeSet};

use super::{
    OperationalPolicy, OperationalPolicyAction, OperationalPolicyAdmission,
    OperationalPolicyAdmissionRequest, OperationalPolicyAdmissionStatus,
    OperationalPolicyAutomationPermissions, OperationalPolicyDedupePolicy, OperationalPolicyError,
    OperationalPolicyOutcomeCloseMode, OperationalPolicyOwnerRoute, OperationalPolicyPublishMode,
    OperationalPolicyReadback, OperationalPolicyRunnerReadback, OperationalPolicyRunnerRule,
    OperationalPolicyRunnerState, OperationalPolicySourceReadback, OperationalPolicySourceRule,
    OperationalPolicyTargetReadback, OperationalPolicyTargetRule,
    OperationalPolicyValidationFinding, action_name,
};

pub fn validate_operational_policy_contract(
    policy: &OperationalPolicy,
) -> Result<(), OperationalPolicyError> {
    validate_required_shape(policy).map_err(OperationalPolicyError::Contract)
}

pub fn lint_operational_policy_contract(
    policy: &OperationalPolicy,
) -> Result<Vec<OperationalPolicyValidationFinding>, OperationalPolicyError> {
    validate_operational_policy_contract(policy)?;
    Ok(collect_semantic_findings(policy))
}

pub fn validate_operational_policy_semantics(
    policy: &OperationalPolicy,
) -> Result<(), OperationalPolicyError> {
    let findings = lint_operational_policy_contract(policy)?;
    if let Some(finding) = findings.into_iter().next() {
        return Err(OperationalPolicyError::Semantic(finding));
    }
    Ok(())
}

pub fn admit_operational_policy_request(
    policy: &OperationalPolicy,
    request: &OperationalPolicyAdmissionRequest,
) -> Result<OperationalPolicyAdmission, OperationalPolicyError> {
    validate_operational_policy_contract(policy)?;

    let mut findings = collect_semantic_findings(policy);
    let source = select_request_source(policy, request, &mut findings);
    let target = select_request_target(policy, request, &mut findings);
    let runner = select_request_runner(policy, request, target, &mut findings);
    let owner_route = target.and_then(|target| {
        policy
            .owner_routes
            .iter()
            .find(|route| route.route_id == target.default_owner_route)
    });

    validate_admitted_source(source, request, &mut findings);
    validate_admitted_target(target, request, &mut findings);
    validate_admitted_runner(runner, target, request, &mut findings);

    Ok(OperationalPolicyAdmission {
        status: admission_status(&findings),
        findings,
        policy_id: policy.policy_id.to_string(),
        source_id: source.map(|source| source.source_id.to_string()),
        target_repo: target.map(|target| target.repo.clone()),
        runner_id: runner.map(|runner| runner.runner_id.to_string()),
        owner_route_id: owner_route.map(|route| route.route_id.to_string()),
        owners: owner_route.map(|route| route.owners.iter().map(ToString::to_string).collect()),
        dedupe_strategy: policy.dedupe.strategy,
        outcome_close_mode: policy.outcomes.close_source_issue,
        source_thread_required: source.is_some_and(|source| source.source_thread.required),
        mutate_target_repo: policy.permissions.mutate_target_repo,
        require_human_merge_gate: policy.permissions.require_human_merge_gate,
    })
}

fn validate_admitted_source(
    source: Option<&OperationalPolicySourceRule>,
    request: &OperationalPolicyAdmissionRequest,
    findings: &mut Vec<OperationalPolicyValidationFinding>,
) {
    if let Some(source) = source {
        if !source.allowed_actions.contains(&request.action) {
            findings.push(finding(
                "source_action_not_allowed",
                "/request/action",
                &format!(
                    "source '{}' does not allow action '{}'.",
                    source.source_id, request.action
                ),
            ));
        }
        if source.source_thread.required
            && non_empty_string(&request.source_thread_locator).is_none()
        {
            findings.push(finding(
                "source_thread_locator_required",
                "/request/source_thread_locator",
                &format!(
                    "source '{}' requires recoverable source-thread routing.",
                    source.source_id
                ),
            ));
        }
    }
}

fn validate_admitted_target(
    target: Option<&OperationalPolicyTargetRule>,
    request: &OperationalPolicyAdmissionRequest,
    findings: &mut Vec<OperationalPolicyValidationFinding>,
) {
    if let Some(target) = target
        && !target.allowed_actions.contains(&request.action)
    {
        findings.push(finding(
            "target_action_not_allowed",
            "/request/action",
            &format!(
                "target '{}' does not allow action '{}'.",
                target.repo, request.action
            ),
        ));
    }
}

fn validate_admitted_runner(
    runner: Option<&OperationalPolicyRunnerRule>,
    target: Option<&OperationalPolicyTargetRule>,
    request: &OperationalPolicyAdmissionRequest,
    findings: &mut Vec<OperationalPolicyValidationFinding>,
) {
    if let Some(runner) = runner {
        if runner.state != OperationalPolicyRunnerState::Available {
            findings.push(finding(
                "runner_unavailable",
                "/request/runner_id",
                &format!(
                    "runner '{}' is '{}', not available.",
                    runner.runner_id, runner.state
                ),
            ));
        }
        if !runner.allowed_actions.contains(&request.action) {
            findings.push(finding(
                "runner_action_not_allowed",
                "/request/action",
                &format!(
                    "runner '{}' does not allow action '{}'.",
                    runner.runner_id, request.action
                ),
            ));
        }
        if let Some(target) = target
            && !runner.target_repos.contains(&target.repo)
        {
            findings.push(finding(
                "runner_target_not_allowed",
                "/request/target_repo",
                &format!(
                    "runner '{}' does not allow target repo '{}'.",
                    runner.runner_id, target.repo
                ),
            ));
        }
    }
}

fn admission_status(
    findings: &[OperationalPolicyValidationFinding],
) -> OperationalPolicyAdmissionStatus {
    if findings.is_empty() {
        OperationalPolicyAdmissionStatus::Allow
    } else {
        OperationalPolicyAdmissionStatus::Deny
    }
}

pub fn project_operational_policy_readback(
    policy: &OperationalPolicy,
) -> Result<OperationalPolicyReadback, OperationalPolicyError> {
    let findings = lint_operational_policy_contract(policy)?;
    Ok(OperationalPolicyReadback {
        policy_id: policy.policy_id.to_string(),
        schema_version: policy.schema_version,
        valid: findings.is_empty(),
        findings,
        sources: policy.sources.iter().map(source_readback).collect(),
        runners: policy.runners.iter().map(runner_readback).collect(),
        targets: policy
            .targets
            .iter()
            .map(|target| target_readback(policy, target))
            .collect(),
        outcomes: policy.outcomes.clone(),
        permissions: policy.permissions.clone(),
    })
}

fn select_request_source<'a>(
    policy: &'a OperationalPolicy,
    request: &OperationalPolicyAdmissionRequest,
    findings: &mut Vec<OperationalPolicyValidationFinding>,
) -> Option<&'a OperationalPolicySourceRule> {
    if let Some(source_id) = non_empty_string(&request.source_id) {
        let source = policy
            .sources
            .iter()
            .find(|candidate| candidate.source_id == source_id);
        if source.is_none() {
            findings.push(finding(
                "unknown_source",
                "/request/source_id",
                &format!("request references unknown source '{source_id}'."),
            ));
        }
        return source;
    }
    if policy.sources.len() == 1 {
        return policy.sources.first();
    }
    findings.push(finding(
        "source_required",
        "/request/source_id",
        "request must identify a source when policy contains multiple sources.",
    ));
    None
}

fn select_request_target<'a>(
    policy: &'a OperationalPolicy,
    request: &OperationalPolicyAdmissionRequest,
    findings: &mut Vec<OperationalPolicyValidationFinding>,
) -> Option<&'a OperationalPolicyTargetRule> {
    let Some(target_repo) = non_empty_string(&request.target_repo) else {
        findings.push(finding(
            "target_repo_required",
            "/request/target_repo",
            "request must identify a target repo.",
        ));
        return None;
    };

    let target = policy
        .targets
        .iter()
        .find(|candidate| candidate.repo == target_repo);
    if target.is_none() {
        findings.push(finding(
            "unknown_target_repo",
            "/request/target_repo",
            &format!("request references unknown target repo '{target_repo}'."),
        ));
    }
    target
}

fn select_request_runner<'a>(
    policy: &'a OperationalPolicy,
    request: &OperationalPolicyAdmissionRequest,
    target: Option<&OperationalPolicyTargetRule>,
    findings: &mut Vec<OperationalPolicyValidationFinding>,
) -> Option<&'a OperationalPolicyRunnerRule> {
    if let Some(runner_id) = non_empty_string(&request.runner_id) {
        let runner = resolve_runner(policy, runner_id);
        if runner.is_none() {
            findings.push(finding(
                "unknown_runner",
                "/request/runner_id",
                &format!("request references unknown runner '{runner_id}'."),
            ));
        } else if let Some(target) = target
            && !target_admits_runner(target, runner_id)
        {
            findings.push(finding(
                "runner_not_allowed_for_target",
                "/request/runner_id",
                &format!(
                    "target '{}' does not allow runner '{}'.",
                    target.repo, runner_id
                ),
            ));
        }
        return runner;
    }

    let target = target?;
    let runner = target
        .runner_ids
        .iter()
        .filter_map(|runner_id| resolve_runner(policy, runner_id))
        .find(|candidate| runner_ready_for_action(candidate, &request.action));
    if runner.is_none() {
        findings.push(finding(
            "runner_required",
            "/request/runner_id",
            &format!(
                "request needs an available runner for target '{}' and action '{}'.",
                target.repo, request.action
            ),
        ));
    }
    runner
}

/// Resolve a runner id to the matching runner rule, if any. Shared by the
/// explicit-runner admission path and the auto-select filter so both entry
/// points perform runner lookup identically.
fn resolve_runner<'a>(
    policy: &'a OperationalPolicy,
    runner_id: &str,
) -> Option<&'a OperationalPolicyRunnerRule> {
    policy
        .runners
        .iter()
        .find(|candidate| candidate.runner_id == runner_id)
}

/// Whether the target lists this runner id among its allowed runners. Names the
/// membership predicate the explicit path applies before emitting
/// `runner_not_allowed_for_target`. The auto path enforces the same membership
/// structurally by iterating `target.runner_ids` directly.
fn target_admits_runner(target: &OperationalPolicyTargetRule, runner_id: &str) -> bool {
    target.runner_ids.iter().any(|id| id == runner_id)
}

/// Whether a runner is available and permits the requested action. Used only by
/// the auto-select filter; the explicit path defers this check to
/// `validate_admitted_runner` and intentionally does not apply it here.
fn runner_ready_for_action(
    runner: &OperationalPolicyRunnerRule,
    action: &OperationalPolicyAction,
) -> bool {
    runner.state == OperationalPolicyRunnerState::Available
        && runner.allowed_actions.contains(action)
}

fn source_readback(source: &OperationalPolicySourceRule) -> OperationalPolicySourceReadback {
    OperationalPolicySourceReadback {
        source_id: source.source_id.to_string(),
        provider: source.provider.clone(),
        locator_count: source.allowed_locators.len(),
        allowed_actions: source.allowed_actions.clone(),
        source_thread_required: source.source_thread.required,
        publish_mode: source.source_thread.publish_mode,
    }
}

fn runner_readback(runner: &OperationalPolicyRunnerRule) -> OperationalPolicyRunnerReadback {
    OperationalPolicyRunnerReadback {
        runner_id: runner.runner_id.to_string(),
        kind: runner.kind.clone(),
        state: runner.state,
        target_repos: runner.target_repos.clone(),
        allowed_actions: runner.allowed_actions.clone(),
        scafld_required: runner.scafld_required,
    }
}

fn target_readback(
    policy: &OperationalPolicy,
    target: &OperationalPolicyTargetRule,
) -> OperationalPolicyTargetReadback {
    let owner_count = policy
        .owner_routes
        .iter()
        .find(|route| route.route_id == target.default_owner_route)
        .map_or(0, |route| route.owners.len());
    let available_runner_count = target
        .runner_ids
        .iter()
        .filter_map(|runner_id| {
            policy
                .runners
                .iter()
                .find(|runner| &runner.runner_id == runner_id)
        })
        .filter(|runner| runner.state == OperationalPolicyRunnerState::Available)
        .count();

    OperationalPolicyTargetReadback {
        repo: target.repo.clone(),
        runner_ids: target.runner_ids.iter().map(ToString::to_string).collect(),
        default_owner_route: target.default_owner_route.to_string(),
        owner_count,
        allowed_actions: target.allowed_actions.clone(),
        scafld_required: target.scafld_required,
        available_runner_count,
    }
}

fn validate_required_shape(
    policy: &OperationalPolicy,
) -> Result<(), OperationalPolicyValidationFinding> {
    require_id(&policy.policy_id, "/policy_id", "policy_id")?;
    require_optional_date_time(&policy.created_at, "/created_at")?;
    require_non_empty(&policy.sources, "/sources", "sources")?;
    require_non_empty(&policy.runners, "/runners", "runners")?;
    require_non_empty(&policy.owner_routes, "/owner_routes", "owner_routes")?;
    require_non_empty(&policy.targets, "/targets", "targets")?;
    validate_sources(&policy.sources)?;
    validate_runners(&policy.runners)?;
    validate_owner_routes(&policy.owner_routes)?;
    validate_targets(&policy.targets)?;
    validate_dedupe(&policy.dedupe)?;
    validate_permissions(&policy.permissions)?;
    Ok(())
}

fn validate_sources(
    sources: &[OperationalPolicySourceRule],
) -> Result<(), OperationalPolicyValidationFinding> {
    for (index, source) in sources.iter().enumerate() {
        require_id(
            &source.source_id,
            &format!("/sources/{index}/source_id"),
            "source_id",
        )?;
        require_string_items(
            &source.allowed_locators,
            &format!("/sources/{index}/allowed_locators"),
            "allowed_locators",
        )?;
        require_non_empty(
            &source.allowed_actions,
            &format!("/sources/{index}/allowed_actions"),
            "allowed_actions",
        )?;
        if let Some(confidence) = source.minimum_confidence {
            require_unit_interval(
                confidence,
                &format!("/sources/{index}/minimum_confidence"),
                "minimum_confidence",
            )?;
        }
    }
    Ok(())
}

fn validate_runners(
    runners: &[OperationalPolicyRunnerRule],
) -> Result<(), OperationalPolicyValidationFinding> {
    for (index, runner) in runners.iter().enumerate() {
        require_id(
            &runner.runner_id,
            &format!("/runners/{index}/runner_id"),
            "runner_id",
        )?;
        require_non_empty(
            &runner.allowed_actions,
            &format!("/runners/{index}/allowed_actions"),
            "allowed_actions",
        )?;
        require_repo_items(
            &runner.target_repos,
            &format!("/runners/{index}/target_repos"),
            "target_repos",
        )?;
    }
    Ok(())
}

fn validate_owner_routes(
    routes: &[OperationalPolicyOwnerRoute],
) -> Result<(), OperationalPolicyValidationFinding> {
    for (index, route) in routes.iter().enumerate() {
        require_id(
            &route.route_id,
            &format!("/owner_routes/{index}/route_id"),
            "route_id",
        )?;
        require_string_items(
            &route.owners,
            &format!("/owner_routes/{index}/owners"),
            "owners",
        )?;
        require_repo_items(
            &route.target_repos,
            &format!("/owner_routes/{index}/target_repos"),
            "target_repos",
        )?;
        require_optional_string(&route.project, &format!("/owner_routes/{index}/project"))?;
        require_string_items_if_present(&route.labels, &format!("/owner_routes/{index}/labels"))?;
    }
    Ok(())
}

fn validate_targets(
    targets: &[OperationalPolicyTargetRule],
) -> Result<(), OperationalPolicyValidationFinding> {
    for (index, target) in targets.iter().enumerate() {
        require_repo_slug(&target.repo, &format!("/targets/{index}/repo"))?;
        require_string_items(
            &target.runner_ids,
            &format!("/targets/{index}/runner_ids"),
            "runner_ids",
        )?;
        require_non_empty(
            &target.allowed_actions,
            &format!("/targets/{index}/allowed_actions"),
            "allowed_actions",
        )?;
        require_id(
            &target.default_owner_route,
            &format!("/targets/{index}/default_owner_route"),
            "default_owner_route",
        )?;
        require_optional_string(
            &target.base_branch,
            &format!("/targets/{index}/base_branch"),
        )?;
    }
    Ok(())
}

fn validate_dedupe(
    dedupe: &OperationalPolicyDedupePolicy,
) -> Result<(), OperationalPolicyValidationFinding> {
    require_string_items(&dedupe.key_fields, "/dedupe/key_fields", "key_fields")
}

fn validate_permissions(
    permissions: &OperationalPolicyAutomationPermissions,
) -> Result<(), OperationalPolicyValidationFinding> {
    if permissions.auto_merge {
        return Err(finding(
            "literal_false",
            "/permissions/auto_merge",
            "permissions.auto_merge must be false.",
        ));
    }
    if !permissions.require_human_merge_gate {
        return Err(finding(
            "literal_true",
            "/permissions/require_human_merge_gate",
            "permissions.require_human_merge_gate must be true.",
        ));
    }
    Ok(())
}

fn collect_semantic_findings(
    policy: &OperationalPolicy,
) -> Vec<OperationalPolicyValidationFinding> {
    let mut findings = Vec::new();
    collect_duplicates(policy, &mut findings);
    collect_source_findings(policy, &mut findings);
    collect_target_findings(policy, &mut findings);
    collect_outcome_findings(policy, &mut findings);
    findings
}

fn collect_duplicates(
    policy: &OperationalPolicy,
    findings: &mut Vec<OperationalPolicyValidationFinding>,
) {
    duplicate_findings(
        policy
            .sources
            .iter()
            .map(|source| source.source_id.as_str()),
        "sources",
        "source_id",
        findings,
    );
    duplicate_findings(
        policy
            .runners
            .iter()
            .map(|runner| runner.runner_id.as_str()),
        "runners",
        "runner_id",
        findings,
    );
    duplicate_findings(
        policy
            .owner_routes
            .iter()
            .map(|route| route.route_id.as_str()),
        "owner_routes",
        "route_id",
        findings,
    );
    duplicate_findings(
        policy.targets.iter().map(|target| target.repo.as_str()),
        "targets",
        "repo",
        findings,
    );
}

fn collect_source_findings(
    policy: &OperationalPolicy,
    findings: &mut Vec<OperationalPolicyValidationFinding>,
) {
    for (source_index, source) in policy.sources.iter().enumerate() {
        let automates_issue_or_pr = source.allowed_actions.iter().any(|action| {
            matches!(
                action,
                OperationalPolicyAction::IssueToPr
                    | OperationalPolicyAction::PrFixUp
                    | OperationalPolicyAction::MergeAssist
            )
        });
        if automates_issue_or_pr
            && (!source.source_thread.required
                || source.source_thread.publish_mode == OperationalPolicyPublishMode::None)
        {
            findings.push(finding(
                "source_thread_required",
                &format!("/sources/{source_index}/source_thread"),
                &format!(
                    "source '{}' allows issue/PR automation but does not require source-thread publishing.",
                    source.source_id
                ),
            ));
        }
    }
}

fn collect_target_findings(
    policy: &OperationalPolicy,
    findings: &mut Vec<OperationalPolicyValidationFinding>,
) {
    let runner_ids = policy
        .runners
        .iter()
        .map(|runner| runner.runner_id.as_str())
        .collect::<BTreeSet<_>>();
    let owner_route_ids = policy
        .owner_routes
        .iter()
        .map(|route| route.route_id.as_str())
        .collect::<BTreeSet<_>>();

    for (target_index, target) in policy.targets.iter().enumerate() {
        collect_owner_route_findings(policy, target, target_index, &owner_route_ids, findings);
        collect_runner_findings(policy, target, target_index, &runner_ids, findings);
    }
}

fn collect_owner_route_findings(
    policy: &OperationalPolicy,
    target: &OperationalPolicyTargetRule,
    target_index: usize,
    owner_route_ids: &BTreeSet<&str>,
    findings: &mut Vec<OperationalPolicyValidationFinding>,
) {
    if !owner_route_ids.contains(target.default_owner_route.as_str()) {
        findings.push(finding(
            "unknown_owner_route",
            &format!("/targets/{target_index}/default_owner_route"),
            &format!(
                "target '{}' references unknown owner route '{}'.",
                target.repo, target.default_owner_route
            ),
        ));
    }
    let owner_route = policy
        .owner_routes
        .iter()
        .find(|route| route.route_id == target.default_owner_route);
    if owner_route.is_some_and(|route| !route.target_repos.contains(&target.repo)) {
        findings.push(finding(
            "owner_route_target_mismatch",
            &format!("/targets/{target_index}/default_owner_route"),
            &format!(
                "owner route '{}' does not cover target repo '{}'.",
                target.default_owner_route, target.repo
            ),
        ));
    }
}

fn collect_runner_findings(
    policy: &OperationalPolicy,
    target: &OperationalPolicyTargetRule,
    target_index: usize,
    runner_ids: &BTreeSet<&str>,
    findings: &mut Vec<OperationalPolicyValidationFinding>,
) {
    let mut coverage = target
        .allowed_actions
        .iter()
        .map(|action| (*action, false))
        .collect::<BTreeMap<_, _>>();

    for (runner_index, runner_id) in target.runner_ids.iter().enumerate() {
        let runner = policy
            .runners
            .iter()
            .find(|runner| runner.runner_id == *runner_id);
        if !runner_ids.contains(runner_id.as_str()) {
            findings.push(finding(
                "unknown_runner",
                &format!("/targets/{target_index}/runner_ids/{runner_index}"),
                &format!(
                    "target '{}' references unknown runner '{}'.",
                    target.repo, runner_id
                ),
            ));
            continue;
        }
        if let Some(runner) = runner {
            collect_runner_target_findings(target, target_index, runner_index, runner, findings);
            mark_action_coverage(target, runner, &mut coverage);
        }
    }
    collect_action_coverage_findings(target, target_index, coverage, findings);
}

fn collect_runner_target_findings(
    target: &OperationalPolicyTargetRule,
    target_index: usize,
    runner_index: usize,
    runner: &OperationalPolicyRunnerRule,
    findings: &mut Vec<OperationalPolicyValidationFinding>,
) {
    if !runner.target_repos.contains(&target.repo) {
        findings.push(finding(
            "runner_target_mismatch",
            &format!("/targets/{target_index}/runner_ids/{runner_index}"),
            &format!(
                "runner '{}' does not allow target repo '{}'.",
                runner.runner_id, target.repo
            ),
        ));
    }
    if target.scafld_required && !runner.scafld_required {
        findings.push(finding(
            "runner_scafld_mismatch",
            &format!("/targets/{target_index}/runner_ids/{runner_index}"),
            &format!(
                "target '{}' requires scafld but runner '{}' does not.",
                target.repo, runner.runner_id
            ),
        ));
    }
}

fn mark_action_coverage(
    target: &OperationalPolicyTargetRule,
    runner: &OperationalPolicyRunnerRule,
    coverage: &mut BTreeMap<OperationalPolicyAction, bool>,
) {
    if runner.state != OperationalPolicyRunnerState::Available {
        return;
    }
    for action in &target.allowed_actions {
        if runner.allowed_actions.contains(action) {
            coverage.insert(*action, true);
        }
    }
}

fn collect_action_coverage_findings(
    target: &OperationalPolicyTargetRule,
    target_index: usize,
    coverage: BTreeMap<OperationalPolicyAction, bool>,
    findings: &mut Vec<OperationalPolicyValidationFinding>,
) {
    for (action, covered) in coverage {
        if !covered {
            findings.push(finding(
                "target_action_without_runner",
                &format!("/targets/{target_index}/allowed_actions"),
                &format!(
                    "target '{}' allows '{}' but no available runner supports it.",
                    target.repo,
                    action_name(action)
                ),
            ));
        }
    }
}

fn collect_outcome_findings(
    policy: &OperationalPolicy,
    findings: &mut Vec<OperationalPolicyValidationFinding>,
) {
    if policy.outcomes.publish_final_source_thread_update
        && !policy
            .sources
            .iter()
            .any(|source| source.source_thread.required)
    {
        findings.push(finding(
            "outcome_without_source_thread",
            "/outcomes/publish_final_source_thread_update",
            "final source-thread updates require at least one source with source_thread.required=true.",
        ));
    }
    if policy.outcomes.close_source_issue == OperationalPolicyOutcomeCloseMode::WhenVerified
        && !policy.outcomes.verification_required
    {
        findings.push(finding(
            "source_issue_closure_without_verification",
            "/outcomes/close_source_issue",
            "close_source_issue=when_verified requires verification_required=true.",
        ));
    }
    if policy.permissions.mutate_target_repo
        && policy.targets.iter().any(|target| !target.scafld_required)
    {
        findings.push(finding(
            "mutation_without_scafld",
            "/permissions/mutate_target_repo",
            "mutating target repo policy requires every target to set scafld_required=true.",
        ));
    }
}

fn duplicate_findings<'a>(
    ids: impl Iterator<Item = &'a str>,
    collection_name: &str,
    field_name: &str,
    findings: &mut Vec<OperationalPolicyValidationFinding>,
) {
    let mut seen = BTreeSet::new();
    for (index, id) in ids.enumerate() {
        if seen.insert(id) {
            continue;
        }
        findings.push(finding(
            "duplicate_id",
            &format!("/{collection_name}/{index}/{field_name}"),
            &format!("{collection_name}.{field_name} '{id}' must be unique."),
        ));
    }
}

fn require_id(
    value: &str,
    path: &str,
    field: &str,
) -> Result<(), OperationalPolicyValidationFinding> {
    if !value.is_empty() && value.chars().all(is_id_char) {
        return Ok(());
    }
    Err(finding(
        "invalid_id",
        path,
        &format!("{field} must match ^[A-Za-z0-9_.:-]+$."),
    ))
}

fn require_repo_slug(value: &str, path: &str) -> Result<(), OperationalPolicyValidationFinding> {
    let mut parts = value.split('/');
    let owner = parts.next();
    let repo = parts.next();
    if parts.next().is_none()
        && owner.is_some_and(valid_repo_part)
        && repo.is_some_and(valid_repo_part)
    {
        return Ok(());
    }
    Err(finding(
        "invalid_repo",
        path,
        "repo must match owner/repo with non-empty slug parts.",
    ))
}

fn require_repo_items<T: AsRef<str>>(
    values: &[T],
    path: &str,
    field: &str,
) -> Result<(), OperationalPolicyValidationFinding> {
    require_non_empty(values, path, field)?;
    for (index, value) in values.iter().enumerate() {
        require_repo_slug(value.as_ref(), &format!("{path}/{index}"))?;
    }
    Ok(())
}

fn require_string_items<T: AsRef<str>>(
    values: &[T],
    path: &str,
    field: &str,
) -> Result<(), OperationalPolicyValidationFinding> {
    require_non_empty(values, path, field)?;
    require_string_items_if_present(values, path)
}

fn require_string_items_if_present<T: AsRef<str>>(
    values: &[T],
    path: &str,
) -> Result<(), OperationalPolicyValidationFinding> {
    for (index, value) in values.iter().enumerate() {
        if value.as_ref().is_empty() {
            return Err(finding(
                "empty_string",
                &format!("{path}/{index}"),
                "string entries must not be empty.",
            ));
        }
    }
    Ok(())
}

fn require_optional_string<T: AsRef<str>>(
    value: &Option<T>,
    path: &str,
) -> Result<(), OperationalPolicyValidationFinding> {
    if value
        .as_ref()
        .is_some_and(|value| value.as_ref().is_empty())
    {
        return Err(finding("empty_string", path, "value must not be empty."));
    }
    Ok(())
}

fn require_optional_date_time<T: AsRef<str>>(
    value: &Option<T>,
    path: &str,
) -> Result<(), OperationalPolicyValidationFinding> {
    match value.as_ref().map(AsRef::as_ref) {
        Some(value) if !matches_ts_date_time_pattern(value) => Err(finding(
            "date_time",
            path,
            "value must match YYYY-MM-DDTHH:MM:SS(.fraction)?Z.",
        )),
        _ => Ok(()),
    }
}

fn matches_ts_date_time_pattern(value: &str) -> bool {
    let Some(prefix) = value.strip_suffix('Z') else {
        return false;
    };
    let Some((seconds_prefix, fraction)) = prefix.split_once('.') else {
        return matches_date_time_without_zone(prefix);
    };
    matches_date_time_without_zone(seconds_prefix)
        && !fraction.is_empty()
        && fraction.chars().all(|character| character.is_ascii_digit())
}

fn matches_date_time_without_zone(value: &str) -> bool {
    value.len() == 19
        && value.as_bytes().get(4) == Some(&b'-')
        && value.as_bytes().get(7) == Some(&b'-')
        && value.as_bytes().get(10) == Some(&b'T')
        && value.as_bytes().get(13) == Some(&b':')
        && value.as_bytes().get(16) == Some(&b':')
        && value.chars().enumerate().all(|(index, character)| {
            matches!(index, 4 | 7 | 10 | 13 | 16) || character.is_ascii_digit()
        })
}

fn require_non_empty<T>(
    values: &[T],
    path: &str,
    field: &str,
) -> Result<(), OperationalPolicyValidationFinding> {
    if values.is_empty() {
        return Err(finding(
            "min_items",
            path,
            &format!("{field} must contain at least one entry."),
        ));
    }
    Ok(())
}

fn require_unit_interval(
    value: f64,
    path: &str,
    field: &str,
) -> Result<(), OperationalPolicyValidationFinding> {
    if (0.0..=1.0).contains(&value) {
        return Ok(());
    }
    Err(finding(
        "range",
        path,
        &format!("{field} must be between 0 and 1."),
    ))
}

fn non_empty_string(value: &Option<String>) -> Option<&str> {
    let trimmed = value.as_deref()?.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

fn valid_repo_part(value: &str) -> bool {
    !value.is_empty() && value.chars().all(is_repo_char)
}

fn is_id_char(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '_' | '.' | ':' | '-')
}

fn is_repo_char(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '_' | '.' | '-')
}

fn finding(code: &str, path: &str, message: &str) -> OperationalPolicyValidationFinding {
    OperationalPolicyValidationFinding {
        code: code.into(),
        path: path.into(),
        message: message.into(),
    }
}
