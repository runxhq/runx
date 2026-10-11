use std::cell::RefCell;
use std::path::Path;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ring::signature::KeyPair;
use runx_contracts::sha256_prefixed;
use runx_runtime::registry::{
    AcquireOptions, InstallCandidate, InstallError, InstallLocalSkillOptions, InstallStatus,
    RegistryClient, RegistryClientError, RegistryManifestSignature, RegistryManifestSigner,
    RegistryManifestSourceAuthority, RegistryResolveError, RegistrySignedManifest, TrustTier,
    TrustedRegistryManifestKey, install_local_skill, materialization_cache_path,
    materialization_digest_marker, parse_registry_ref,
};
use runx_runtime::{
    HttpMethod, RuntimeHttpError, RuntimeHttpRequest as HttpRequest,
    RuntimeHttpResponse as HttpResponse, RuntimeHttpTransport as Transport,
};
use serde_json::json;
use tempfile::tempdir;

const TEST_MANIFEST_KEY_ID: &str = "runx-registry-test-key";
const TEST_MANIFEST_SIGNER_ID: &str = "runx-registry-test-signer";
const TEST_MANIFEST_SEED: [u8; 32] = [7; 32];

#[derive(Default)]
struct MockTransport {
    responses: RefCell<Vec<HttpResponse>>,
    requests: RefCell<Vec<HttpRequest>>,
}

impl MockTransport {
    fn with(response: serde_json::Value) -> Self {
        Self {
            responses: RefCell::new(vec![HttpResponse::new(200, response.to_string())]),
            requests: RefCell::new(Vec::new()),
        }
    }

    fn with_status(status: u16, response: serde_json::Value) -> Self {
        Self {
            responses: RefCell::new(vec![HttpResponse::new(status, response.to_string())]),
            requests: RefCell::new(Vec::new()),
        }
    }

    fn with_body(status: u16, body: impl Into<String>) -> Self {
        Self {
            responses: RefCell::new(vec![HttpResponse::new(status, body)]),
            requests: RefCell::new(Vec::new()),
        }
    }

    fn requests(&self) -> Vec<HttpRequest> {
        self.requests.borrow().clone()
    }
}

impl Transport for &MockTransport {
    fn send(&self, request: HttpRequest) -> Result<HttpResponse, RuntimeHttpError> {
        self.requests.borrow_mut().push(request);
        Ok(self.responses.borrow_mut().remove(0))
    }
}

#[test]
fn search_builds_url_and_parses_trust_tier() -> Result<(), Box<dyn std::error::Error>> {
    let transport = MockTransport::with(search_success_fixture()?);
    let client = RegistryClient::with_transport("https://registry.example/", &transport)?;

    let results = client.search(" echo ")?;

    assert_eq!(results[0].trust_tier, TrustTier::Verified);
    assert_eq!(
        transport.requests()[0].url,
        "https://registry.example/v1/skills?q=echo&limit=20"
    );
    assert_eq!(transport.requests()[0].method, HttpMethod::Get);
    Ok(())
}

#[test]
fn search_rejects_unknown_trust_tier_with_field_path() -> Result<(), Box<dyn std::error::Error>> {
    let transport = MockTransport::with(json!({
        "status": "success",
        "skills": [{
            "skill_id": "acme/echo",
            "name": "echo",
            "owner": "acme",
            "profile_mode": "portable",
            "runner_names": [],
            "required_scopes": [],
            "tags": [],
            "trust_tier": "owner_derived",
            "install_command": command(&["runx", "add", "acme/echo@1.0.0"]),
            "run_command": command(&["runx", "skill", "acme/echo@1.0.0"])
        }]
    }));
    let client = RegistryClient::with_transport("https://registry.example", &transport)?;

    let error = match client.search("echo") {
        Ok(_) => return Err("unknown tier should fail".into()),
        Err(error) => error,
    };

    assert!(error.to_string().contains("$.skills[0].trust_tier"));
    Ok(())
}

#[test]
fn search_reports_invalid_json_with_route() -> Result<(), Box<dyn std::error::Error>> {
    let transport = MockTransport::with_body(200, "{not-json");
    let client = RegistryClient::with_transport("https://registry.example", &transport)?;

    let error = match client.search("echo") {
        Ok(_) => return Err("invalid JSON should fail".into()),
        Err(error) => error,
    };

    assert!(matches!(error, RegistryClientError::InvalidJson { .. }));
    assert!(error.to_string().contains("/v1/skills?q=echo&limit=20"));
    Ok(())
}

#[test]
fn client_rejects_unsupported_registry_base_scheme() {
    let transport = MockTransport::default();
    let error = RegistryClient::with_transport("file:///tmp/runx-registry", &transport).err();

    assert!(matches!(
        error,
        Some(RegistryClientError::RuntimeHttp(
            RuntimeHttpError::UnsupportedUrlScheme { .. }
        ))
    ));
}

