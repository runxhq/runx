mod assertions;
pub mod fixtures;
mod json_assertions;
mod package;
mod provider_responses;
pub mod runner;
mod transport;
mod turns;

pub use package::PackageHarnessReport;
pub(crate) use package::run_package_harness_with_effects;

use std::collections::{BTreeMap, BTreeSet};

const AMBIENT_HARNESS_AUTHORITY_ENV: &[&str] = &[
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
pub(crate) fn isolate_harness_environment(
    env: &mut BTreeMap<String, String>,
    credential_variables: &BTreeSet<String>,
) {
    env.retain(|name, _| {
        !AMBIENT_HARNESS_AUTHORITY_ENV.contains(&name.as_str())
            && !credential_variables.contains(name)
    });
}

pub(crate) use crate::execution_environment::credential_variables;

pub use assertions::HarnessReplayReceipt;
pub use fixtures::{
    HarnessExpectedStatus, HarnessFixture, HarnessFixtureCase, HarnessFixtureError,
    HarnessFixtureKind, HarnessFixtureStepOracle, HarnessSetup, ReceiptExpectation, list_cases,
    load_harness_fixture,
};
pub use runner::{
    HarnessOptions, HarnessReplayError, HarnessReplayOutput, replay_harness_fixture,
    run_harness_fixture, run_harness_fixture_with_adapter,
};

#[cfg(test)]
mod environment_tests {
    use std::collections::{BTreeMap, BTreeSet};

    use super::isolate_harness_environment;

    #[test]
    fn harness_environment_removes_ambient_authority_and_connector_credentials() {
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

        isolate_harness_environment(&mut env, &BTreeSet::from(["EXAMPLE_TOKEN".to_owned()]));

        assert_eq!(env.keys().map(String::as_str).collect::<Vec<_>>(), ["PATH"]);
    }
}
