use std::collections::BTreeSet;

use runx_contracts::schema::NonEmptyString;
use runx_contracts::{
    SkillArchitectureChoice, SkillArchitectureDecision, SkillChangeBundle, SkillChangeDecision,
    SkillExecutionLane, SkillIdentityAction, SkillPackageMetrics, SkillPackageVisibility,
    SkillProofKind,
};

use super::{RuntimeError, architecture_digest, invalid_skill_change};

pub(super) fn validate_change_contract(
    change: &SkillChangeBundle,
    mode: &str,
    current_digest: &str,
) -> Result<(), RuntimeError> {
    validate_digest("base_digest", change.base_digest.as_str())?;
    validate_digest("plan_digest", change.plan_digest.as_str())?;
    let expected_plan = architecture_digest(change.base_digest.as_str(), &change.architecture)?;
    if change.plan_digest.as_str() != expected_plan {
        return Err(invalid_skill_change(
            "plan_digest does not bind the supplied architecture decision",
        ));
    }
    validate_mode_decision(mode, change.decision)?;
    validate_change_shape(change)?;
    if change.decision != SkillChangeDecision::Write
        && change.base_digest.as_str() != current_digest
    {
        return Err(invalid_skill_change(
            "non-write decision is stale against the inspected base digest",
        ));
    }
    validate_architecture(change)
}

fn validate_mode_decision(mode: &str, decision: SkillChangeDecision) -> Result<(), RuntimeError> {
    let allowed = match mode {
        "build" => matches!(
            decision,
            SkillChangeDecision::Write
                | SkillChangeDecision::NoSkill
                | SkillChangeDecision::NeedsCore
        ),
        "improve" => matches!(
            decision,
            SkillChangeDecision::Write
                | SkillChangeDecision::NoChange
                | SkillChangeDecision::NeedsCore
        ),
        "harness" => matches!(
            decision,
            SkillChangeDecision::Write | SkillChangeDecision::NoChange
        ),
        _ => {
            return Err(invalid_skill_change(
                "mode must be build, improve, or harness",
            ));
        }
    };
    if allowed {
        return Ok(());
    }
    Err(invalid_skill_change(format!(
        "{} is not a valid decision in {mode} mode",
        decision_name(decision)
    )))
}

pub(super) fn validate_change_shape(change: &SkillChangeBundle) -> Result<(), RuntimeError> {
    if change.decision != SkillChangeDecision::Write
        && (!change.writes.is_empty() || !change.deletes.is_empty())
    {
        return Err(invalid_skill_change(
            "non-write bundles must not contain writes or deletes",
        ));
    }
    if change.decision == SkillChangeDecision::Write
        && change.writes.is_empty()
        && change.deletes.is_empty()
    {
        return Err(invalid_skill_change(
            "a write decision must contain at least one write or delete",
        ));
    }
    Ok(())
}

pub(super) fn validate_architecture(change: &SkillChangeBundle) -> Result<(), RuntimeError> {
    let outcome_matches = matches!(
        (change.decision, change.architecture.choice),
        (
            SkillChangeDecision::Write,
            SkillArchitectureChoice::Build | SkillArchitectureChoice::ExtendExisting
        ) | (
            SkillChangeDecision::NoChange,
            SkillArchitectureChoice::ExtendExisting
        ) | (
            SkillChangeDecision::NoSkill,
            SkillArchitectureChoice::NoSkill
        ) | (
            SkillChangeDecision::NeedsCore,
            SkillArchitectureChoice::NeedsCore
        )
    );
    if !outcome_matches {
        return Err(invalid_skill_change(
            "change decision conflicts with the architecture choice",
        ));
    }
    validate_architecture_decision(&change.architecture)?;
    validate_planned_deletions(change)
}

fn validate_planned_deletions(change: &SkillChangeBundle) -> Result<(), RuntimeError> {
    let planned = change
        .architecture
        .deletions
        .iter()
        .map(NonEmptyString::as_str)
        .collect::<BTreeSet<_>>();
    let bundled = change
        .deletes
        .iter()
        .map(NonEmptyString::as_str)
        .collect::<BTreeSet<_>>();
    if planned == bundled {
        return Ok(());
    }
    Err(invalid_skill_change(
        "bundle deletes must exactly match architecture deletions",
    ))
}

