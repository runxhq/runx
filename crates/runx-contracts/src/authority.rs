//! Authority algebra: terms, verbs, bounds and attenuation.
use serde::{Deserialize, Serialize};

use crate::Reference;
use crate::schema::{IsoDateTime, NonEmptyString, RunxSchema};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum AuthorityResourceFamily {
    /// Operations on one connector instance.
    Connector,
}

/// What a run may change, and whether a change waits (D25). A host sets it
/// on a run, and a method or step only narrows it: `none` is narrower than
/// `ask`, and `ask` than `allow`.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, RunxSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Effects {
    /// Nothing changes in the workspace or at a provider, at any depth: every
    /// local effect, every provider write and every `store` write is
    /// refused, even with a term.
    None,
    /// Every write with no approval of its own waits on a person or a
    /// standing approval, at any depth, local or provider. A package's own
    /// `store` state is exempt.
    Ask,
    /// Nothing waits. The default with no host.
    #[default]
    Allow,
}

impl Effects {
    /// These effects, narrowed by `by` where it is narrower. A narrowing never
    /// widens.
    #[must_use]
    pub fn narrowed(self, by: Option<Self>) -> Self {
        by.map_or(self, |by| self.min(by))
    }
}

/// Where a run's file, git and command tools may go (D25). A host sets it on
/// a run, and a method or step only narrows it: `workspace` is narrower than
/// `inputs`.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, RunxSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Reach {
    /// Inside the workspace, and off secret files and runx's own state.
    Workspace,
    /// The roots a run's inputs name, another repository by its absolute path
    /// included. The default with no host.
    #[default]
    Inputs,
}

impl Reach {
    /// This reach, narrowed by `by` where it is narrower. A narrowing never
    /// widens.
    #[must_use]
    pub fn narrowed(self, by: Option<Self>) -> Self {
        by.map_or(self, |by| self.min(by))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum AuthorityVerb {
    Read,
    Write,
    Comment,
    Review,
    Merge,
    Create,
    Update,
    Delete,
    Execute,
    Revoke,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct AuthorityBounds {
    /// Connector operations, as `<connector>/<operation>` or `<connector>/*`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub operations: Vec<NonEmptyString>,
    /// Provider grant scopes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scopes: Vec<NonEmptyString>,
    /// Provider targets: exact values, `<prefix>*`, or `*`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub targets: Vec<NonEmptyString>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uses: Option<AuthorityUseLimit>,
    /// The packages every call's chain must be covered by, in any order
    /// (D31). An absent bound covers nothing, as every bound does, so a
    /// delegation states its own and an execution term names its chain. A
    /// package is listed once.
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "crate::schema::deserialize_unique_items"
    )]
    #[runx_schema(unique_items)]
    pub packages: Vec<PackagePattern>,
}

/// The patterns that cover every package: a capability's, which only a
/// delegation or a run's `packages` narrows.
#[must_use]
pub fn every_package() -> Vec<PackagePattern> {
    vec![
        PackagePattern(format!("{WILDCARD}/{WILDCARD}")),
        PackagePattern(format!("{PROJECT_PREFIX}{WILDCARD}")),
    ]
}

const WILDCARD: &str = "*";
const PROJECT_PREFIX: &str = "project:";
const PACKAGE_PATTERN: &str =
    r"^(([a-z0-9][a-z0-9-]*|\*)/([a-z0-9][a-z0-9-]*|\*)|project:([A-Za-z0-9][A-Za-z0-9._-]*|\*))$";

/// A package, or a pattern of packages: `<publisher>/<name>`, a registry or
/// bundled package, or `project:<name>`, one the project authored. `*`
/// stands for one segment and never crosses `/`, so `acme/*` covers acme's
/// packages and `project:*` the project's own. A publisher never holds `/`,
/// so neither form can pass for the other.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct PackagePattern(String);

impl PackagePattern {
    /// `None` when `value` is neither a package nor a pattern of them.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Option<Self> {
        let value = value.into();
        Self::segments(&value).is_some().then_some(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether every package `child` names is one this pattern covers.
    #[must_use]
    pub fn covers(&self, child: &Self) -> bool {
        let (Some(parent), Some(child)) = (Self::segments(&self.0), Self::segments(&child.0))
        else {
            return false;
        };
        parent.0 == child.0
            && parent
                .1
                .iter()
                .zip(&child.1)
                .all(|(parent, child)| *parent == WILDCARD || parent == child)
    }

    /// Whether the pattern names the project's own packages, and its
    /// segments: the project name alone, or the publisher and the name, each
    /// a name or `*`.
    fn segments(value: &str) -> Option<(bool, Vec<&str>)> {
        let name = |segment: &str, project: bool| {
            segment == WILDCARD
                || !segment.is_empty()
                    && segment
                        .bytes()
                        .enumerate()
                        .all(|(index, byte)| match (project, index) {
                            (true, 0) => byte.is_ascii_alphanumeric(),
                            (true, _) => {
                                byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-')
                            }
                            (false, 0) => byte.is_ascii_lowercase() || byte.is_ascii_digit(),
                            (false, _) => {
                                byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'
                            }
                        })
        };
        if let Some(project) = value.strip_prefix(PROJECT_PREFIX) {
            return name(project, true).then(|| (true, vec![project]));
        }
        let (publisher, package) = value.split_once('/')?;
        (name(publisher, false) && name(package, false)).then(|| (false, vec![publisher, package]))
    }
}

impl std::fmt::Display for PackagePattern {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for PackagePattern {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value.clone()).ok_or_else(|| {
            serde::de::Error::custom(format!(
                "{value:?} is not a package: <publisher>/<name> or project:<name>, with * for one segment"
            ))
        })
    }
}

impl RunxSchema for PackagePattern {
    fn json_schema() -> serde_json::Value {
        serde_json::json!({"type": "string", "pattern": PACKAGE_PATTERN})
    }
}

/// How many times a term may be used. An execution term is single use.
/// Usage is reserved atomically with each effect attempt, never counted from
/// receipts.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct AuthorityUseLimit {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub per_day: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct AuthorityApproval {
    pub approval_ref: Reference,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approved_by_ref: Option<Reference>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approved_at: Option<IsoDateTime>,
}

