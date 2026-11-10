//! `runx.verdict.v1` and `runx.criteria.v1`: whether an exact subject meets a
//! fixed set of checks it didn't choose, each check answered, with the
//! evidence the evaluator obtained itself.
//!
//! A verdict is shared data with deterministic validation, never a kernel
//! subsystem: an evaluator is an ordinary step or call that returns one, and
//! [`Verdict::decide`] is the one rule every reader of it applies. The rule
//! reads the criteria set, never what the evaluator chose to answer, so an
//! evaluator that leaves out the check it failed gets `inconclusive`, never
//! `met`.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, de};
use serde_json::{Value, json};
use thiserror::Error;

use crate::canonical_json::canonical_digest;
use crate::connector::Digest;
use crate::connector::ids::identifier;
use crate::schema::{BoundedString, NonEmptyString, NonEmptyVec, RunxSchema, SchemaMapKey};

pub const VERDICT_SCHEMA: &str = "runx.verdict.v1";
pub const CRITERIA_SCHEMA: &str = "runx.criteria.v1";

/// A criterion id: lowercase, starting with a letter, then letters, digits
/// and underscores.
pub const CRITERION_ID_PATTERN: &str = "^[a-z][a-z0-9_]*$";
pub const CRITERION_ID_MAX_LENGTH: usize = 128;

identifier!(
    /// A criterion's stable id, unique in its set, such as
    /// `every_repository_read`.
    CriterionId,
    CRITERION_ID_PATTERN,
    CRITERION_ID_MAX_LENGTH,
    is_criterion_id,
    "criterion id"
);

#[must_use]
pub fn is_criterion_id(value: &str) -> bool {
    let bytes = value.as_bytes();
    (1..=CRITERION_ID_MAX_LENGTH).contains(&bytes.len())
        && bytes[0].is_ascii_lowercase()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'_')
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
pub enum VerdictSchema {
    #[serde(rename = "runx.verdict.v1")]
    V1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
pub enum CriteriaSchema {
    #[serde(rename = "runx.criteria.v1")]
    V1,
}

/// An answer: to one criterion, or for the whole.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum VerdictAnswer {
    Met,
    NotMet,
    Inconclusive,
}

/// Something named exactly: its `ref` and its exact `revision`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct Pinned {
    #[serde(rename = "ref")]
    pub reference: BoundedString<2048>,
    pub revision: BoundedString<256>,
}

impl fmt::Display for Pinned {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}@{}",
            self.reference.as_str(),
            self.revision.as_str()
        )
    }
}

/// What failed: what was expected, what happened, and where. The criterion
/// says what failed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct AssessmentFailure {
    pub expected: NonEmptyString,
    pub actual: NonEmptyString,
    #[serde(rename = "where")]
    pub location: NonEmptyString,
}

/// One answer to one criterion, with the references to the sealed calls or
/// receipts the evaluator obtained against this subject. A `not_met` answer
/// says what failed, and a `met` one has nothing to say.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(tag = "verdict", rename_all = "snake_case", deny_unknown_fields)]
pub enum Assessment {
    Met {
        id: CriterionId,
        evidence: Vec<BoundedString<2048>>,
    },
    NotMet {
        id: CriterionId,
        evidence: Vec<BoundedString<2048>>,
        failure: AssessmentFailure,
    },
    Inconclusive {
        id: CriterionId,
        evidence: Vec<BoundedString<2048>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        failure: Option<AssessmentFailure>,
    },
}

impl Assessment {
    #[must_use]
    pub fn id(&self) -> &CriterionId {
        match self {
            Self::Met { id, .. } | Self::NotMet { id, .. } | Self::Inconclusive { id, .. } => id,
        }
    }

    #[must_use]
    pub fn answer(&self) -> VerdictAnswer {
        match self {
            Self::Met { .. } => VerdictAnswer::Met,
            Self::NotMet { .. } => VerdictAnswer::NotMet,
            Self::Inconclusive { .. } => VerdictAnswer::Inconclusive,
        }
    }