#[test]
fn read_returns_none_on_404_and_encodes_versioned_suffix() -> Result<(), Box<dyn std::error::Error>>
{
    let transport = MockTransport::with_status(404, json!({ "status": "missing" }));
    let client = RegistryClient::with_transport("https://registry.example", &transport)?;

    let result = client.read("ac me/echo tool", Some("v 1/2"))?;

    assert!(result.is_none());
    assert_eq!(
        transport.requests()[0].url,
        "https://registry.example/v1/skills/ac%20me/echo%20tool%40v%201%2F2"
    );
    Ok(())
}

#[test]
fn dot_path_segments_are_rejected_before_url_construction() -> Result<(), Box<dyn std::error::Error>>
{
    let transport = MockTransport::with_status(404, json!({ "status": "missing" }));
    let client = RegistryClient::with_transport("https://registry.example", &transport)?;

    let result = client.read("../..", Some("../v.1"));

    assert!(matches!(
        result,
        Err(RegistryClientError::InvalidSkillId(_))
    ));
    assert!(transport.requests().is_empty());
    Ok(())
}

#[test]
fn install_error_is_exported_for_callers() {
    fn takes_public_install_error(error: InstallError) -> InstallError {
        error
    }

    let error = InstallError::RunnerMetadataMismatch("echo".to_owned());

    assert!(
        takes_public_install_error(error)
            .to_string()
            .contains("runner manifest")
    );
}

#[test]
fn acquire_requires_installation_id_and_posts_default_channel()
-> Result<(), Box<dyn std::error::Error>> {
    let transport = MockTransport::with(acquire_success_fixture()?);
    let client = RegistryClient::with_transport("https://registry.example", &transport)?;

    assert!(matches!(
        client.acquire(
            "acme/echo",
            AcquireOptions {
                installation_id: "",
                version: None,
                channel: None,
            },
        ),
        Err(RegistryClientError::MissingInstallationId)
    ));
    let acquired = client.acquire(
        "acme/echo",
        AcquireOptions {
            installation_id: "inst_1",
            version: Some("1.0.0"),
            channel: None,
        },
    )?;

    assert_eq!(acquired.install_count, 1);
    assert_eq!(
        acquired.package_digest.as_deref(),
        Some("sha256:fixture-package")
    );
    assert_eq!(acquired.package_files[0].path, "run.mjs");
    assert_eq!(acquired.package_files[0].content, "console.log('echo');\n");
    assert!(transport.requests()[0].body.as_ref().is_some_and(|body| {
        body.contains("\"installation_id\":\"inst_1\"") && body.contains("\"channel\":\"cli\"")
    }));
    assert_eq!(transport.requests()[0].method, HttpMethod::Post);
    assert_eq!(transport.requests()[0].headers[0].name, "content-type");
    Ok(())
}

#[test]
fn bare_ref_resolution_reports_zero_one_and_ambiguous() -> Result<(), Box<dyn std::error::Error>> {
    let zero = MockTransport::with(json!({ "status": "success", "skills": [] }));
    let zero_client = RegistryClient::with_transport("https://registry.example", &zero)?;
    assert_eq!(zero_client.resolve_ref("echo", None)?, None);

    let one = MockTransport::with(json!({
        "status": "success",
        "skills": [search_skill("acme/echo", "echo", "1.0.0")]
    }));
    let one_client = RegistryClient::with_transport("https://registry.example", &one)?;
    assert_eq!(
        one_client
            .resolve_ref("registry:echo", None)?
            .map(|value| value.skill_id),
        Some("acme/echo".to_owned())
    );

    let ambiguous = MockTransport::with(json!({
        "status": "success",
        "skills": [
            search_skill("acme/echo", "echo", "1.0.0"),
            search_skill("runx/echo", "echo", "1.0.0")
        ]
    }));
    let ambiguous_client = RegistryClient::with_transport("https://registry.example", &ambiguous)?;
    assert!(matches!(
        ambiguous_client.resolve_ref("echo", None),
        Err(RegistryResolveError::Ambiguous(_))
    ));
    Ok(())
}

#[test]
fn local_install_is_idempotent_and_rejects_conflicts() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let candidate = install_candidate()?;
    let options = InstallLocalSkillOptions {
        destination_root: temp.path().join("skills"),
        expected_digest: None,
        trusted_manifest_keys: trusted_manifest_keys()?,
    };

    let first = install_local_skill(&candidate, &options)?;
    let second = install_local_skill(&candidate, &options)?;

    assert_eq!(first.status, InstallStatus::Installed);
    assert_eq!(second.status, InstallStatus::Unchanged);
    std::fs::write(&first.destination, "different")?;
    let conflict = match install_local_skill(&candidate, &options) {
        Ok(_) => return Err("conflicting skill should fail".into()),
        Err(error) => error,
    };
    assert!(conflict.to_string().contains("different content"));

    std::fs::write(&first.destination, &candidate.markdown)?;
    std::fs::write(
        first.destination.with_file_name("run.yaml"),
        "skill: other\n",
    )?;
    let manifest_conflict = match install_local_skill(&candidate, &options) {
        Ok(_) => return Err("conflicting runner manifest should fail".into()),
        Err(error) => error,
    };
    assert!(manifest_conflict.to_string().contains("runner manifest"));
    Ok(())
}

