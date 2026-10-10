//! The act a governed call performs, as its receipt seals it.
//!
//! An act is OPENED with its intent and form, and CLOSED with the execution
//! outcome. Only a governed call seals one: the runtime testifies to what it
//! observed (its seal), and never states an intent nobody gave it. The act's
//! purpose, reason and target are its caller's; when the caller states none,
//! its purpose is the operation's declared description. Its legitimacy is the
//! execution term admission derived for the call.

use runx_contracts::schema::NonEmptyString;
use runx_contracts::{
    ActForm, CallAct, Closure, ClosureDisposition, CriterionBinding, CriterionStatus,
    DecisionChoice, Intent, ReceiptAct, Reference, ReferenceType, SuccessCriterion,
    authority_term_legitimacy,
};

/// What a governed call's receipt records beyond its step: the act its caller
/// stated, the operation's declared description, the authority it was
/// admitted under, what it produced, and the context the run was prepared
/// with.
pub(crate) struct SealedCall {
    pub(crate) act: Option<CallAct>,
    /// The operation's declared description: the call's purpose when its
    /// caller states none.
    pub(crate) description: NonEmptyString,
    /// The execution term admission derived for the call.
    pub(crate) term_id: String,
    /// Where the act's target lives, classified by the declared references.
    pub(crate) target_ref: Option<Reference>,
    /// What the call produced, as its operation declares it.
    pub(crate) effects: Vec<Reference>,
    /// The operator context the run was prepared with.
    pub(crate) context_ref: Option<Reference>,
}

impl SealedCall {
    /// The receipt this act follows, when its caller names one.
    pub(crate) fn previous(&self) -> Option<Reference> {
        let previous = self.act.as_ref()?.previous.as_ref()?;
        Some(Reference::runx(ReferenceType::Receipt, previous.as_str()))
    }

    /// The decision the call carries out, when its caller states one.
    pub(crate) fn decision(&self) -> Option<DecisionChoice> {
        self.act.as_ref()?.decision.map(DecisionChoice::from)
    }
}

/// The execution outcome that closes an act into its sealed body.
pub(crate) struct ActOutcome<'a> {
    /// How the act closed (`Closed`, `Failed`, ...).
    pub(crate) disposition: ClosureDisposition,
    /// Whether the call succeeded, driving the criterion status.
    pub(crate) succeeded: bool,
    /// The outcome the runtime observed, shared by the criterion and closure.
    pub(crate) summary: String,
    /// When the act closed.
    pub(crate) performed_at: &'a str,
    /// The evidence of the outcome and its verification.
    pub(crate) evidence_refs: &'a [Reference],
    pub(crate) verification_refs: &'a [Reference],
}

/// The criterion every call's outcome is judged by.
const OUTCOME_CRITERION: &str = "step_outcome";

/// A governed call's act, opened with its intent and form.
pub(crate) struct RuntimeAct {
    id: NonEmptyString,
    form: ActForm,
    intent: Intent,
    summary: NonEmptyString,
    verified_bindings: Vec<CriterionBinding>,
    target_refs: Vec<Reference>,
    produced: Vec<Reference>,
    context_ref: Option<Reference>,
}

impl RuntimeAct {
    /// The act a governed call performs: the one its caller states, or an
    /// observation whose purpose is its operation's declared description.
    pub(crate) fn for_call(step_id: &str, call: &SealedCall) -> Self {
        let stated = call.act.as_ref();
        let purpose =
            stated.map_or_else(|| call.description.clone(), |stated| stated.purpose.clone());
        Self {
            id: format!("act_{step_id}").into(),
            form: stated.map_or(ActForm::Observation, |stated| stated.form.into()),
            intent: Intent {
                purpose: purpose.clone(),
                legitimacy: authority_term_legitimacy(&call.term_id).into(),
                success_criteria: vec![SuccessCriterion {
                    criterion_id: OUTCOME_CRITERION.into(),
                    statement: "runtime step completes successfully".into(),
                    required: true,
                }],
                constraints: Vec::new(),
                derived_from: Vec::new(),
            },
            summary: stated.map_or(purpose, |stated| stated.summary().clone()),
            verified_bindings: Vec::new(),
            target_refs: call.target_ref.iter().cloned().collect(),
            produced: call.effects.clone(),
            context_ref: call.context_ref.clone(),
        }
    }

    /// Bind native verification evidence into the call's act. Criteria are
    /// generic contract proofs; the receipt layer needs no provider- or
    /// skill-specific knowledge.
    pub(crate) fn with_verified_criteria(
        mut self,
        criteria: Vec<SuccessCriterion>,
        bindings: Vec<CriterionBinding>,
    ) -> Self {
        self.intent.success_criteria.extend(criteria);
        self.verified_bindings = bindings;
        self
    }

    /// Close the act with its execution outcome, producing the sealed act body.
    pub(crate) fn close(self, outcome: ActOutcome<'_>) -> ReceiptAct {
        let artifact_refs = if outcome.succeeded {
            self.produced
        } else {
            Vec::new()
        };
        let mut criterion_bindings = vec![CriterionBinding {
            criterion_id: OUTCOME_CRITERION.into(),
            status: if outcome.succeeded {
                CriterionStatus::Verified
            } else {
                CriterionStatus::Failed
            },
            evidence_refs: outcome.evidence_refs.to_vec(),
            verification_refs: outcome.verification_refs.to_vec(),
            summary: Some(outcome.summary.clone().into()),
        }];
        criterion_bindings.extend(self.verified_bindings);
        ReceiptAct {
            id: self.id,
            form: self.form,
            intent: self.intent,
            summary: self.summary,
            criterion_bindings,
            by: None,
            source_refs: Vec::new(),
            target_refs: self.target_refs,
            artifact_refs,
            context_ref: self.context_ref,
            closure: Closure {
                disposition: outcome.disposition,
                reason_code: OUTCOME_CRITERION.into(),
                summary: outcome.summary.into(),
                closed_at: outcome.performed_at.into(),
            },
            revision: None,
            verification: None,
        }
    }
}