    #[must_use]
    pub fn evidence(&self) -> &[BoundedString<2048>] {
        match self {
            Self::Met { evidence, .. }
            | Self::NotMet { evidence, .. }
            | Self::Inconclusive { evidence, .. } => evidence,
        }
    }
}

/// Who judged: the package and method of the step that returned the
/// verdict, and the run it returned it in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct Evaluator {
    pub package: NonEmptyString,
    pub method: NonEmptyString,
    pub run: NonEmptyString,
}

/// Whether an exact subject meets a fixed set of checks it didn't choose,
/// each check answered, with the evidence the evaluator obtained itself.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.verdict.v1")]
pub struct Verdict {
    pub schema: VerdictSchema,
    /// The whole: any required criterion not_met makes it not_met; otherwise
    /// any missing or unresolved one makes it inconclusive; only every
    /// required one met makes it met.
    pub verdict: VerdictAnswer,
    /// What was judged. The verdict counts only for this exact revision.
    pub subject: Pinned,
    /// Which runx.criteria.v1 set it answered, by its revision digest.
    pub criteria_set: Pinned,
    /// One answer per check. Every answer stays visible, even when one
    /// failure already decides the whole.
    pub assessments: Vec<Assessment>,
    /// Who judged. The runtime sets it from the step that returned the
    /// verdict, never the step.
    pub evaluator: Evaluator,
}

/// One check: its stable id, whether it's required, and the question that
/// must be answered yes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct VerdictCriterion {
    pub id: CriterionId,
    pub required: bool,
    pub question: BoundedString<1024>,
}

/// A fixed set of checks a verdict answers. Deterministic steps produce it,
/// bound to the subject's revision, or a person writes it, and whoever
/// produced the subject can't change it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.criteria.v1")]
pub struct Criteria {
    pub schema: CriteriaSchema,
    /// The set's digest, which a verdict binds.
    pub revision: Digest,
    pub criteria: NonEmptyVec<VerdictCriterion>,
}

/// A refusal to read a verdict at all. A refused verdict answers nothing,
/// which is not the same as `inconclusive`.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum VerdictRefused {
    #[error("the verdict judges {named}, not {expected}")]
    Subject { expected: String, named: String },
    #[error("the verdict answers the criteria set {named}, not {expected}")]
    CriteriaSet { expected: String, named: String },
    #[error("the criteria set's revision {stated} is not its digest {computed}")]
    CriteriaRevision { stated: String, computed: String },
    #[error("the criteria set holds {0} more than once")]
    RepeatedCriterion(CriterionId),
    #[error("criterion {0} is not in the criteria set")]
    UnknownCriterion(CriterionId),
    #[error("criterion {0} is answered more than once")]
    DuplicateAnswer(CriterionId),
    #[error("the criteria set can't be digested: {0}")]
    Undigestible(String),
}

/// What the one rule decided, and the required criteria left unanswered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decided {
    pub verdict: VerdictAnswer,
    pub unresolved: Vec<CriterionId>,
}

impl Criteria {
    /// The set over `criteria`, its revision computed.
    pub fn new(criteria: NonEmptyVec<VerdictCriterion>) -> Result<Self, VerdictRefused> {
        let revision = digest_of(&criteria)?;
        Ok(Self {
            schema: CriteriaSchema::V1,
            revision,
            criteria,
        })
    }

    /// The set as a verdict may bind it: its revision is the digest of what
    /// it holds, and no id is in it twice.
    pub fn check(&self) -> Result<(), VerdictRefused> {
        let computed = digest_of(&self.criteria)?;
        if computed != self.revision {
            return Err(VerdictRefused::CriteriaRevision {
                stated: self.revision.to_string(),
                computed: computed.to_string(),
            });
        }
        let mut seen = BTreeSet::new();
        for criterion in self.criteria.iter() {
            if !seen.insert(&criterion.id) {
                return Err(VerdictRefused::RepeatedCriterion(criterion.id.clone()));
            }
        }
        Ok(())
    }
}