/// How an act's legitimacy names the execution term it acts under.
const TERM_LEGITIMACY_PREFIX: &str = "runx:authority_term:";
const TERM_LEGITIMACY_PATTERN: &str = r"^runx:authority_term:\S+$";
const TERM_LEGITIMACY_MAX_LENGTH: usize = 512;

/// An act's legitimacy: a reference to the execution term it was admitted
/// under, `runx:authority_term:<term id>`, never prose. Every act carries one,
/// and a verifier checks the receipt's authority holds the term it names.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct TermLegitimacy(String);

impl TermLegitimacy {
    /// The legitimacy of an act admitted under `term_id`; `None` when the id
    /// is empty or holds whitespace.
    #[must_use]
    pub fn for_term(term_id: &str) -> Option<Self> {
        Self::new(format!("{TERM_LEGITIMACY_PREFIX}{term_id}"))
    }

    /// `None` when `value` is not a reference to an execution term.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Option<Self> {
        let value = value.into();
        let valid = value.len() <= TERM_LEGITIMACY_MAX_LENGTH
            && value
                .strip_prefix(TERM_LEGITIMACY_PREFIX)
                .is_some_and(|id| {
                    !id.is_empty() && !id.chars().any(|c| c.is_whitespace() || c.is_control())
                });
        valid.then_some(Self(value))
    }

    /// The execution term this legitimacy names.
    #[must_use]
    pub fn term_id(&self) -> &str {
        &self.0[TERM_LEGITIMACY_PREFIX.len()..]
    }
}

impl std::fmt::Display for TermLegitimacy {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for TermLegitimacy {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value.clone()).ok_or_else(|| {
            serde::de::Error::custom(format!("{value:?} is not a reference to an execution term"))
        })
    }
}

