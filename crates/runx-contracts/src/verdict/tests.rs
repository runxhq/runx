//! verdict.md's validation rows and plan §9's verdict row: a verdict is
//! checked against its bound criterion set, leaving out a failed required
//! check gives `inconclusive`, never `met`, a duplicate or unknown criterion
//! is refused, and any failed required check gives `not_met`.

use serde_json::json;

use super::{
    Criteria, CriterionId, Decided, Pinned, Verdict, VerdictAnswer, VerdictCriterion,
    VerdictRefused,
};
use crate::schema::NonEmptyVec;
use crate::schema::RunxSchema;

const CRITERIA_REF: &str = "runx/github.coverage";

fn criterion(id: &str, required: bool) -> Result<VerdictCriterion, String> {
    serde_json::from_value(json!({
        "id": id,
        "required": required,
        "question": format!("Is {id} so?"),
    }))
    .map_err(|error| error.to_string())
}

/// `every_repository_read` and `window_moved` required, `quiet` not.
fn criteria() -> Result<Criteria, String> {
    let set = NonEmptyVec::new(vec![
        criterion("every_repository_read", true)?,
        criterion("window_moved", true)?,
        criterion("quiet", false)?,
    ])
    .ok_or("a criteria set holds a criterion")?;
    Criteria::new(set).map_err(|error| error.to_string())
}

fn subject() -> Result<Pinned, String> {
    serde_json::from_value(json!({"ref": "window:runxhq", "revision": "sha256:window"}))
        .map_err(|error| error.to_string())
}

fn failure() -> serde_json::Value {
    json!({
        "expected": "position at 2026-11-07T05:00Z",
        "actual": "position at 2026-11-07T07:00Z",
        "where": "runx/github/positions#runxhq/runx",
    })
}

/// A verdict stating `stated` with `assessments`, against [`criteria`].
fn verdict(stated: &str, assessments: serde_json::Value) -> Result<Verdict, String> {
    let criteria = criteria()?;
    serde_json::from_value(json!({
        "schema": "runx.verdict.v1",
        "verdict": stated,
        "subject": {"ref": "window:runxhq", "revision": "sha256:window"},
        "criteria_set": {"ref": CRITERIA_REF, "revision": criteria.revision.as_str()},
        "assessments": assessments,
        "evaluator": {"package": "runx/github", "method": "follow", "run": "run_1"},
    }))
    .map_err(|error| error.to_string())
}

fn decide(verdict: &Verdict) -> Result<Result<Decided, VerdictRefused>, String> {
    Ok(verdict.decide(&subject()?, CRITERIA_REF, &criteria()?))
}

fn id(value: &str) -> Result<CriterionId, String> {
    CriterionId::new(value).ok_or_else(|| format!("{value} is a criterion id"))
}

#[test]
fn every_required_criterion_met_is_met() -> Result<(), String> {
    let met = verdict(
        "met",
        json!([
            {"id": "every_repository_read", "verdict": "met", "evidence": ["call:1"]},
            {"id": "window_moved", "verdict": "met", "evidence": ["call:2"]},
        ]),
    )?;
    assert_eq!(
        decide(&met)?,
        Ok(Decided {
            verdict: VerdictAnswer::Met,
            unresolved: Vec::new(),
        })
    );
    Ok(())
}

#[test]
fn a_failed_assessment_with_identical_expected_and_actual_is_refused() -> Result<(), String> {
    let contradictory = verdict(
        "not_met",
        json!([{
            "id": "every_repository_read", "verdict": "not_met", "evidence": ["call:1"],
            "failure": {"expected": "No provider was queried.", "actual": "No provider was queried.", "where": "call:1"}
        }]),
    )?;
    assert_eq!(
        decide(&contradictory)?,
        Err(VerdictRefused::ContradictoryFailure(id(
            "every_repository_read"
        )?))
    );
    let exact_text = verdict(
        "not_met",
        json!([{
            "id": "every_repository_read", "verdict": "not_met", "evidence": ["call:1"],
            "failure": {"expected": "token ", "actual": "token", "where": "call:1"}
        }]),
    )?;
    assert_eq!(
        decide(&exact_text)?.map(|decision| decision.verdict),
        Ok(VerdictAnswer::NotMet)
    );
    Ok(())
}

#[test]
fn leaving_out_a_failed_required_check_is_inconclusive_never_met() -> Result<(), String> {
    // The evaluator failed window_moved, left it out, and said met.
    let claimed = verdict(
        "met",
        json!([{"id": "every_repository_read", "verdict": "met", "evidence": ["call:1"]}]),
    )?;
    assert_eq!(
        decide(&claimed)?,
        Ok(Decided {
            verdict: VerdictAnswer::Inconclusive,
            unresolved: vec![id("window_moved")?],
        })
    );
    // Answering nothing at all is no better.
    let silent = verdict("met", json!([]))?;
    assert_eq!(
        decide(&silent)?.map(|decided| decided.verdict),
        Ok(VerdictAnswer::Inconclusive)
    );
    Ok(())
}