pub(super) fn validate_architecture_decision(
    architecture: &SkillArchitectureDecision,
) -> Result<(), RuntimeError> {
    validate_architecture_completeness(architecture)?;
    validate_operator_contract(architecture)?;
    let selected = selected_capabilities(architecture)?;
    validate_behavior_lanes(architecture, &selected)?;
    validate_provider_boundary(architecture)
}

fn validate_operator_contract(
    architecture: &SkillArchitectureDecision,
) -> Result<(), RuntimeError> {
    let implements_package = matches!(
        architecture.choice,
        SkillArchitectureChoice::Build | SkillArchitectureChoice::ExtendExisting
    );
    if !implements_package {
        if architecture.identity.is_some()
            || architecture.direct_use.is_some()
            || architecture.chain_use.is_some()
        {
            return Err(invalid_skill_change(
                "no_skill and needs_core architectures must not invent a package identity or operator contract",
            ));
        }
        return Ok(());
    }

    let identity = architecture.identity.as_ref().ok_or_else(|| {
        invalid_skill_change("build and extension architectures require an identity decision")
    })?;
    validate_identity_decision(architecture.choice, identity)?;

    match identity.visibility {
        SkillPackageVisibility::Public => {
            let direct = architecture.direct_use.as_ref().ok_or_else(|| {
                invalid_skill_change("public architectures require a direct_use contract")
            })?;
            let chain = architecture.chain_use.as_ref().ok_or_else(|| {
                invalid_skill_change("public architectures require a chain_use contract")
            })?;
            if direct.trigger_requests.is_empty() || direct.non_trigger_requests.is_empty() {
                return Err(invalid_skill_change(
                    "public direct_use requires at least one trigger request and one non-trigger request",
                ));
            }
            if chain.accepted_inputs.is_empty() || chain.must_not_repeat.is_empty() {
                return Err(invalid_skill_change(
                    "public chain_use requires accepted inputs and work that must not repeat",
                ));
            }
            validate_operator_proofs(architecture)?;
        }
        SkillPackageVisibility::Internal => {
            if architecture.direct_use.is_some() {
                return Err(invalid_skill_change(
                    "internal architectures must not declare a public direct_use trigger contract",
                ));
            }
            if identity.action == SkillIdentityAction::Internalize
                && architecture.chain_use.is_none()
            {
                return Err(invalid_skill_change(
                    "internalized architectures require a chain_use contract for their runtime owner",
                ));
            }
        }
    }
    Ok(())
}

fn validate_identity_decision(
    choice: SkillArchitectureChoice,
    identity: &runx_contracts::SkillIdentityDecision,
) -> Result<(), RuntimeError> {
    let current = identity.current_name.as_ref().map(NonEmptyString::as_str);
    let proposed = identity.proposed_name.as_ref().map(NonEmptyString::as_str);
    match (choice, identity.action, current, proposed) {
        (SkillArchitectureChoice::Build, SkillIdentityAction::Create, None, Some(_)) => {}
        (
            SkillArchitectureChoice::ExtendExisting,
            SkillIdentityAction::Keep | SkillIdentityAction::Internalize,
            Some(current),
            Some(proposed),
        ) if current == proposed => {}
        (
            SkillArchitectureChoice::ExtendExisting,
            SkillIdentityAction::Rename,
            Some(current),
            Some(proposed),
        ) if current != proposed => {}
        _ => {
            return Err(invalid_skill_change(
                "identity action, current_name, and proposed_name conflict with the architecture choice",
            ));
        }
    }
    if identity.action == SkillIdentityAction::Internalize
        && identity.visibility != SkillPackageVisibility::Internal
    {
        return Err(invalid_skill_change(
            "an internalize identity decision must select internal visibility",
        ));
    }
    Ok(())
}

fn validate_operator_proofs(architecture: &SkillArchitectureDecision) -> Result<(), RuntimeError> {
    let kinds = architecture
        .proof_plan
        .iter()
        .map(|proof| proof.kind)
        .collect::<BTreeSet<_>>();
    let required = [
        SkillProofKind::SelectionTrial,
        SkillProofKind::StandaloneOperatorJourney,
        SkillProofKind::ComposedOperatorJourney,
    ];
    if required.iter().all(|kind| kinds.contains(kind)) {
        return Ok(());
    }
    Err(invalid_skill_change(
        "public architectures require selection_trial, standalone_operator_journey, and composed_operator_journey proofs",
    ))
}