/// The revision of a criteria set: the canonical digest of the document
/// without its revision, so a changed question or a check added or made
/// optional is another revision.
fn digest_of(criteria: &NonEmptyVec<VerdictCriterion>) -> Result<Digest, VerdictRefused> {
    let document = json!({ "schema": CRITERIA_SCHEMA, "criteria": criteria });
    let digest = canonical_digest(&document)
        .map_err(|error| VerdictRefused::Undigestible(error.to_string()))?;
    Digest::new(digest.clone()).ok_or(VerdictRefused::Undigestible(digest))
}

impl Verdict {
    /// Check the verdict against the subject the reader expects and the
    /// criteria set it names, and decide the whole by the one rule: any
    /// required criterion `not_met` makes it `not_met`; otherwise any
    /// required one unanswered or `inconclusive` makes it `inconclusive`;
    /// only every required one `met` makes it `met`. The whole the evaluator
    /// stated decides nothing, and a criterion that isn't required is
    /// answered but decides nothing either.
    pub fn decide(
        &self,
        subject: &Pinned,
        criteria_ref: &str,
        criteria: &Criteria,
    ) -> Result<Decided, VerdictRefused> {
        if &self.subject != subject {
            return Err(VerdictRefused::Subject {
                expected: subject.to_string(),
                named: self.subject.to_string(),
            });
        }
        if self.criteria_set.reference.as_str() != criteria_ref
            || self.criteria_set.revision.as_str() != criteria.revision.as_str()
        {
            return Err(VerdictRefused::CriteriaSet {
                expected: format!("{criteria_ref}@{}", criteria.revision),
                named: self.criteria_set.to_string(),
            });
        }
        criteria.check()?;
        let known: BTreeSet<&CriterionId> = criteria
            .criteria
            .iter()
            .map(|criterion| &criterion.id)
            .collect();
        let mut answers = BTreeMap::new();
        for assessment in &self.assessments {
            let id = assessment.id();
            if !known.contains(id) {
                return Err(VerdictRefused::UnknownCriterion(id.clone()));
            }
            if answers.insert(id, assessment.answer()).is_some() {
                return Err(VerdictRefused::DuplicateAnswer(id.clone()));
            }
        }
        let mut failed = false;
        let mut unsure = false;
        let mut unresolved = Vec::new();
        for criterion in criteria
            .criteria
            .iter()
            .filter(|criterion| criterion.required)
        {
            match answers.get(&criterion.id) {
                Some(VerdictAnswer::Met) => {}
                Some(VerdictAnswer::NotMet) => failed = true,
                Some(VerdictAnswer::Inconclusive) => unsure = true,
                None => unresolved.push(criterion.id.clone()),
            }
        }
        let verdict = if failed {
            VerdictAnswer::NotMet
        } else if unsure || !unresolved.is_empty() {
            VerdictAnswer::Inconclusive
        } else {
            VerdictAnswer::Met
        };
        Ok(Decided {
            verdict,
            unresolved,
        })
    }
}

/// What a reader made of a verdict (`verdict.decide`): the whole the one rule
/// decided and the required criteria left unanswered, or why it was refused.
/// A refused verdict answers nothing, which isn't `inconclusive`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
#[runx_schema(id = "runx.verdict.decision.v1")]
pub enum VerdictDecision {
    Decided {
        verdict: VerdictAnswer,
        unresolved: Vec<CriterionId>,
    },
    Refused {
        reason: NonEmptyString,
    },
}

impl VerdictDecision {
    /// `verdict` read against the subject and criteria set the reader
    /// expects.
    #[must_use]
    pub fn of(
        verdict: &Verdict,
        subject: &Pinned,
        criteria_ref: &str,
        criteria: &Criteria,
    ) -> Self {
        match verdict.decide(subject, criteria_ref, criteria) {
            Ok(decided) => Self::Decided {
                verdict: decided.verdict,
                unresolved: decided.unresolved,
            },
            Err(refused) => Self::Refused {
                reason: refused.to_string().into(),
            },
        }
    }
}

#[cfg(test)]
mod tests;