#[test]
fn any_failed_required_check_is_not_met() -> Result<(), String> {
    let failed = verdict(
        "met",
        json!([
            {"id": "every_repository_read", "verdict": "met", "evidence": ["call:1"]},
            {"id": "window_moved", "verdict": "not_met", "evidence": ["call:2"], "failure": failure()},
        ]),
    )?;
    assert_eq!(
        decide(&failed)?.map(|decided| decided.verdict),
        Ok(VerdictAnswer::NotMet)
    );
    // A failure decides it even with another required check left out, and
    // an inconclusive answer beside it doesn't soften it.
    let failed_and_missing = verdict(
        "inconclusive",
        json!([{"id": "window_moved", "verdict": "not_met", "evidence": [], "failure": failure()}]),
    )?;
    assert_eq!(
        decide(&failed_and_missing)?,
        Ok(Decided {
            verdict: VerdictAnswer::NotMet,
            unresolved: vec![id("every_repository_read")?],
        })
    );
    Ok(())
}

#[test]
fn an_inconclusive_required_answer_is_inconclusive() -> Result<(), String> {
    let unsure = verdict(
        "met",
        json!([
            {"id": "every_repository_read", "verdict": "met", "evidence": ["call:1"]},
            {"id": "window_moved", "verdict": "inconclusive", "evidence": []},
        ]),
    )?;
    assert_eq!(
        decide(&unsure)?,
        Ok(Decided {
            verdict: VerdictAnswer::Inconclusive,
            unresolved: Vec::new(),
        })
    );
    Ok(())
}

#[test]
fn a_criterion_that_is_not_required_decides_nothing() -> Result<(), String> {
    let noisy = verdict(
        "not_met",
        json!([
            {"id": "every_repository_read", "verdict": "met", "evidence": ["call:1"]},
            {"id": "window_moved", "verdict": "met", "evidence": ["call:2"]},
            {"id": "quiet", "verdict": "not_met", "evidence": ["call:3"], "failure": failure()},
        ]),
    )?;
    assert_eq!(
        decide(&noisy)?.map(|decided| decided.verdict),
        Ok(VerdictAnswer::Met)
    );
    // Every answer stays visible, the one that decided nothing included.
    assert_eq!(noisy.assessments.len(), 3);
    Ok(())
}

#[test]
fn a_duplicate_or_unknown_criterion_is_refused() -> Result<(), String> {
    let twice = verdict(
        "met",
        json!([
            {"id": "every_repository_read", "verdict": "met", "evidence": []},
            {"id": "window_moved", "verdict": "met", "evidence": []},
            {"id": "window_moved", "verdict": "not_met", "evidence": [], "failure": failure()},
        ]),
    )?;
    assert_eq!(
        decide(&twice)?,
        Err(VerdictRefused::DuplicateAnswer(id("window_moved")?))
    );
    let unknown = verdict(
        "met",
        json!([
            {"id": "every_repository_read", "verdict": "met", "evidence": []},
            {"id": "window_moved", "verdict": "met", "evidence": []},
            {"id": "looked_fine", "verdict": "met", "evidence": []},
        ]),
    )?;
    assert_eq!(
        decide(&unknown)?,
        Err(VerdictRefused::UnknownCriterion(id("looked_fine")?))
    );
    Ok(())
}

#[test]
fn a_criteria_set_naming_an_id_twice_is_refused() -> Result<(), String> {
    let set = NonEmptyVec::new(vec![
        criterion("every_repository_read", true)?,
        criterion("every_repository_read", false)?,
    ])
    .ok_or("a criteria set holds a criterion")?;
    let repeated = Criteria::new(set).map_err(|error| error.to_string())?;
    assert_eq!(
        repeated.check(),
        Err(VerdictRefused::RepeatedCriterion(id(
            "every_repository_read"
        )?))
    );
    Ok(())
}

#[test]
fn a_verdict_counts_only_for_its_exact_subject_and_criteria_revisions() -> Result<(), String> {
    let met = verdict(
        "met",
        json!([
            {"id": "every_repository_read", "verdict": "met", "evidence": ["call:1"]},
            {"id": "window_moved", "verdict": "met", "evidence": ["call:2"]},
        ]),
    )?;
    let later: Pinned =
        serde_json::from_value(json!({"ref": "window:runxhq", "revision": "sha256:later"}))
            .map_err(|error| error.to_string())?;
    assert!(matches!(
        met.decide(&later, CRITERIA_REF, &criteria()?),
        Err(VerdictRefused::Subject { .. })
    ));
    assert!(matches!(
        met.decide(&subject()?, "runx/github.other", &criteria()?),
        Err(VerdictRefused::CriteriaSet { .. })
    ));
    // A set with one more check is another revision, so the verdict
    // doesn't answer it.
    let widened = Criteria::new(
        NonEmptyVec::new(vec![
            criterion("every_repository_read", true)?,
            criterion("window_moved", true)?,
            criterion("quiet", false)?,
            criterion("no_open_decision", true)?,
        ])
        .ok_or("a criteria set holds a criterion")?,
    )
    .map_err(|error| error.to_string())?;
    assert_ne!(widened.revision, criteria()?.revision);
    assert!(matches!(
        met.decide(&subject()?, CRITERIA_REF, &widened),
        Err(VerdictRefused::CriteriaSet { .. })
    ));
    Ok(())
}