#[test]
fn local_install_accepts_signed_registry_manifest() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let candidate = install_candidate()?;
    let options = InstallLocalSkillOptions {
        destination_root: temp.path().join("skills"),
        expected_digest: Some(skill_digest()),
        trusted_manifest_keys: trusted_manifest_keys()?,
    };

    let install = install_local_skill(&candidate, &options)?;

    assert_eq!(install.status, InstallStatus::Installed);
    assert_eq!(install.skill_id.as_deref(), Some("acme/echo"));
    assert!(install.digest.starts_with("sha256:"));
    Ok(())
}

#[test]
fn non_local_install_requires_signed_registry_manifest() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let mut candidate = install_candidate()?;
    candidate.signed_manifest = None;
    let options = InstallLocalSkillOptions {
        destination_root: temp.path().join("skills"),
        expected_digest: None,
        trusted_manifest_keys: trusted_manifest_keys()?,
    };

    let error = match install_local_skill(&candidate, &options) {
        Ok(_) => return Err("unsigned install should fail".into()),
        Err(error) => error,
    };

    assert!(matches!(error, InstallError::UnsignedManifest(_)));
    Ok(())
}

#[test]
fn local_install_rejects_bad_expected_digest() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let candidate = install_candidate()?;
    let options = InstallLocalSkillOptions {
        destination_root: temp.path().join("skills"),
        expected_digest: Some("sha256:wrong".to_owned()),
        trusted_manifest_keys: trusted_manifest_keys()?,
    };

    let error = match install_local_skill(&candidate, &options) {
        Ok(_) => return Err("expected digest mismatch should fail".into()),
        Err(error) => error,
    };

    assert!(error.to_string().contains("digest mismatch"));
    assert!(!options.destination_root.exists());
    Ok(())
}

#[test]
fn profile_digest_mismatch_leaves_no_partial_install() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let mut candidate = install_candidate()?;
    candidate.signed_manifest = Some(signed_manifest(
        "acme/echo",
        "1.0.0",
        &skill_digest(),
        Some("sha256:wrong"),
        None,
    )?);
    let options = InstallLocalSkillOptions {
        destination_root: temp.path().join("skills"),
        expected_digest: None,
        trusted_manifest_keys: trusted_manifest_keys()?,
    };

    let error = match install_local_skill(&candidate, &options) {
        Ok(_) => return Err("profile digest mismatch should fail".into()),
        Err(error) => error,
    };

    assert!(error.to_string().contains("binding digest mismatch"));
    assert!(!options.destination_root.exists());
    Ok(())
}

#[test]
fn ref_helpers_parse_cache_and_package_paths() {
    let parsed = parse_registry_ref("runx://skill/acme%2Fecho%401.0.0");
    assert_eq!(parsed.skill_id, "acme/echo");
    assert_eq!(parsed.version.as_deref(), Some("1.0.0"));
    assert_eq!(
        parse_registry_ref("runx://skill/acme%2Fecho+tool").skill_id,
        "acme/echo+tool"
    );

    assert_eq!(
        materialization_cache_path(
            Path::new("/tmp/cache"),
            "Acme",
            "Echo Tool",
            "1.0.0",
            "sha256:1234567890abcdef9999"
        ),
        Path::new("/tmp/cache")
            .join("acme")
            .join("echo-tool")
            .join("1.0.0")
            .join("1234567890abcdef")
    );
    assert_eq!(
        materialization_digest_marker("sha256:abc", Some("sha256:def"), None),
        "digest=sha256:abc\nprofile_digest=sha256:def\npackage_digest=\n"
    );
}

fn search_skill(skill_id: &str, name: &str, version: &str) -> serde_json::Value {
    json!({
        "skill_id": skill_id,
        "name": name,
        "owner": skill_id.split('/').next().unwrap_or("acme"),
        "version": version,
        "profile_mode": "portable",
        "runner_names": [],
        "required_scopes": [],
        "tags": [],
        "trust_tier": "community",
        "install_command": command(&["runx", "add", &format!("{skill_id}@{version}")]),
        "run_command": command(&["runx", "skill", &format!("{skill_id}@{version}")])
    })
}

