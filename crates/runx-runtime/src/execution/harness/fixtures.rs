//! Filesystem admission and public error projection for parser-owned harness fixtures.

use std::fs;
use std::path::{Path, PathBuf};

pub use runx_parser::test_fixture::{
    ReceiptExpectation, TestExpectation, TestExpectedStatus, TestFixture, TestFixtureKind,
    TestJsonExpectation, TestSetup,
};
use thiserror::Error;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestFixtureCase {
    pub name: &'static str,
    pub fixture_path: &'static str,
    pub root_oracle_path: &'static str,
    pub step_oracles: &'static [TestFixtureStepOracle],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestFixtureStepOracle {
    pub step_id: &'static str,
    pub oracle_path: &'static str,
}

const TEST_FIXTURE_CASES: &[TestFixtureCase] = &[
    TestFixtureCase {
        name: "echo-skill",
        fixture_path: "fixtures/harness/echo-skill.yaml",
        root_oracle_path: "fixtures/harness/oracle/echo-skill.receipt.json",
        step_oracles: &[TestFixtureStepOracle {
            step_id: "default",
            oracle_path: "fixtures/harness/oracle/echo-skill.default.json",
        }],
    },
    TestFixtureCase {
        name: "sequential-workflow",
        fixture_path: "fixtures/harness/sequential-workflow.yaml",
        root_oracle_path: "fixtures/harness/oracle/sequential-workflow.receipt.json",
        step_oracles: &[
            TestFixtureStepOracle {
                step_id: "first",
                oracle_path: "fixtures/harness/oracle/sequential-workflow.first.json",
            },
            TestFixtureStepOracle {
                step_id: "second",
                oracle_path: "fixtures/harness/oracle/sequential-workflow.second.json",
            },
        ],
    },
];

#[must_use]
pub fn list_cases() -> &'static [TestFixtureCase] {
    TEST_FIXTURE_CASES
}

/// Stable runtime-facing error surface. Syntax and fixture semantics are owned
/// by `runx-parser`; runtime adds only filesystem admission.
#[derive(Debug, Error)]
pub enum TestFixtureError {
    #[error("failed to read harness fixture {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error(transparent)]
    Parser(#[from] runx_parser::test_fixture::TestFixtureError),
}

pub fn load_test_fixture(path: impl AsRef<Path>) -> Result<TestFixture, TestFixtureError> {
    let path = path.as_ref();
    let contents = fs::read_to_string(path).map_err(|source| TestFixtureError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(runx_parser::test_fixture::parse_test_fixture(&contents)?)
}
