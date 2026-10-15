use std::cell::RefCell;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ring::signature::KeyPair;
use runx_runtime::registry::{
    AcquireOptions, InstallCandidate, InstallError, InstallLocalSkillOptions, InstallStatus,
    RegistryClient, RegistryClientError, RegistryManifestSignature, RegistryManifestSigner,
    RegistryManifestSourceAuthority, RegistrySignedManifest, RegistrySubject, RegistrySubjectKind,
    TrustTier, TrustedRegistryManifestKey, install_local_skill, parse_registry_ref,
    skill_content_digest,
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
fn a_root_name_reads_as_its_canonical_skill() -> Result<(), Box<dyn std::error::Error>> {
    let held = MockTransport::with(json!({
        "status": "success",
        "skill": skill_detail("runx/web-search", "1.2.0")
    }));
    let client = RegistryClient::with_transport("https://registry.example", &held)?;
    let detail = client
        .read_root("web-search", Some("1.2.0"))?
        .ok_or("the root name should resolve")?;
    assert_eq!(detail.skill_id, "runx/web-search");
    assert_eq!(
        held.requests()[0].url,
        "https://registry.example/v1/skills/web-search%401.2.0"
    );

    let unheld = MockTransport::with_status(404, json!({ "status": "missing" }));
    let client = RegistryClient::with_transport("https://registry.example", &unheld)?;
    assert!(client.read_root("echo", None)?.is_none());

    // An answer that is not a canonical id, or not the version asked for, is refused.
    let moved = MockTransport::with(json!({
        "status": "success",
        "skill": skill_detail("runx/web-search", "2.0.0")
    }));
    let client = RegistryClient::with_transport("https://registry.example", &moved)?;
    assert!(matches!(
        client.read_root("web-search", Some("1.2.0")),
        Err(RegistryClientError::Contract { .. })
    ));

    let none = MockTransport::default();
    let client = RegistryClient::with_transport("https://registry.example", &none)?;
    assert!(matches!(
        client.read_root("acme/echo", None),
        Err(RegistryClientError::InvalidRootName(_))
    ));
    assert!(none.requests().is_empty());
    Ok(())
}

#[test]
fn local_install_is_idempotent_and_rejects_conflicts() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let candidate = install_candidate()?;
    let options = InstallLocalSkillOptions {
        destination_root: temp.path().join("skills"),
        connectors_root: temp.path().join("connectors"),
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
        connectors_root: temp.path().join("connectors"),
        // A pinned digest is the content digest the registry signs.
        expected_digest: Some(candidate_content_digest(&candidate)?),
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
        connectors_root: temp.path().join("connectors"),
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
        connectors_root: temp.path().join("connectors"),
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
fn a_profile_changed_after_signing_leaves_no_partial_install()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let mut candidate = install_candidate()?;
    candidate.profile_document = Some(format!(
        "{}\n# changed\n",
        candidate.profile_document.unwrap_or_default()
    ));
    let options = InstallLocalSkillOptions {
        destination_root: temp.path().join("skills"),
        connectors_root: temp.path().join("connectors"),
        expected_digest: None,
        trusted_manifest_keys: trusted_manifest_keys()?,
    };

    let error = match install_local_skill(&candidate, &options) {
        Ok(_) => return Err("a changed profile should fail".into()),
        Err(error) => error,
    };

    // The envelope signs every file: a changed profile is a content mismatch.
    assert!(
        matches!(error, InstallError::DigestMismatch { .. }),
        "{error}"
    );
    assert!(!options.destination_root.exists());
    Ok(())
}

#[test]
fn ref_helpers_parse_cache_and_package_paths() {
    let parsed = parse_registry_ref("registry:acme/echo@1.0.0");
    assert_eq!(parsed.skill_id, "acme/echo");
    assert_eq!(parsed.version.as_deref(), Some("1.0.0"));
    // There is one spelling: another prefix is not a registry ref.
    assert_eq!(
        parse_registry_ref("runx-registry:acme/echo").skill_id,
        "runx-registry:acme/echo"
    );
}

fn skill_detail(skill_id: &str, version: &str) -> serde_json::Value {
    let (owner, name) = skill_id.split_once('/').unwrap_or(("acme", skill_id));
    json!({
        "skill_id": skill_id,
        "owner": owner,
        "name": name,
        "version": version,
        "digest": "sha256:fixture",
        "markdown": format!("---\nname: {name}\n---\n"),
        "runner_names": [],
        "trust_tier": "verified",
        "required_scopes": [],
        "tags": [],
        "publisher": { "kind": "organization", "id": format!("pub_{owner}"), "handle": owner },
        "attestations": [],
        "install_command": command(&["runx", "registry", "install", &format!("{skill_id}@{version}")]),
        "run_command": command(&["runx", "skill", &format!("{skill_id}@{version}")])
    })
}

/// A copyable command as the registry serves it. Fixture words are all bare,
/// so the rendering is the words joined; the reader checks it against `argv`.
fn command(argv: &[&str]) -> serde_json::Value {
    json!({ "shell": "posix", "argv": argv, "copy_text": argv.join(" ") })
}

fn install_candidate() -> Result<InstallCandidate, Box<dyn std::error::Error>> {
    let mut candidate = InstallCandidate {
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
        signed_manifest: None,
        profile_digest: None,
        runner_names: vec!["default".to_owned()],
        trust_tier: Some(TrustTier::Community),
        manifest_source_authority: Some(RegistryManifestSourceAuthority::RegistrySource(
            "test-registry".to_owned(),
        )),
    };
    candidate.signed_manifest = Some(signed_manifest(
        "acme/echo",
        "1.0.0",
        &candidate_content_digest(&candidate)?,
    )?);
    Ok(candidate)
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

/// The envelope the registry serves for a candidate's exact files, signed by the test key.
fn signed_manifest(
    skill_id: &str,
    version: &str,
    content_digest: &str,
) -> Result<RegistrySignedManifest, Box<dyn std::error::Error>> {
    let mut manifest = RegistrySignedManifest {
        schema: runx_runtime::registry::REGISTRY_SIGNED_MANIFEST_SCHEMA.to_owned(),
        subject: RegistrySubject {
            kind: RegistrySubjectKind::Skill,
            subject_ref: skill_id.to_owned(),
        },
        version: version.to_owned(),
        content_digest: content_digest.to_owned(),
        signer: RegistryManifestSigner {
            id: TEST_MANIFEST_SIGNER_ID.to_owned(),
            key_id: TEST_MANIFEST_KEY_ID.to_owned(),
        },
        signature: RegistryManifestSignature {
            alg: "ed25519".to_owned(),
            value: "base64:".to_owned(),
        },
    };
    let signature = test_manifest_key_pair()?.sign(&manifest.signing_payload()?);
    manifest.signature.value = format!("base64:{}", URL_SAFE_NO_PAD.encode(signature.as_ref()));
    Ok(manifest)
}

fn candidate_content_digest(
    candidate: &InstallCandidate,
) -> Result<String, Box<dyn std::error::Error>> {
    Ok(skill_content_digest(
        &candidate.markdown,
        candidate.profile_document.as_deref(),
        &candidate.package_files,
    )?)
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

#[test]
fn connector_reads_name_the_qualified_ref_and_version() -> Result<(), Box<dyn std::error::Error>> {
    let detail = json!({
        "status": "success",
        "connector": {
            "ref": "acme/ops-api",
            "version": "1.0.0",
            "name": "Ops API",
            "description": "Reads ops items.",
            "content_digest": format!("sha256:{}", "a".repeat(64)),
            "manifest_digest": format!("sha256:{}", "b".repeat(64)),
            "connector_digest": format!("sha256:{}", "c".repeat(64)),
            "runtime_capability": false,
            "trust_tier": "community",
            "operations": [{ "name": "item.read", "description": "Read an item." }],
            "install_command": command(&["runx", "registry", "install", "acme/ops-api@1.0.0"]),
            "placements": []
        }
    });
    let transport = MockTransport {
        responses: RefCell::new(vec![
            HttpResponse::new(200, detail.to_string()),
            HttpResponse::new(200, detail.to_string()),
            HttpResponse::new(404, "{}"),
        ]),
        requests: RefCell::new(Vec::new()),
    };
    let client = RegistryClient::with_transport("https://registry.example", &transport)?;
    let connector_ref =
        runx_contracts::connector::ConnectorName::new("acme/ops-api").ok_or("connector ref")?;

    let current = client
        .read_connector(&connector_ref, None)?
        .ok_or("current version")?;
    let pinned = client
        .read_connector(&connector_ref, Some("1.0.0"))?
        .ok_or("pinned version")?;
    let missing = client.read_connector(&connector_ref, Some("2.0.0"))?;

    assert_eq!(current.connector, connector_ref);
    assert_eq!(pinned.operations[0].name.as_str(), "item.read");
    assert!(missing.is_none());
    let urls = transport
        .requests()
        .into_iter()
        .map(|request| request.url)
        .collect::<Vec<_>>();
    assert_eq!(
        urls,
        [
            "https://registry.example/v1/connectors/acme/ops-api",
            "https://registry.example/v1/connectors/acme/ops-api/versions/1.0.0",
            "https://registry.example/v1/connectors/acme/ops-api/versions/2.0.0",
        ]
    );
    Ok(())
}

#[test]
fn a_connector_package_is_exactly_its_files_and_signed_manifest()
-> Result<(), Box<dyn std::error::Error>> {
    let manifest = signed_manifest(
        "acme/ops-api",
        "1.0.0",
        &format!("sha256:{}", "a".repeat(64)),
    )?;
    let mut manifest = serde_json::to_value(manifest)?;
    manifest["subject"]["kind"] = json!("connector");
    let package = json!({
        "ref": "acme/ops-api",
        "version": "1.0.0",
        "trust_tier": "community",
        "files": [{ "path": "connector.yaml", "content": "schema: runx.connector.v1\n" }],
        "signed_manifest": manifest
    });
    let transport = MockTransport::with(json!({ "status": "success", "package": package }));
    let client = RegistryClient::with_transport("https://registry.example", &transport)?;
    let connector_ref =
        runx_contracts::connector::ConnectorName::new("acme/ops-api").ok_or("connector ref")?;

    let published = client
        .connector_package(&connector_ref, "1.0.0")?
        .ok_or("package")?;

    assert_eq!(published.files[0].path, "connector.yaml");
    assert_eq!(
        published.signed_manifest.subject.kind,
        RegistrySubjectKind::Connector
    );
    assert_eq!(
        transport.requests()[0].url,
        "https://registry.example/v1/connectors/acme/ops-api/versions/1.0.0/package"
    );

    let mut extra = package;
    extra["manifest_digest"] = json!(format!("sha256:{}", "b".repeat(64)));
    let transport = MockTransport::with(json!({ "status": "success", "package": extra }));
    let client = RegistryClient::with_transport("https://registry.example", &transport)?;
    let error = match client.connector_package(&connector_ref, "1.0.0") {
        Ok(_) => return Err("a package with fields beyond its files must fail".into()),
        Err(error) => error,
    };
    assert!(error.to_string().contains("$.package"), "{error}");
    Ok(())
}
