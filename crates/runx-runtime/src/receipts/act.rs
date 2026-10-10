//! The act as a first-class object.
//!
//! An act is OPENED with its declared intent and form, and CLOSED with the
//! execution outcome. Separating the intent (fixed up front) from the outcome
//! (criterion status, refs, closure, filled from the result) is what makes the
//! act a unit of its own rather than a value reconstructed at seal time.
//!
//! A step opens a generic observation act. A governed call whose caller
//! states an act opens that act instead: its form, purpose, reason and target
//! are the caller's, and its legitimacy is the execution term admission
//! derived for the call.

use runx_contracts::schema::NonEmptyString;
use runx_contracts::{
    ActForm, CallAct, Closure, ClosureDisposition, CriterionBinding, CriterionStatus,
    DecisionChoice, Intent, ReceiptAct, Reference, ReferenceType, SuccessCriterion,
};

use crate::execution::output_projection::StepOutputRefs;

/// What a governed call's receipt records beyond its step: the act its caller
/// stated, the authority it was admitted under, and what it produced.
pub(crate) struct SealedCall {
    pub(crate) act: Option<CallAct>,
    /// The execution term admission derived for the call.
    pub(crate) term_id: String,
    /// Where the act's target lives, classified by the declared references.
    pub(crate) target_ref: Option<Reference>,
    /// What the call produced, as its operation declares it.
    pub(crate) effects: Vec<Reference>,
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
    /// How the act closed (`Closed`, `Deferred`, ...). May differ from
    /// `succeeded`: an agent act can close `Deferred` on a successful turn.
    pub(crate) disposition: ClosureDisposition,
    /// Whether the underlying invocation succeeded, driving the criterion status.
    pub(crate) succeeded: bool,
    /// Human summary of the outcome, shared by the criterion and the closure.
    pub(crate) summary: String,
    /// When the act closed.
    pub(crate) performed_at: &'a str,
    /// References projected from the execution output.
    pub(crate) refs: &'a StepOutputRefs,
}

/// An act opened with its intent and form fixed up front.
pub(crate) struct RuntimeAct {
    id: NonEmptyString,
    form: ActForm,
    intent: Intent,
    summary: NonEmptyString,
    criterion_id: NonEmptyString,
    reason_code: NonEmptyString,
    verified_bindings: Vec<CriterionBinding>,
    target_refs: Vec<Reference>,
    produced: Vec<Reference>,
    context_ref: Option<Reference>,
}

impl RuntimeAct {
    /// The act a step opens: the act its call's caller stated, or the generic
    /// observation, "this step ran, admitted by the local harness".
    pub(crate) fn open(step_id: &str, call: Option<&SealedCall>) -> Self {
        let act = Self::observation(step_id);
        let Some(call) = call else {
            return act;
        };
        let act = Self {
            produced: call.effects.clone(),
            ..act
        };
        let Some(stated) = &call.act else {
            return act;
        };
        Self {
            form: stated.form.into(),
            intent: Intent {
                purpose: stated.purpose.clone(),
                legitimacy: format!("runx:authority_term:{}", call.term_id).into(),
                ..act.intent
            },
            summary: stated.summary().clone(),
            target_refs: call.target_ref.iter().cloned().collect(),
            ..act
        }
    }

    fn observation(step_id: &str) -> Self {
        Self {
            id: format!("act_{step_id}").into(),
            form: ActForm::Observation,
            intent: Intent {
                purpose: format!("Run graph step {step_id}").into(),
                legitimacy: "Runtime graph execution was admitted by the local harness".into(),
                success_criteria: vec![SuccessCriterion {
                    criterion_id: "step_outcome".into(),
                    statement: "runtime step completes successfully".into(),
                    required: true,
                }],
                constraints: Vec::new(),
                derived_from: Vec::new(),
            },
            summary: format!("Executed graph step {step_id}").into(),
            criterion_id: "step_outcome".into(),
            reason_code: "step_outcome".into(),
            verified_bindings: Vec::new(),
            target_refs: Vec::new(),
            produced: Vec::new(),
            context_ref: None,
        }
    }

    /// Bind native verification evidence into the act opened for this step.
    /// Criteria are generic contract proofs; the runtime receipt layer does not
    /// need provider- or skill-specific knowledge.
    pub(crate) fn with_verified_criteria(
        mut self,
        criteria: Vec<SuccessCriterion>,
        bindings: Vec<CriterionBinding>,
    ) -> Self {
        self.intent.success_criteria.extend(criteria);
        self.verified_bindings = bindings;
        self
    }

    /// The operator context the run was prepared with.
    pub(crate) fn with_context(mut self, context_ref: Option<Reference>) -> Self {
        self.context_ref = context_ref;
        self
    }

    /// Close the act with its execution outcome, producing the sealed act body.
    pub(crate) fn close(self, outcome: ActOutcome<'_>) -> ReceiptAct {
        let mut artifact_refs = outcome.refs.artifact_refs.clone();
        artifact_refs.extend(outcome.refs.surface_refs.iter().cloned());
        if outcome.succeeded {
            artifact_refs.extend(self.produced);
        }
        let mut criterion_bindings = vec![CriterionBinding {
            criterion_id: self.criterion_id,
            status: if outcome.succeeded {
                CriterionStatus::Verified
            } else {
                CriterionStatus::Failed
            },
            evidence_refs: outcome.refs.evidence_refs.clone(),
            verification_refs: outcome.refs.verification_refs.clone(),
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
            source_refs: outcome.refs.source_refs.clone(),
            target_refs: self.target_refs,
            artifact_refs,
            context_ref: self.context_ref,
            closure: Closure {
                disposition: outcome.disposition,
                reason_code: self.reason_code,
                summary: outcome.summary.into(),
                closed_at: outcome.performed_at.into(),
            },
            revision: None,
            verification: None,
        }
    }
}
