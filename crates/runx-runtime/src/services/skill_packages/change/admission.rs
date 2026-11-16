//! The checks on an authored change's stated intent. The files it writes are
//! the package's contract, and validation checks the package they make.

use runx_contracts::{SkillAuthoringMode, SkillChangeBundle, SkillChangeDecision};

use super::{RuntimeError, invalid_skill_change};

/// `change` as `mode` applies it to a package whose state is now
/// `current_digest`: its decision fits the mode, its files fit its decision,
/// and a decision to write nothing is about the package as it stands.
pub(super) fn validate_change_contract(
    change: &SkillChangeBundle,
    mode: SkillAuthoringMode,
    current_digest: &str,
) -> Result<(), RuntimeError> {
    validate_digest("base_digest", change.base_digest.as_str())?;
    validate_mode_decision(mode, change.decision)?;
    validate_change_shape(change)?;
    if change.decision != SkillChangeDecision::Write
        && change.base_digest.as_str() != current_digest
    {
        return Err(invalid_skill_change(
            "the package changed since it was inspected, so a decision to write nothing is about another package; inspect and author again",
        ));
    }
    Ok(())
}

/// A decision `mode` takes: a design takes any, since it writes nothing; a
/// build writes a new package or finds the work isn't a skill's or needs
/// core; an improve changes a package, or needs none or core; a test adds
/// cases or needs none.
pub(super) fn validate_mode_decision(
    mode: SkillAuthoringMode,
    decision: SkillChangeDecision,
) -> Result<(), RuntimeError> {
    let allowed = match mode {
        SkillAuthoringMode::Design => true,
        SkillAuthoringMode::Build => matches!(
            decision,
            SkillChangeDecision::Write
                | SkillChangeDecision::NoSkill
                | SkillChangeDecision::NeedsCore
        ),
        SkillAuthoringMode::Improve => matches!(
            decision,
            SkillChangeDecision::Write
                | SkillChangeDecision::NoChange
                | SkillChangeDecision::NeedsCore
        ),
        SkillAuthoringMode::Test => matches!(
            decision,
            SkillChangeDecision::Write | SkillChangeDecision::NoChange
        ),
    };
    if allowed {
        return Ok(());
    }
    Err(invalid_skill_change(format!(
        "{} is not a decision {} takes",
        decision_name(decision),
        mode.as_str()
    )))
}

/// Where `mode` writes: a build creates a package where none stands, and an
/// improve or a test changes the one that does.
pub(super) fn validate_mode_target(
    mode: SkillAuthoringMode,
    decision: SkillChangeDecision,
    target_exists: bool,
) -> Result<(), RuntimeError> {
    if decision != SkillChangeDecision::Write {
        return Ok(());
    }
    match (mode, target_exists) {
        (SkillAuthoringMode::Build, true) => Err(invalid_skill_change(
            "build creates a package where none stands, and one stands at the target: improve it, or build at another target",
        )),
        (SkillAuthoringMode::Improve | SkillAuthoringMode::Test, false) => {
            Err(invalid_skill_change(format!(
                "{} changes a package that stands, and none stands at the target: build it",
                mode.as_str()
            )))
        }
        _ => Ok(()),
    }
}

/// A `write` carries a write or a delete, and no other decision carries
/// either.
pub(super) fn validate_change_shape(change: &SkillChangeBundle) -> Result<(), RuntimeError> {
    if change.decision != SkillChangeDecision::Write
        && (!change.writes.is_empty() || !change.deletes.is_empty())
    {
        return Err(invalid_skill_change(format!(
            "{} writes nothing, so it carries no writes or deletes",
            decision_name(change.decision)
        )));
    }
    if change.decision == SkillChangeDecision::Write
        && change.writes.is_empty()
        && change.deletes.is_empty()
    {
        return Err(invalid_skill_change(
            "a write decision carries at least one write or delete",
        ));
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
