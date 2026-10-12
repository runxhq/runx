//! The act a governed call performs, as its receipt seals it.
//!
//! An act is OPENED with its intent and form, and CLOSED with the checks the
//! runtime bound to it; its outcome is the receipt's seal. Only a governed
//! call seals one: the runtime testifies to what it observed, and never
//! states an intent or a reason nobody gave it. The act's purpose, reason and
//! target are its caller's; when the caller states none, its purpose is the
//! operation's declared description and it carries no reason. Its legitimacy
//! is the execution term admission derived for the call.

use runx_contracts::schema::NonEmptyString;
use runx_contracts::{
    ActForm, CallAct, Criterion, CriterionBinding, CriterionStatus, DecisionChoice, Intent,
    ReceiptAct, Reference, ReferenceType, TermLegitimacy,
};

/// What a governed call's receipt records beyond its step: the act its caller
/// stated, the operation's declared description, the authority it was
/// admitted under, what it produced, and the context the run was prepared
/// with.
pub(crate) struct SealedCall {
    pub(crate) operation_intent: Option<runx_contracts::OperationIntent>,
    pub(crate) act: Option<CallAct>,
    /// The operation's declared description: the call's purpose when its
    /// caller states none.
    pub(crate) description: NonEmptyString,
    /// The execution term admission derived for the call, as the act's
    /// legitimacy.
    pub(crate) legitimacy: TermLegitimacy,
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
        self.act.as_ref()?.decision
    }
}

/// The execution outcome that closes an act into its sealed body.
pub(crate) struct ActOutcome<'a> {
    /// Whether the call succeeded, driving the outcome criterion's status.
    pub(crate) succeeded: bool,
    /// The evidence of the outcome and its verification.
    pub(crate) evidence_refs: &'a [Reference],
    pub(crate) verification_refs: &'a [Reference],
}

/// A governed call's act, opened with its intent and form.
pub(crate) struct RuntimeAct {
    id: NonEmptyString,
    form: ActForm,
    intent: Intent,
    reason: Option<NonEmptyString>,
    verified_bindings: Vec<CriterionBinding>,
    target_refs: Vec<Reference>,
    produced: Vec<Reference>,
    context_ref: Option<Reference>,
    operation: Option<runx_contracts::OperationIntent>,
    previous: Option<Reference>,
}

impl RuntimeAct {
    /// The act a governed call performs: the one its caller states, or an
    /// observation whose purpose is its operation's declared description.
    pub(crate) fn for_call(step_id: &str, call: &SealedCall) -> Self {
        let stated = call.act.as_ref();
        Self {
            id: format!("act_{step_id}").into(),
            form: stated.map_or(ActForm::Observation, |stated| stated.form),
            intent: Intent {
                purpose: stated
                    .map_or_else(|| call.description.clone(), |stated| stated.purpose.clone()),
                legitimacy: call.legitimacy.clone(),
            },
            reason: stated.and_then(|stated| stated.reason.clone()),
            verified_bindings: Vec::new(),
            target_refs: call.target_ref.iter().cloned().collect(),
            produced: call.effects.clone(),
            context_ref: call.context_ref.clone(),
            operation: call.operation_intent.clone(),
            previous: call.previous(),
        }
    }

    /// Bind the contract checks the runtime ran on the call's result.
    pub(crate) fn with_verified_criteria(mut self, bindings: Vec<CriterionBinding>) -> Self {
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
            criterion_id: Criterion::StepOutcome,
            status: if outcome.succeeded {
                CriterionStatus::Verified
            } else {
                CriterionStatus::Failed
            },
            evidence_refs: outcome.evidence_refs.to_vec(),
            verification_refs: outcome.verification_refs.to_vec(),
        }];
        criterion_bindings.extend(self.verified_bindings);
        ReceiptAct {
            id: self.id,
            form: self.form,
            intent: self.intent,
            reason: self.reason,
            criterion_bindings,
            target_refs: self.target_refs,
            artifact_refs,
            context_ref: self.context_ref,
            operation: self.operation,
            previous: self.previous,
        }
    }
}