#[test]
fn a_criteria_set_whose_revision_is_not_its_digest_is_refused() -> Result<(), String> {
    let mut changed = criteria()?;
    changed.criteria = NonEmptyVec::new(vec![criterion("every_repository_read", true)?])
        .ok_or("a criteria set holds a criterion")?;
    assert!(matches!(
        changed.check(),
        Err(VerdictRefused::CriteriaRevision { .. })
    ));
    // A verdict bound to the old revision can't be read against it either.
    let met = verdict(
        "met",
        json!([{"id": "every_repository_read", "verdict": "met", "evidence": []}]),
    )?;
    assert!(matches!(
        met.decide(&subject()?, CRITERIA_REF, &changed),
        Err(VerdictRefused::CriteriaRevision { .. })
    ));
    Ok(())
}

#[test]
fn a_malformed_verdict_does_not_parse() -> Result<(), String> {
    let base = |assessment: serde_json::Value| {
        json!({
            "schema": "runx.verdict.v1",
            "verdict": "met",
            "subject": {"ref": "window:runxhq", "revision": "sha256:window"},
            "criteria_set": {"ref": CRITERIA_REF, "revision": "sha256:set"},
            "assessments": [assessment],
            "evaluator": {"package": "runx/github", "method": "follow", "run": "run_1"},
        })
    };
    for malformed in [
        // not_met says what failed.
        base(json!({"id": "window_moved", "verdict": "not_met", "evidence": []})),
        // met has nothing to say.
        base(json!({"id": "window_moved", "verdict": "met", "evidence": [], "failure": failure()})),
        // An answer is one of three.
        base(json!({"id": "window_moved", "verdict": "probably", "evidence": []})),
        // A criterion id is snake_case.
        base(json!({"id": "Window-Moved", "verdict": "met", "evidence": []})),
        // Evidence is a list of references.
        base(json!({"id": "window_moved", "verdict": "met"})),
        // An answer holds nothing else.
        base(json!({"id": "window_moved", "verdict": "met", "evidence": [], "note": "fine"})),
    ] {
        assert!(
            serde_json::from_value::<Verdict>(malformed.clone()).is_err(),
            "{malformed}"
        );
    }
    let mut no_evaluator = base(json!({"id": "window_moved", "verdict": "met", "evidence": []}));
    no_evaluator
        .as_object_mut()
        .and_then(|verdict| verdict.remove("evaluator"))
        .ok_or("the base names an evaluator")?;
    assert!(serde_json::from_value::<Verdict>(no_evaluator).is_err());
    Ok(())
}

#[test]
fn the_schemas_accept_and_refuse_what_the_types_do() -> Result<(), String> {
    let verdict_schema = jsonschema::draft202012::options()
        .build(&Verdict::json_schema())
        .map_err(|error| error.to_string())?;
    let criteria_schema = jsonschema::draft202012::options()
        .build(&Criteria::json_schema())
        .map_err(|error| error.to_string())?;
    let met = verdict(
        "not_met",
        json!([
            {"id": "every_repository_read", "verdict": "met", "evidence": ["call:1"]},
            {"id": "window_moved", "verdict": "not_met", "evidence": ["call:2"], "failure": failure()},
            {"id": "quiet", "verdict": "inconclusive", "evidence": []},
        ]),
    )?;
    let written = serde_json::to_value(&met).map_err(|error| error.to_string())?;
    assert!(verdict_schema.is_valid(&written), "{written}");
    let set = serde_json::to_value(criteria()?).map_err(|error| error.to_string())?;
    assert!(criteria_schema.is_valid(&set), "{set}");
    // What the types refuse, the schemas refuse.
    let mut no_failure = written.clone();
    no_failure["assessments"][1]
        .as_object_mut()
        .and_then(|assessment| assessment.remove("failure"))
        .ok_or("the not_met answer has a failure")?;
    assert!(!verdict_schema.is_valid(&no_failure));
    let mut failed_met = written;
    failed_met["assessments"][0]["failure"] = failure();
    assert!(!verdict_schema.is_valid(&failed_met));
    let mut empty_set = set;
    empty_set["criteria"] = json!([]);
    assert!(!criteria_schema.is_valid(&empty_set));
    Ok(())
}
