//! A package's own evals, in the Agent Skills format: the queries a host
//! should or shouldn't select it for (`eval_queries.json`), and the evals of
//! the work it does (`evals/evals.json`). They are part of a package's
//! declared surface, as its tests are: the parser validates both against
//! these types, and a package's inspection carries them, so a reader never
//! reads the files themselves.

use serde::{Deserialize, Serialize};

use crate::connector::Digest;
use crate::schema::{NonEmptyString, RunxSchema};

/// One query of `eval_queries.json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct EvalQuery {
    /// A request, as a person would make it.
    pub query: NonEmptyString,
    /// Whether a host should select the package for it.
    pub should_trigger: bool,
}

/// `evals/evals.json`: the package's evals, under its own name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct PackageEvals {
    /// The package's name.
    pub skill_name: NonEmptyString,
    pub evals: Vec<PackageEval>,
}

/// One eval: a prompt, and what the work it asks for should produce.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct PackageEval {
    /// Its id, unique among the package's evals.
    pub id: u64,
    /// The request an attempt is given.
    pub prompt: NonEmptyString,
    /// What a met attempt produces.
    pub expected_output: NonEmptyString,
    /// The files the eval gives an attempt, relative to the package.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<NonEmptyString>,
    /// Each further expectation an attempt is held to, every one required.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub expectations: Vec<NonEmptyString>,
    /// The package's method the eval is of, when it names one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<NonEmptyString>,
}

/// A file an eval names, as the package holds it: what an attempt at the
/// eval is given to work from, such as the prior run it builds on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct EvalFile {
    /// Its path in the package, as the eval names it.
    pub path: NonEmptyString,
    /// The digest of its bytes.
    pub digest: Digest,
    /// Its text.
    pub content: String,
}
