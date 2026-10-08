//! Filesystem admission and public error projection for parser-owned harness fixtures.

use std::fs;
use std::path::{Path, PathBuf};

pub use runx_parser::harness_fixture::{
    HarnessExpectation, HarnessExpectedStatus, HarnessFixture, HarnessFixtureKind,
    HarnessJsonExpectation, HarnessSetup, ReceiptExpectation,
};
use thiserror::Error;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HarnessFixtureCase {
    pub name: &'static str,
    pub fixture_path: &'static str,
    pub root_oracle_path: &'static str,
    pub step_oracles: &'static [HarnessFixtureStepOracle],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HarnessFixtureStepOracle {
    pub step_id: &'static str,
    pub oracle_path: &'static str,
}

const HARNESS_FIXTURE_CASES: &[HarnessFixtureCase] = &[
    HarnessFixtureCase {
        name: "echo-skill",
        fixture_path: "fixtures/harness/echo-skill.yaml",
        root_oracle_path: "fixtures/harness/oracle/echo-skill.receipt.json",
        step_oracles: &[HarnessFixtureStepOracle {
            step_id: "default",
            oracle_path: "fixtures/harness/oracle/echo-skill.default.json",
        }],
    },
    HarnessFixtureCase {
        name: "sequential-graph",
        fixture_path: "fixtures/harness/sequential-graph.yaml",
        root_oracle_path: "fixtures/harness/oracle/sequential-graph.receipt.json",
        step_oracles: &[
            HarnessFixtureStepOracle {
                step_id: "first",
                oracle_path: "fixtures/harness/oracle/sequential-graph.first.json",
            },
            HarnessFixtureStepOracle {
                step_id: "second",
                oracle_path: "fixtures/harness/oracle/sequential-graph.second.json",
            },
        ],
    },
];

#[must_use]
pub fn list_cases() -> &'static [HarnessFixtureCase] {
    HARNESS_FIXTURE_CASES
}

/// Stable runtime-facing error surface. Syntax and fixture semantics are owned
/// by `runx-parser`; runtime adds only filesystem admission.
#[derive(Debug, Error)]
pub enum HarnessFixtureError {
    #[error("failed to read harness fixture {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error(transparent)]
    Parser(#[from] runx_parser::harness_fixture::HarnessFixtureError),
}

pub fn load_harness_fixture(path: impl AsRef<Path>) -> Result<HarnessFixture, HarnessFixtureError> {
    let path = path.as_ref();
    let contents = fs::read_to_string(path).map_err(|source| HarnessFixtureError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(runx_parser::harness_fixture::parse_harness_fixture(
        &contents,
    )?)
}
