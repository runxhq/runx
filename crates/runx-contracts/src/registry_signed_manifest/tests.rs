use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use ring::signature::{ED25519, UnparsedPublicKey};

use super::*;

fn vectors() -> serde_json::Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/contracts/registry-signed-manifest/registry-signed-manifest-v1.vectors.json"
    );
    serde_json::from_str(&std::fs::read_to_string(path).expect("vectors")).expect("vectors json")
}

fn files(value: &serde_json::Value) -> Vec<RegistryContentFile<'_>> {
    value
        .as_array()
        .expect("files")
        .iter()
        .map(|file| RegistryContentFile {
            path: file["path"].as_str().expect("path"),
            content: file["content"].as_str().expect("content"),
        })
        .collect()
}

#[test]
fn content_digests_match_the_shared_vectors() {
    let vectors = vectors();
    for case in vectors["content_digests"].as_array().expect("cases") {
        assert_eq!(
            registry_content_digest(files(&case["files"])).expect("digest"),
            case["digest"].as_str().expect("digest"),
            "{}",
            case["name"]
        );
    }
    for case in vectors["invalid_content"].as_array().expect("cases") {
        assert!(registry_content_digest(files(&case["files"])).is_err(), "{}", case["name"]);
    }
}

#[test]
fn signs_the_canonical_envelope_and_binds_every_file() {
    let vectors = vectors();
    let public_key = STANDARD
        .decode(vectors["public_key_base64"].as_str().expect("key"))
        .expect("key bytes");
    for case in vectors["envelopes"].as_array().expect("envelopes") {
        let manifest: RegistrySignedManifest =
            serde_json::from_value(case["manifest"].clone()).expect("manifest");
        manifest.validate().expect("valid");
        let payload = manifest.signing_payload().expect("payload");
        assert_eq!(
            std::str::from_utf8(&payload).expect("utf8"),
            case["signing_payload"].as_str().expect("payload"),
            "{}",
            case["name"]
        );
        let signature = URL_SAFE_NO_PAD
            .decode(manifest.signature.value.trim_start_matches(REGISTRY_MANIFEST_SIGNATURE_PREFIX))
            .expect("signature");
        UnparsedPublicKey::new(&ED25519, &public_key)
            .verify(&payload, &signature)
            .expect("verifies");
        assert_eq!(
            registry_content_digest(files(&case["files"])).expect("digest"),
            manifest.content_digest,
            "{}",
            case["name"]
        );
    }
}

#[test]
fn an_envelope_signs_only_its_own_kind_subject_and_version() {
    let vectors = vectors();
    let envelopes = vectors["envelopes"].as_array().expect("envelopes");
    for case in vectors["subject_bindings"].as_array().expect("cases") {
        let envelope = envelopes
            .iter()
            .find(|envelope| envelope["name"] == case["envelope"])
            .expect("named envelope");
        let manifest: RegistrySignedManifest =
            serde_json::from_value(envelope["manifest"].clone()).expect("manifest");
        let subject: RegistrySubject = serde_json::from_value(case["subject"].clone()).expect("subject");
        assert_eq!(
            manifest.signs(&subject, case["version"].as_str().expect("version")),
            case["signs"].as_bool().expect("signs"),
            "{}",
            case["name"]
        );
    }
}

#[test]
fn refuses_every_malformed_term_and_any_other_field() {
    let vectors = vectors();
    for case in vectors["invalid_envelopes"].as_array().expect("cases") {
        let manifest: RegistrySignedManifest =
            serde_json::from_value(case["manifest"].clone()).expect("shape");
        assert!(manifest.validate().is_err(), "{}", case["name"]);
    }
    assert!(
        serde_json::from_value::<RegistrySignedManifest>(vectors["unknown_field_envelope"].clone())
            .is_err()
    );
}