/// A copyable command as the registry serves it. Fixture words are all bare,
/// so the rendering is the words joined; the reader checks it against `argv`.
fn command(argv: &[&str]) -> serde_json::Value {
    json!({ "shell": "posix", "argv": argv, "copy_text": argv.join(" ") })
}

fn install_candidate() -> Result<InstallCandidate, Box<dyn std::error::Error>> {
    Ok(InstallCandidate {
        markdown: include_str!("../../../fixtures/registry/install/echo-SKILL.md").to_owned(),
        profile_document: Some(
            include_str!("../../../fixtures/registry/install/echo-run.yaml").to_owned(),
        ),
        package_files: Vec::new(),
        package_digest: None,
        source: "runx-registry".to_owned(),
        source_label: "runx registry".to_owned(),
        r#ref: "acme/echo@1.0.0".to_owned(),
        skill_id: Some("acme/echo".to_owned()),
        version: Some("1.0.0".to_owned()),
        signed_manifest: Some(signed_manifest(
            "acme/echo",
            "1.0.0",
            &skill_digest(),
            Some(&profile_digest()),
            None,
        )?),
        profile_digest: None,
        runner_names: vec!["default".to_owned()],
        trust_tier: Some(TrustTier::Community),
        manifest_source_authority: Some(RegistryManifestSourceAuthority::RegistrySource(
            "test-registry".to_owned(),
        )),
    })
}

fn skill_digest() -> String {
    sha256_prefixed(include_str!("../../../fixtures/registry/install/echo-SKILL.md").as_bytes())
}

fn profile_digest() -> String {
    sha256_prefixed(include_str!("../../../fixtures/registry/install/echo-run.yaml").as_bytes())
}

fn trusted_manifest_keys() -> Result<Vec<TrustedRegistryManifestKey>, Box<dyn std::error::Error>> {
    let public_key = manifest_public_key_base64()?;
    Ok(vec![TrustedRegistryManifestKey::from_base64(
        TEST_MANIFEST_KEY_ID.to_owned(),
        &public_key,
        "acme".to_owned(),
        "test-registry".to_owned(),
    )?])
}

fn manifest_public_key_base64() -> Result<String, Box<dyn std::error::Error>> {
    Ok(base64::engine::general_purpose::STANDARD
        .encode(test_manifest_key_pair()?.public_key().as_ref()))
}

fn signed_manifest(
    skill_id: &str,
    version: &str,
    digest: &str,
    profile_digest: Option<&str>,
    package_digest: Option<&str>,
) -> Result<RegistrySignedManifest, Box<dyn std::error::Error>> {
    let payload =
        registry_manifest_payload(skill_id, version, digest, profile_digest, package_digest);
    let signature = test_manifest_key_pair()?.sign(payload.as_bytes());
    Ok(RegistrySignedManifest {
        schema: runx_runtime::registry::REGISTRY_SIGNED_MANIFEST_SCHEMA.to_owned(),
        skill_id: skill_id.to_owned(),
        version: version.to_owned(),
        digest: digest.to_owned(),
        profile_digest: profile_digest.map(str::to_owned),
        package_digest: package_digest.map(str::to_owned),
        signer: RegistryManifestSigner {
            id: TEST_MANIFEST_SIGNER_ID.to_owned(),
            key_id: TEST_MANIFEST_KEY_ID.to_owned(),
        },
        signature: RegistryManifestSignature {
            alg: "ed25519".to_owned(),
            value: format!("base64:{}", URL_SAFE_NO_PAD.encode(signature.as_ref())),
        },
    })
}

fn registry_manifest_payload(
    skill_id: &str,
    version: &str,
    digest: &str,
    profile_digest: Option<&str>,
    package_digest: Option<&str>,
) -> String {
    format!(
        "{}\nskill_id={skill_id}\nversion={version}\ndigest={digest}\nprofile_digest={}\npackage_digest={}\nsigner_id={TEST_MANIFEST_SIGNER_ID}\nkey_id={TEST_MANIFEST_KEY_ID}\n",
        runx_runtime::registry::REGISTRY_SIGNED_MANIFEST_SCHEMA,
        profile_digest.unwrap_or(""),
        package_digest.unwrap_or("")
    )
}

fn test_manifest_key_pair() -> Result<ring::signature::Ed25519KeyPair, std::io::Error> {
    ring::signature::Ed25519KeyPair::from_seed_unchecked(&TEST_MANIFEST_SEED).map_err(|error| {
        std::io::Error::other(format!("static registry manifest seed rejected: {error:?}"))
    })
}

fn search_success_fixture() -> Result<serde_json::Value, serde_json::Error> {
    serde_json::from_str(include_str!(
        "../../../fixtures/registry/remote/search-success.json"
    ))
}

fn acquire_success_fixture() -> Result<serde_json::Value, serde_json::Error> {
    serde_json::from_str(include_str!(
        "../../../fixtures/registry/remote/acquire-success.json"
    ))
}
