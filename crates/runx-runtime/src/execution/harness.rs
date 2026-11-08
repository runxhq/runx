mod assertions;
pub mod fixtures;
mod json_assertions;
mod package;
mod provider_responses;
pub mod runner;
mod transport;
mod turns;

pub use package::PackageTestReport;
pub(crate) use package::{run_package_case_with_effects, run_package_test_with_effects};

use std::collections::{BTreeMap, BTreeSet};

const AMBIENT_TEST_AUTHORITY_ENV: &[&str] = &[
    "RUNX_AGENT_API_KEY",
    "RUNX_AGENT_ENDPOINT_URL",
    "RUNX_AGENT_AUTH_MODE",
    "RUNX_PUBLIC_API_ALLOW_PRIVATE_NETWORK",
    "RUNX_PUBLIC_API_BASE_URL",
    "RUNX_PUBLIC_API_TOKEN",
];

/// Remove ambient authority and credential custody from a harness process:
/// the variables every connector in reach of the skill delivers a credential
/// in. A case may add an explicit fake binding back through its own `env`,
/// but a developer's live account state must never become implicit test
/// input.
pub(crate) fn isolate_test_environment(
    env: &mut BTreeMap<String, String>,
    credential_variables: &BTreeSet<String>,
) {
    env.retain(|name, _| {
        !AMBIENT_TEST_AUTHORITY_ENV.contains(&name.as_str()) && !credential_variables.contains(name)
    });
}

pub(crate) use crate::execution_environment::credential_variables;

pub use assertions::TestReplayReceipt;
pub use fixtures::{
    ReceiptExpectation, TestExpectedStatus, TestFixture, TestFixtureCase, TestFixtureError,
    TestFixtureStepOracle, TestSetup, list_cases, load_test_fixture,
};
pub use runner::{
    TestOptions, TestReplayError, TestReplayOutput, replay_test_fixture, run_test_fixture,
    run_test_fixture_with_adapter,
};

#[cfg(test)]
mod environment_tests {
    use std::collections::{BTreeMap, BTreeSet};

    use super::isolate_test_environment;

    #[test]
    fn test_environment_removes_ambient_authority_and_connector_credentials() {
        let mut env = BTreeMap::from([
            ("PATH".to_owned(), "/usr/bin".to_owned()),
            ("EXAMPLE_TOKEN".to_owned(), "live-secret".to_owned()),
            (
                "RUNX_PUBLIC_API_TOKEN".to_owned(),
                "live-cloud-token".to_owned(),
            ),
            ("RUNX_AGENT_API_KEY".to_owned(), "live-agent-key".to_owned()),
            (
                "RUNX_AGENT_ENDPOINT_URL".to_owned(),
                "http://127.0.0.1:18081/v1/chat/completions".to_owned(),
            ),
            ("RUNX_AGENT_AUTH_MODE".to_owned(), "local_none".to_owned()),
        ]);

        isolate_test_environment(&mut env, &BTreeSet::from(["EXAMPLE_TOKEN".to_owned()]));

        assert_eq!(env.keys().map(String::as_str).collect::<Vec<_>>(), ["PATH"]);
    }
}