impl RunxSchema for TermLegitimacy {
    fn json_schema() -> serde_json::Value {
        serde_json::json!({
            "type": "string",
            "pattern": TERM_LEGITIMACY_PATTERN,
            "maxLength": TERM_LEGITIMACY_MAX_LENGTH,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.authority_term.v1")]
pub struct AuthorityTerm {
    pub term_id: NonEmptyString,
    pub principal_ref: Reference,
    pub resource_ref: Reference,
    pub resource_family: AuthorityResourceFamily,
    pub verbs: Vec<AuthorityVerb>,
    pub bounds: AuthorityBounds,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub approvals: Vec<AuthorityApproval>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<IsoDateTime>,
    pub issued_by_ref: Reference,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credential_ref: Option<Reference>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum AuthoritySubsetRelation {
    Equal,
    Subset,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct AuthoritySubsetComparison {
    pub child_term_id: NonEmptyString,
    pub parent_term_id: NonEmptyString,
    pub relation: AuthoritySubsetRelation,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
pub enum AuthoritySubsetResult {
    #[serde(rename = "subset")]
    Subset,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(
    id = "runx.authority_subset_proof.v1",
    url = "https://schemas.runx.ai/runx/authority/subset-proof/v1.json"
)]
pub struct AuthoritySubsetProof {
    pub parent_authority_ref: Reference,
    pub comparison_algorithm: NonEmptyString,
    pub result: AuthoritySubsetResult,
    pub compared_terms: Vec<AuthoritySubsetComparison>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proof_ref: Option<Reference>,
    pub checked_at: IsoDateTime,
}

/// The three kinds of evidence admission proves. Each has one job.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum AuthorityEvidenceKind {
    /// What the principal can do at the provider: a grant, credential or
    /// session. Authorizes only the principal acting directly.
    Capability,
    /// What an actor may do for the principal: a mandate, or child authority
    /// attenuated from a parent run. Narrows; never creates.
    Delegation,
    /// A person's confirmation of this exact intent, or of a bounded kind of
    /// intent. Satisfies an approval floor; never authorizes alone.
    Approval,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct AuthorityEvidence {
    pub kind: AuthorityEvidenceKind,
    pub reference: Reference,
    /// The capability or delegation term, sealed so the subset proofs can be
    /// re-checked offline. Approvals carry no term.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub term: Option<AuthorityTerm>,
}

/// How an execution term was derived: the evidence admission proved, ordered
/// capability first, then each delegation link from principal to actor, then
/// any approval; and one subset proof per parent term.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.authority_derivation.v1")]
pub struct AuthorityDerivation {
    pub term_id: NonEmptyString,
    pub intent_digest: NonEmptyString,
    pub evidence: Vec<AuthorityEvidence>,
    pub proofs: Vec<AuthoritySubsetProof>,
    pub derived_at: IsoDateTime,
}

#[cfg(test)]
mod tests {
    /// A package is `<publisher>/<name>` or `project:<name>`, and `*` stands
    /// for one segment and never crosses `/`, so neither form passes for the
    /// other.
    #[test]
    fn a_package_pattern_covers_one_segment_per_star() {
        let pattern = |value: &str| super::PackagePattern::new(value);
        for valid in [
            "acme/counter",
            "runx/memory",
            "acme/*",
            "*/*",
            "project:work",
            "project:My.Skill_1",
            "project:*",
        ] {
            assert!(pattern(valid).is_some(), "{valid}");
        }
        for invalid in [
            "acme",
            "*",
            "acme/foo/github",
            "Acme/x",
            "project:",
            "project:a/b",
            "acme/",
            "/x",
            "acme/x*",
        ] {
            assert!(pattern(invalid).is_none(), "{invalid}");
        }
        let covers = |parent: &str, child: &str| {
            pattern(parent)
                .zip(pattern(child))
                .is_some_and(|(parent, child)| parent.covers(&child))
        };
        assert!(covers("*/*", "acme/counter"));
        assert!(covers("*/*", "acme/*"));
        assert!(covers("acme/*", "acme/counter"));
        assert!(covers("project:*", "project:work"));
        assert!(!covers("*/*", "project:work"));
        assert!(!covers("project:*", "acme/counter"));
        assert!(!covers("acme/*", "other/counter"));
        assert!(!covers("acme/counter", "acme/*"));
        assert_eq!(super::every_package().len(), 2);
        assert!(super::every_package().iter().all(|parent| {
            ["acme/x", "project:y"]
                .iter()
                .any(|child| covers(parent.as_str(), child))
        }));
    }

    /// `runx.authority_term.v1` changes in place (authority.md): a term names
    /// no resource family, since the instance it names is the boundary, and
    /// its packages never repeat.
    #[test]
    fn a_term_names_no_resource_family_and_its_packages_never_repeat() {
        use crate::schema::RunxSchema;
        let term = |extra: serde_json::Value, packages: serde_json::Value| {
            let mut term = serde_json::json!({
                "term_id": "capability",
                "principal_ref": {"type": "principal", "uri": "runx:principal:operator"},
                "resource_ref": {"type": "connector_instance", "uri": "runx:connector_instance:acme"},
                "verbs": ["read"],
                "bounds": {"operations": ["acme/*"], "packages": packages},
                "issued_by_ref": {"type": "principal", "uri": "runx:principal:operator"},
            });
            if let (Some(term), Some(extra)) = (term.as_object_mut(), extra.as_object()) {
                term.extend(extra.clone());
            }
            serde_json::from_value::<super::AuthorityTerm>(term)
        };
        let none = serde_json::json!({});
        assert!(
            term(
                none.clone(),
                serde_json::json!(["acme/counter", "project:*"])
            )
            .is_ok()
        );
        assert!(
            term(
                serde_json::json!({"resource_family": "connector"}),
                serde_json::json!(["acme/counter"])
            )
            .is_err(),
            "a term carrying resource_family is refused"
        );
        assert!(
            term(none, serde_json::json!(["acme/counter", "acme/counter"])).is_err(),
            "a package named twice is refused"
        );
        let schema = super::AuthorityTerm::json_schema();
        assert!(
            schema["properties"].get("resource_family").is_none(),
            "{schema}"
        );
        assert_eq!(
            schema["properties"]["bounds"]["properties"]["packages"]["uniqueItems"],
            serde_json::json!(true),
            "{schema}"
        );
    }

    use super::{Effects, Reach};

    #[test]
    fn effects_and_reach_only_narrow() {
        assert!(Effects::None < Effects::Ask && Effects::Ask < Effects::Allow);
        assert!(Reach::Workspace < Reach::Inputs);
        assert_eq!(Effects::default(), Effects::Allow);
        assert_eq!(Reach::default(), Reach::Inputs);

        assert_eq!(Effects::Ask.narrowed(Some(Effects::None)), Effects::None);
        assert_eq!(Effects::Ask.narrowed(Some(Effects::Allow)), Effects::Ask);
        assert_eq!(Effects::None.narrowed(Some(Effects::Ask)), Effects::None);
        assert_eq!(Effects::Allow.narrowed(None), Effects::Allow);
        assert_eq!(
            Reach::Workspace.narrowed(Some(Reach::Inputs)),
            Reach::Workspace
        );
        assert_eq!(
            Reach::Inputs.narrowed(Some(Reach::Workspace)),
            Reach::Workspace
        );
    }

    #[test]
    fn effects_and_reach_read_only_their_words() -> Result<(), serde_json::Error> {
        assert_eq!(serde_json::from_str::<Effects>("\"ask\"")?, Effects::Ask);
        assert_eq!(
            serde_json::from_str::<Reach>("\"workspace\"")?,
            Reach::Workspace
        );
        for word in ["\"free\"", "\"read_only\"", "true"] {
            assert!(serde_json::from_str::<Effects>(word).is_err(), "{word}");
        }
        assert!(serde_json::from_str::<Reach>("\"agent\"").is_err());
        Ok(())
    }
}
