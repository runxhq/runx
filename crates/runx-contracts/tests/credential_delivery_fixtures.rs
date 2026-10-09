use serde::Deserialize;

use runx_contracts::CredentialDeliveryObservation;

use crate::fixture_support::roundtrip;

#[derive(Debug, Deserialize)]
struct Fixture {
    expected: serde_json::Value,
}

fn observation() -> Result<Fixture, serde_json::Error> {
    serde_json::from_str(include_str!(
        "../../../fixtures/contracts/credential-delivery/observation.json"
    ))
}

#[test]
fn credential_delivery_observation_matches_its_wire_shape() -> Result<(), serde_json::Error> {
    roundtrip::<CredentialDeliveryObservation>(observation()?.expected)
}

#[test]
fn credential_delivery_observation_rejects_raw_secret_material() -> Result<(), serde_json::Error> {
    let mut observation = observation()?.expected;
    observation["api_key"] = serde_json::Value::String("super-secret-token".to_owned());

    let result = serde_json::from_value::<CredentialDeliveryObservation>(observation);

    assert!(result.is_err());
    Ok(())
}