fn validate_architecture_completeness(
    architecture: &SkillArchitectureDecision,
) -> Result<(), RuntimeError> {
    if matches!(
        architecture.choice,
        SkillArchitectureChoice::Build | SkillArchitectureChoice::ExtendExisting
    ) && (architecture.required_behaviors.is_empty() || architecture.proof_plan.is_empty())
    {
        return Err(invalid_skill_change(
            "build and extension architectures require behaviors and a proof plan",
        ));
    }
    if architecture.choice == SkillArchitectureChoice::NeedsCore
        && architecture.native_reuse.missing_capabilities.is_empty()
    {
        return Err(invalid_skill_change(
            "needs_core architecture must identify at least one missing native capability",
        ));
    }
    if !architecture.native_reuse.missing_capabilities.is_empty()
        && architecture.choice != SkillArchitectureChoice::NeedsCore
    {
        return Err(invalid_skill_change(
            "missing native capabilities require a needs_core architecture decision",
        ));
    }
    Ok(())
}

fn selected_capabilities(
    architecture: &SkillArchitectureDecision,
) -> Result<BTreeSet<&str>, RuntimeError> {
    let selected = architecture
        .native_reuse
        .selected_capabilities
        .iter()
        .map(NonEmptyString::as_str)
        .collect::<BTreeSet<_>>();
    let inspected = architecture
        .native_reuse
        .inspected_capabilities
        .iter()
        .map(NonEmptyString::as_str)
        .collect::<BTreeSet<_>>();
    if !selected.is_subset(&inspected) {
        return Err(invalid_skill_change(
            "selected native capabilities must be present in inspected_capabilities",
        ));
    }
    Ok(selected)
}

fn validate_behavior_lanes(
    architecture: &SkillArchitectureDecision,
    selected: &BTreeSet<&str>,
) -> Result<(), RuntimeError> {
    for behavior in &architecture.required_behaviors {
        match behavior.lane {
            SkillExecutionLane::DomainModule if behavior.domain_module_justification.is_none() => {
                return Err(invalid_skill_change(format!(
                    "domain-module behavior '{}' requires a justification",
                    behavior.id.as_str()
                )));
            }
            SkillExecutionLane::NativeCapability => {
                let reuse = behavior.reuses.as_ref().ok_or_else(|| {
                    invalid_skill_change(format!(
                        "native behavior '{}' requires reuses",
                        behavior.id.as_str()
                    ))
                })?;
                if !selected.contains(reuse.as_str()) {
                    return Err(invalid_skill_change(format!(
                        "native behavior '{}' must reference a selected capability",
                        behavior.id.as_str()
                    )));
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn validate_provider_boundary(
    architecture: &SkillArchitectureDecision,
) -> Result<(), RuntimeError> {
    let provider_lane = architecture
        .required_behaviors
        .iter()
        .any(|behavior| behavior.lane == SkillExecutionLane::Operation);
    let provider_effect = architecture
        .effects
        .iter()
        .any(|effect| effect.provider_boundary);
    if provider_lane == provider_effect {
        return Ok(());
    }
    Err(invalid_skill_change(
        "provider-adapter behavior and provider-boundary effect evidence must agree",
    ))
}

pub(super) fn validate_resource_budget(
    architecture: &SkillArchitectureDecision,
    metrics: &SkillPackageMetrics,
) -> Result<(), RuntimeError> {
    if metrics.files > architecture.resource_budget.max_files {
        return Err(invalid_skill_change(format!(
            "candidate has {} files, exceeding architecture budget {}",
            metrics.files, architecture.resource_budget.max_files
        )));
    }
    if metrics.executable_lines > architecture.resource_budget.max_executable_lines {
        return Err(invalid_skill_change(format!(
            "candidate has {} executable lines, exceeding architecture budget {}",
            metrics.executable_lines, architecture.resource_budget.max_executable_lines
        )));
    }
    Ok(())
}

pub(super) fn validate_digest(field: &str, value: &str) -> Result<(), RuntimeError> {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return Err(invalid_skill_change(format!(
            "{field} must be a sha256-prefixed digest"
        )));
    };
    if hex.len() != 64 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(invalid_skill_change(format!(
            "{field} must contain exactly 64 hexadecimal characters"
        )));
    }
    Ok(())
}

const fn decision_name(decision: SkillChangeDecision) -> &'static str {
    match decision {
        SkillChangeDecision::Write => "write",
        SkillChangeDecision::NoSkill => "no_skill",
        SkillChangeDecision::NoChange => "no_change",
        SkillChangeDecision::NeedsCore => "needs_core",
    }
}
