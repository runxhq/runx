//! A hosted registry on loopback, for OSS tests: one stateful service that
//! speaks the hosted registry's wire contract (the routes
//! `registry/http.rs` reads and `registry/publish.rs` writes, an operator's
//! publish included), keeps what is
//! published, and signs each version with a fixture key the runtime is told
//! to trust through `RUNX_REGISTRY_KEYS`, scoped to one publisher from this
//! registry, and states each version's tier in a signed tier statement
//! served beside its manifest. Cloud's own contract test holds the service
//! side.
//!
//! Each connection is served on its own thread and read to the end of its
//! request, with no timeout: a client that stalls stalls only itself.
//!
//! The runtime and CLI test binaries both compile this file; each uses its
//! own part of it.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use ring::signature::KeyPair;
use runx_contracts::{
    CopyableCommand, REGISTRY_TIER_STATEMENT_SCHEMA, RegistryManifestSignature,
    RegistryManifestSigner, RegistrySignedManifest, RegistrySubject, RegistrySubjectKind,
    RegistryTierStatement, TrustTier, sha256_prefixed,
};
use runx_runtime::registry::{RUNX_REGISTRY_KEYS_ENV, RegistryPackageFile};
use serde_json::{Value, json};

type Error = Box<dyn std::error::Error>;

/// The key id the fixture signs every version with.
pub const KEY_ID: &str = "runx-registry-fixture-key";
const SIGNER_ID: &str = "runx-registry-fixture";
const SEED: [u8; 32] = [7; 32];
/// The namespace the runtime trusts the fixture key to sign, unless a test
/// names another.
pub const PUBLISHER: &str = "acme";
/// The principal `/v1/me` names for a publisher's token.
pub const PRINCIPAL_ID: &str = "rxp_fixture_publisher";
/// The token a publish authenticates with.
pub const TOKEN: &str = "rxk_fixture_publisher";
/// When a statement the fixture issues holds: from before any test runs
/// until long after.
pub const FRESH: (&str, &str) = ("2026-11-01T00:00:00Z", "2099-01-01T00:00:00Z");
/// A statement that held once and has expired.
pub const EXPIRED: (&str, &str) = ("2026-01-01T00:00:00Z", "2026-02-01T00:00:00Z");

/// One version to publish: a package's documents, under a publisher, at a
/// version and the tier the registry states for it, and when that statement
/// holds; none serves no statement, as for a withdrawn version.
#[derive(Clone, Debug)]
pub struct Publication<'a> {
    pub publisher: &'a str,
    pub version: &'a str,
    pub trust_tier: TrustTier,
    pub tier_held: Option<(&'a str, &'a str)>,
    pub markdown: &'a str,
    pub profile: Option<&'a str>,
    pub files: Vec<RegistryPackageFile>,
}

impl<'a> Publication<'a> {
    /// `markdown` and `profile`, published under `publisher` at `version` as
    /// a community package.
    #[must_use]
    pub fn new(
        publisher: &'a str,
        version: &'a str,
        markdown: &'a str,
        profile: Option<&'a str>,
    ) -> Self {
        Self {
            publisher,
            version,
            trust_tier: TrustTier::Community,
            tier_held: Some(FRESH),
            markdown,
            profile,
            files: Vec::new(),
        }
    }
}

pub struct LoopbackRegistry {
    pub url: String,
    state: Arc<Mutex<State>>,
}

#[derive(Default)]
struct State {
    /// Every version of each skill, oldest first; the last is current.
    skills: BTreeMap<String, Vec<Value>>,
    /// Root names, each the canonical skill it addresses.
    roots: BTreeMap<String, String>,
    /// Every version of each connector, oldest first.
    connectors: BTreeMap<String, Vec<StoredConnector>>,
    /// How many versions a page of versions lists.
    page_size: usize,
    /// Each request line, as asked.
    asked: Vec<String>,
    installs: u64,
}

#[derive(Clone)]
struct StoredConnector {
    detail: Value,
    package: Value,
}

impl LoopbackRegistry {
    /// Serve an empty registry on loopback.
    ///
    /// # Errors
    ///
    /// Returns an error when no loopback port can be bound.
    pub fn serve() -> Result<Self, Error> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let url = format!("http://{}", listener.local_addr()?);
        let state = Arc::new(Mutex::new(State {
            page_size: 50,
            ..State::default()
        }));
        let served = Arc::clone(&state);
        let base = url.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let state = Arc::clone(&served);
                let base = base.clone();
                std::thread::spawn(move || serve_connection(stream, &state, &base));
            }
        });
        Ok(Self { url, state })
    }

    /// List versions `page_size` to a page.
    pub fn page_size(&self, page_size: usize) {
        self.state().page_size = page_size.max(1);
    }

    /// The environment a runtime reaching this registry runs with: the
    /// fixture key, trusted to sign `PUBLISHER`'s namespace from this
    /// registry, and leave to reach loopback.
    pub fn env(&self) -> Result<BTreeMap<String, String>, Error> {
        self.env_trusting(PUBLISHER)
    }

    /// As [`Self::env`], with the fixture key trusted for `publisher`.
    pub fn env_trusting(&self, publisher: &str) -> Result<BTreeMap<String, String>, Error> {
        Ok(BTreeMap::from([
            (
                RUNX_REGISTRY_KEYS_ENV.to_owned(),
                self.registry_keys(publisher)?,
            ),
            (
                "RUNX_PUBLIC_API_ALLOW_PRIVATE_NETWORK".to_owned(),
                "1".to_owned(),
            ),
        ]))
    }

    /// The fixture key as `RUNX_REGISTRY_KEYS` names it: trusted to sign
    /// `publisher`'s namespace from this registry.
    pub fn registry_keys(&self, publisher: &str) -> Result<String, Error> {
        Ok(json!([{
            "key_id": KEY_ID,
            "public_key": public_key_base64()?,
            "publisher": publisher,
            "source": self.url,
        }])
        .to_string())
    }

    /// As [`Self::env`], with the token a publisher authenticates with.
    pub fn publisher_env(&self) -> Result<BTreeMap<String, String>, Error> {
        let mut env = self.env()?;
        env.insert("RUNX_PUBLIC_API_TOKEN".to_owned(), TOKEN.to_owned());
        Ok(env)
    }

    /// Publish a version, signed, as the registry records a publish.
    ///
    /// # Errors
    ///
    /// Returns an error when the package does not validate or the version is
    /// already published with other content.
    pub fn publish(&self, publication: Publication<'_>) -> Result<Value, Error> {
        let record = build_record(&publication)?;
        self.state().put(record.clone())?;
        Ok(record)
    }

    /// Publish the package in `directory` (its `SKILL.md`, its `run.yaml`
    /// when it has one, and no other file) under `publisher` at `version`.
    pub fn publish_directory(
        &self,
        directory: &Path,
        publisher: &str,
        version: &str,
    ) -> Result<Value, Error> {
        let markdown = std::fs::read_to_string(directory.join("SKILL.md"))?;
        let profile = std::fs::read_to_string(directory.join("run.yaml")).ok();
        self.publish(Publication::new(
            publisher,
            version,
            &markdown,
            profile.as_deref(),
        ))
    }

    /// Address `skill_id` by the root name `root`.
    pub fn root(&self, root: &str, skill_id: &str) {
        self.state()
            .roots
            .insert(root.to_owned(), skill_id.to_owned());
    }

    /// Change what the registry serves for `skill_id@version`, as a faulty or
    /// hostile registry would.
    pub fn tamper(
        &self,
        skill_id: &str,
        version: &str,
        change: impl FnOnce(&mut Value),
    ) -> Result<(), Error> {
        let mut state = self.state();
        let record = state
            .skills
            .get_mut(skill_id)
            .and_then(|versions| {
                versions
                    .iter_mut()
                    .find(|record| record["version"] == version)
            })
            .ok_or_else(|| format!("the registry holds no {skill_id}@{version}"))?;
        change(record);
        Ok(())
    }

    /// Publish the connector in `directory` at `version`, signed.
    pub fn publish_connector(
        &self,
        directory: &Path,
        version: &str,
        trust_tier: TrustTier,
    ) -> Result<Value, Error> {
        let package = runx_runtime::registry::prepare_registry_connector_package(directory)?;
        let name = package.connector.as_str().to_owned();
        let signed_manifest = sign(
            RegistrySubjectKind::Connector,
            &name,
            version,
            package.content_digest.as_str().to_owned(),
        )?;
        // The fixture key signs a connector in its name's namespace, so that
        // namespace published it.
        let publisher = name
            .split_once('/')
            .map_or(name.as_str(), |(namespace, _)| namespace);
        let tier_statement = state(
            RegistrySubjectKind::Connector,
            &name,
            version,
            package.content_digest.as_str(),
            publisher,
            trust_tier,
            FRESH,
        )?;
        let install_command =
            command(&["runx", "registry", "install", &format!("{name}@{version}")])?;
        let detail = json!({
            "connector": name,
            "version": version,
            "name": package.manifest.title,
            "description": package.manifest.description,
            "content_digest": package.content_digest,
            "manifest_digest": package.manifest_digest,
            "connector_digest": package.connector_digest,
            // The runtime's own connectors are the ones it implements whole.
            "runtime_capability": package.manifest.operations.values().all(|operation| matches!(
                operation.implementation,
                Some(
                    runx_contracts::connector::Implementation::Web(_)
                        | runx_contracts::connector::Implementation::Store(_)
                        | runx_contracts::connector::Implementation::Artifact(_)
                )
            )),
            "trust_tier": trust_tier,
            "operations": package.manifest.operations.iter().map(|(operation, declared)| json!({
                "name": operation,
                "description": declared.description,
            })).collect::<Vec<_>>(),
            "install_command": install_command,
        });
        let stored = StoredConnector {
            package: json!({
                "connector": name,
                "version": version,
                "trust_tier": trust_tier,
                "files": package.files,
                "signed_manifest": signed_manifest,
                "tier_statement": tier_statement,
            }),
            detail: detail.clone(),
        };
        self.state()
            .connectors
            .entry(name)
            .or_default()
            .push(stored);
        Ok(detail)
    }

    /// Each request line the registry was asked, in order.
    #[must_use]
    pub fn asked(&self) -> Vec<String> {
        self.state().asked.clone()
    }

    fn state(&self) -> MutexGuard<'_, State> {
        lock(&self.state)
    }
}

fn lock(state: &Mutex<State>) -> MutexGuard<'_, State> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

impl State {
    /// Record a version. Versions are immutable: the same content again is
    /// unchanged, other content under a published version is refused.
    fn put(&mut self, record: Value) -> Result<Publish, Error> {
        let skill_id = text(&record, "skill_id")?.to_owned();
        let version = text(&record, "version")?.to_owned();
        let versions = self.skills.entry(skill_id.clone()).or_default();
        if let Some(existing) = versions.iter().find(|stored| stored["version"] == version) {
            return if content_digest(existing)? == content_digest(&record)? {
                Ok(Publish::Unchanged)
            } else {
                Err(format!("{skill_id}@{version} is already published with other content").into())
            };
        }
        versions.push(record);
        Ok(Publish::Published)
    }

    fn version(&self, skill_id: &str, version: Option<&str>) -> Option<&Value> {
        let versions = self.skills.get(skill_id)?;
        match version {
            Some(version) => versions.iter().find(|record| record["version"] == version),
            None => versions.last(),
        }
    }
}

enum Publish {
    Published,
    Unchanged,
}

/// The record the registry builds for a publication: the service's fields,
/// signed over the version's content. Its name and methods are read from the
/// manual and profile alone, so a version carrying files a newer runx reads
/// still publishes, as the service would hold it.
fn build_record(publication: &Publication<'_>) -> Result<Value, Error> {
    let package =
        runx_parser::validate_skill_package(runx_parser::SkillPackageSource::from_documents(
            publication.markdown,
            publication.profile.map(str::to_owned),
        ))?;
    let name = package.skill.name.clone();
    let skill_id = format!("{}/{name}", publication.publisher);
    let mut files = publication.files.clone();
    files.sort_by(|left, right| left.path.cmp(&right.path));
    let content_digest = runx_runtime::registry::skill_content_digest(
        publication.markdown,
        publication.profile,
        &files,
    )?;
    let reference = format!("{skill_id}@{}", publication.version);
    let tier_statement = publication
        .tier_held
        .map(|held| {
            state(
                RegistrySubjectKind::Skill,
                &skill_id,
                publication.version,
                &content_digest,
                publication.publisher,
                publication.trust_tier,
                held,
            )
        })
        .transpose()?;
    Ok(json!({
        "skill_id": skill_id,
        "name": name,
        "description": package.skill.description,
        "version": publication.version,
        "digest": sha256_prefixed(publication.markdown.as_bytes()),
        "signed_manifest": sign(
            RegistrySubjectKind::Skill,
            &skill_id,
            publication.version,
            content_digest.clone(),
        )?,
        "tier_statement": tier_statement,
        "markdown": publication.markdown,
        "profile_document": publication.profile,
        "profile_digest": publication.profile.map(|profile| sha256_prefixed(profile.as_bytes())),
        "package_files": files,
        "bundle_digest": bundle_digest(&files)?,
        "method_names": package
            .root_manifest()
            .map(|manifest| manifest.methods.keys().cloned().collect::<Vec<_>>())
            .unwrap_or_default(),
        "trust_tier": publication.trust_tier,
        "tags": [],
        "publisher": {
            "kind": "organization",
            "id": format!("pub_{}", publication.publisher),
            "handle": publication.publisher,
        },
        "install_command": command(&["runx", "registry", "install", &reference])?,
        "run_command": command(&["runx", "skill", &reference])?,
        "created_at": "2026-11-08T00:00:00Z",
        "updated_at": "2026-11-08T00:00:00Z",
    }))
}

/// The digest the registry's record states over a version's package files,
/// none when it has none.
///
/// # Errors
///
/// Returns an error when a file does not encode.
pub fn bundle_digest(files: &[RegistryPackageFile]) -> Result<Option<String>, Error> {
    if files.is_empty() {
        return Ok(None);
    }
    let mut sorted = files.iter().collect::<Vec<_>>();
    sorted.sort_by(|left, right| left.path.cmp(&right.path));
    let document = json!({
        "files": sorted
            .iter()
            .map(|file| json!({"content": file.content, "path": file.path}))
            .collect::<Vec<_>>(),
    });
    Ok(Some(sha256_prefixed(&serde_json::to_vec(&document)?)))
}

/// The envelope the registry serves for one subject's version and content,
/// signed with the fixture key.
///
/// # Errors
///
/// Returns an error when the envelope does not encode.
pub fn sign(
    kind: RegistrySubjectKind,
    name: &str,
    version: &str,
    content_digest: String,
) -> Result<RegistrySignedManifest, Error> {
    let mut manifest = RegistrySignedManifest {
        schema: runx_runtime::registry::REGISTRY_SIGNED_MANIFEST_SCHEMA.to_owned(),
        subject: RegistrySubject {
            kind,
            name: name.to_owned(),
        },
        version: version.to_owned(),
        content_digest,
        signer: RegistryManifestSigner {
            id: SIGNER_ID.to_owned(),
            key_id: KEY_ID.to_owned(),
        },
        signature: RegistryManifestSignature {
            alg: "ed25519".to_owned(),
            value: "base64:".to_owned(),
        },
    };
    let signature = key_pair()?.sign(&manifest.signing_payload()?);
    manifest.signature.value = format!("base64:{}", URL_SAFE_NO_PAD.encode(signature.as_ref()));
    Ok(manifest)
}

/// The tier statement the registry serves for one subject's version and
/// content, naming `publisher` as the account that published it, held from
/// `held.0` until `held.1`, signed with the fixture key.
///
/// # Errors
///
/// Returns an error when the statement does not encode.
pub fn state(
    kind: RegistrySubjectKind,
    name: &str,
    version: &str,
    content_digest: &str,
    publisher: &str,
    tier: TrustTier,
    held: (&str, &str),
) -> Result<RegistryTierStatement, Error> {
    let mut statement = RegistryTierStatement {
        schema: REGISTRY_TIER_STATEMENT_SCHEMA.to_owned(),
        subject: RegistrySubject {
            kind,
            name: name.to_owned(),
        },
        version: version.to_owned(),
        content_digest: content_digest.to_owned(),
        publisher: publisher.to_owned(),
        tier,
        issued_at: held.0.to_owned(),
        expires_at: held.1.to_owned(),
        signer: RegistryManifestSigner {
            id: SIGNER_ID.to_owned(),
            key_id: KEY_ID.to_owned(),
        },
        signature: RegistryManifestSignature {
            alg: "ed25519".to_owned(),
            value: "base64:".to_owned(),
        },
    };
    let signature = key_pair()?.sign(&statement.signing_payload()?);
    statement.signature.value = format!("base64:{}", URL_SAFE_NO_PAD.encode(signature.as_ref()));
    Ok(statement)
}

/// The fixture key's public half, as the trust environment names it.
///
/// # Errors
///
/// Returns an error when the fixture seed does not make a key.
pub fn public_key_base64() -> Result<String, Error> {
    Ok(STANDARD.encode(key_pair()?.public_key().as_ref()))
}

fn key_pair() -> Result<ring::signature::Ed25519KeyPair, Error> {
    ring::signature::Ed25519KeyPair::from_seed_unchecked(&SEED)
        .map_err(|error| format!("the fixture seed made no key: {error:?}").into())
}

fn command(argv: &[&str]) -> Result<CopyableCommand, Error> {
    Ok(CopyableCommand::posix(
        argv.iter().map(|word| (*word).to_owned()).collect(),
    )?)
}

fn text<'a>(value: &'a Value, field: &str) -> Result<&'a str, Error> {
    value[field]
        .as_str()
        .ok_or_else(|| format!("the record has no {field}").into())
}

fn content_digest(record: &Value) -> Result<String, Error> {
    let files: Vec<RegistryPackageFile> =
        serde_json::from_value(record["package_files"].clone()).unwrap_or_default();
    Ok(runx_runtime::registry::skill_content_digest(
        text(record, "markdown")?,
        record["profile_document"].as_str(),
        &files,
    )?)
}

/// One request: its method, path segments (decoded), query and body.
struct Request {
    line: String,
    method: String,
    segments: Vec<String>,
    query: BTreeMap<String, String>,
    authorized: bool,
    body: Vec<u8>,
}

fn serve_connection(mut stream: TcpStream, state: &Mutex<State>, base: &str) {
    let Ok(request) = read_request(&mut stream) else {
        return;
    };
    lock(state).asked.push(request.line.clone());
    let (status, body) = route(&request, state, base).unwrap_or_else(|error| {
        (
            "400 Bad Request",
            json!({"status": "error", "error": {"code": "invalid_request", "detail": error.to_string()}}),
        )
    });
    let body = body.to_string();
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
}

type Response = (&'static str, Value);

fn not_found() -> Response {
    (
        "404 Not Found",
        json!({"status": "error", "error": {"code": "not_found"}}),
    )
}

fn success(mut body: Value) -> Response {
    body["status"] = json!("success");
    ("200 OK", body)
}

fn route(request: &Request, state: &Mutex<State>, base: &str) -> Result<Response, Error> {
    let segments = request
        .segments
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let method = request.method.as_str();
    Ok(match (method, segments.as_slice()) {
        ("GET", ["v1", "me"]) if request.authorized => {
            success(json!({"principal": {"principal_id": PRINCIPAL_ID}}))
        }
        ("GET", ["v1", "me"]) => (
            "401 Unauthorized",
            json!({"status": "error", "error": {"code": "unauthorized"}}),
        ),
        ("POST", ["v1", "skills"]) if request.authorized => publish(request, state, base)?,
        ("POST", ["v1", "admin", "registry", "publish"]) if request.authorized => {
            admin_publish(request, state, base)?
        }
        ("GET", ["v1", "skills"]) => {
            let state = lock(state);
            success(json!({"skills": search_skills(&state, &request.query)?}))
        }
        ("GET", ["v1", "skills", root]) => {
            let state = lock(state);
            let (root, version) = split_version(root);
            match state
                .roots
                .get(root)
                .and_then(|skill_id| state.version(skill_id, version))
            {
                Some(record) => success(json!({"skill": detail(record)})),
                None => not_found(),
            }
        }
        ("GET", ["v1", "skills", publisher, name]) => {
            let state = lock(state);
            let (name, version) = split_version(name);
            match state.version(&format!("{publisher}/{name}"), version) {
                Some(record) => success(json!({"skill": detail(record)})),
                None => not_found(),
            }
        }
        ("POST", ["v1", "skills", publisher, name, "acquire"]) => {
            let asked: Value = serde_json::from_slice(&request.body)?;
            let mut state = lock(state);
            let skill_id = format!("{publisher}/{name}");
            match state.version(&skill_id, asked["version"].as_str()).cloned() {
                Some(record) => {
                    state.installs += 1;
                    success(json!({
                        "install_count": state.installs,
                        "acquisition": acquisition(&record),
                    }))
                }
                None => not_found(),
            }
        }
        ("GET", ["v1", "skills", publisher, name, "versions"]) => {
            let state = lock(state);
            match state.skills.get(&format!("{publisher}/{name}")) {
                Some(versions) => success(page(
                    versions
                        .iter()
                        .rev()
                        .map(|record| record["version"].clone()),
                    state.page_size,
                    &request.query,
                )),
                None => not_found(),
            }
        }
        (
            "GET",
            [
                "v1",
                "skills",
                publisher,
                name,
                "versions",
                version,
                "record",
            ],
        ) => {
            let state = lock(state);
            match state.version(&format!("{publisher}/{name}"), Some(version)) {
                Some(record) => success(json!({"record": record})),
                None => not_found(),
            }
        }
        ("GET", ["v1", "connectors"]) => {
            let state = lock(state);
            success(json!({"connectors": search_connectors(&state, &request.query)}))
        }
        ("GET", ["v1", "connectors", rest @ ..]) => connector_route(rest, state, &request.query)?,
        _ => not_found(),
    })
}

/// A publish, as `POST /v1/skills` takes it: into the namespace it names, or
/// the principal's, at the version it names, or one named by its content.
fn publish(request: &Request, state: &Mutex<State>, base: &str) -> Result<Response, Error> {
    let body: Value = serde_json::from_slice(&request.body)?;
    let publisher = body["publisher"].as_str().unwrap_or(PUBLISHER);
    let (status, record) = match record_publish(&body, publisher, state)? {
        Ok(recorded) => recorded,
        Err(refused) => return Ok(refused),
    };
    let skill_id = text(&record, "skill_id")?;
    let version = text(&record, "version")?;
    Ok(success(json!({"publish": {
        "status": status,
        "skill_id": skill_id,
        "publisher": publisher,
        "name": record["name"],
        "version": version,
        "digest": record["digest"],
        "profile_digest": record["profile_digest"],
        "trust_tier": record["trust_tier"],
        "install_command": record["install_command"],
        "run_command": record["run_command"],
        "public_url": format!("{base}/x/{skill_id}@{version}"),
    }})))
}

/// An operator's publish, as `POST /v1/admin/registry/publish` takes it:
/// into the namespace it must name, with the test report it ran.
fn admin_publish(request: &Request, state: &Mutex<State>, base: &str) -> Result<Response, Error> {
    let body: Value = serde_json::from_slice(&request.body)?;
    let (Some(publisher), true) = (body["publisher"].as_str(), body["test"].is_object()) else {
        return Ok((
            "400 Bad Request",
            json!({"status": "error", "error": {"code": "invalid_request", "detail": "an operator publish names its publisher and carries its test report"}}),
        ));
    };
    let (status, record) = match record_publish(&body, publisher, state)? {
        Ok(recorded) => recorded,
        Err(refused) => return Ok(refused),
    };
    let skill_id = text(&record, "skill_id")?;
    let version = text(&record, "version")?;
    Ok(success(json!({"publish": {
        "status": status,
        "skill_id": skill_id,
        "name": record["name"],
        "version": version,
        "digest": record["digest"],
        "profile_digest": record["profile_digest"],
        "record": {"trust_tier": record["trust_tier"]},
        "link": {
            "install_command": record["install_command"],
            "run_command": record["run_command"],
            "public_url": format!("{base}/x/{skill_id}@{version}"),
        },
    }})))
}

/// Record a publish's version under `publisher`: the version it names, or
/// one named by its content. A version already published with other content
/// is refused.
fn record_publish(
    body: &Value,
    publisher: &str,
    state: &Mutex<State>,
) -> Result<Result<(&'static str, Value), Response>, Error> {
    let markdown = text(body, "markdown")?;
    let files: Vec<RegistryPackageFile> =
        serde_json::from_value(body["package_files"].clone()).unwrap_or_default();
    let profile = body["profile_document"].as_str();
    let content = runx_runtime::registry::skill_content_digest(markdown, profile, &files)?;
    let label = format!(
        "sha-{}",
        &content.trim_start_matches("sha256:")[..12.min(content.len())]
    );
    let version = body["version"].as_str().unwrap_or(&label);
    let record = build_record(&Publication {
        files,
        ..Publication::new(publisher, version, markdown, profile)
    })?;
    Ok(match lock(state).put(record.clone()) {
        Ok(Publish::Published) => Ok(("published", record)),
        Ok(Publish::Unchanged) => Ok(("unchanged", record)),
        Err(error) => Err((
            "409 Conflict",
            json!({"status": "error", "error": {"code": "version_conflict", "detail": error.to_string()}}),
        )),
    })
}

fn connector_route(
    rest: &[&str],
    state: &Mutex<State>,
    query: &BTreeMap<String, String>,
) -> Result<Response, Error> {
    let state = lock(state);
    let split = rest.iter().position(|segment| *segment == "versions");
    let name = rest[..split.unwrap_or(rest.len())].join("/");
    let Some(versions) = state.connectors.get(&name) else {
        return Ok(not_found());
    };
    let at = |version: &str| {
        versions
            .iter()
            .find(|stored| stored.detail["version"] == version)
    };
    Ok(match split.map(|index| &rest[index + 1..]) {
        None => versions.last().map_or_else(not_found, |stored| {
            success(json!({"connector": stored.detail}))
        }),
        Some([]) => success(page(
            versions
                .iter()
                .rev()
                .map(|stored| stored.detail["version"].clone()),
            state.page_size,
            query,
        )),
        Some([version]) => at(version).map_or_else(not_found, |stored| {
            success(json!({"connector": stored.detail}))
        }),
        Some([version, "package"]) => at(version).map_or_else(not_found, |stored| {
            success(json!({"package": stored.package}))
        }),
        Some(_) => not_found(),
    })
}

/// One page of `versions`, newest first, from the cursor asked.
fn page(
    versions: impl Iterator<Item = Value>,
    page_size: usize,
    query: &BTreeMap<String, String>,
) -> Value {
    let versions = versions.collect::<Vec<_>>();
    let start = query
        .get("cursor")
        .and_then(|cursor| cursor.parse::<usize>().ok())
        .unwrap_or(0);
    let end = (start + page_size).min(versions.len());
    let listed = versions[start.min(end)..end]
        .iter()
        .map(|version| json!({"version": version}))
        .collect::<Vec<_>>();
    let next = (end < versions.len()).then(|| end.to_string());
    json!({"versions": listed, "has_more": next.is_some(), "next_cursor": next})
}

fn matches(query: &BTreeMap<String, String>, texts: &[&Value]) -> bool {
    let Some(needle) = query.get("q").map(|q| q.to_lowercase()) else {
        return true;
    };
    texts
        .iter()
        .filter_map(|text| text.as_str())
        .any(|text| text.to_lowercase().contains(&needle))
}

fn limit(query: &BTreeMap<String, String>) -> usize {
    query
        .get("limit")
        .and_then(|limit| limit.parse().ok())
        .unwrap_or(20)
}

fn search_skills(state: &State, query: &BTreeMap<String, String>) -> Result<Vec<Value>, Error> {
    Ok(state
        .skills
        .values()
        .filter_map(|versions| versions.last())
        .filter(|record| {
            matches(
                query,
                &[&record["skill_id"], &record["name"], &record["description"]],
            )
        })
        .take(limit(query))
        .map(|record| {
            json!({
                "skill_id": record["skill_id"],
                "name": record["name"],
                "description": record["description"],
                "version": record["version"],
                "digest": record["digest"],
                "profile_mode": if record["profile_document"].is_string() { "profiled" } else { "portable" },
                "method_names": record["method_names"],
                "profile_digest": record["profile_digest"],
                "tags": record["tags"],
                "trust_tier": record["trust_tier"],
                "install_command": record["install_command"],
                "run_command": record["run_command"],
            })
        })
        .collect())
}

fn search_connectors(state: &State, query: &BTreeMap<String, String>) -> Vec<Value> {
    state
        .connectors
        .values()
        .filter_map(|versions| versions.last())
        .filter(|stored| {
            matches(
                query,
                &[
                    &stored.detail["connector"],
                    &stored.detail["name"],
                    &stored.detail["description"],
                ],
            )
        })
        .take(limit(query))
        .map(|stored| {
            json!({
                "connector": stored.detail["connector"],
                "version": stored.detail["version"],
                "name": stored.detail["name"],
                "description": stored.detail["description"],
                "runtime_capability": stored.detail["runtime_capability"],
                "trust_tier": stored.detail["trust_tier"],
            })
        })
        .collect()
}

/// A record's fields a read of the skill serves.
fn detail(record: &Value) -> Value {
    let mut detail = pick(
        record,
        &[
            "skill_id",
            "name",
            "description",
            "version",
            "digest",
            "signed_manifest",
            "markdown",
            "profile_digest",
            "package_files",
            "bundle_digest",
            "paid_listing",
            "method_names",
            "trust_tier",
            "tags",
            "publisher",
            "install_command",
            "run_command",
        ],
    );
    detail["attestations"] = json!([]);
    detail
}

/// A record's fields an acquisition serves.
fn acquisition(record: &Value) -> Value {
    let mut acquired = pick(
        record,
        &[
            "skill_id",
            "name",
            "version",
            "digest",
            "signed_manifest",
            "tier_statement",
            "markdown",
            "profile_document",
            "profile_digest",
            "package_files",
            "bundle_digest",
            "paid_listing",
            "method_names",
            "trust_tier",
            "publisher",
        ],
    );
    acquired["attestations"] = json!([]);
    acquired
}

fn pick(record: &Value, fields: &[&str]) -> Value {
    Value::Object(
        fields
            .iter()
            .filter_map(|field| {
                record
                    .get(*field)
                    .filter(|value| !value.is_null())
                    .map(|value| ((*field).to_owned(), value.clone()))
            })
            .collect(),
    )
}

/// `name@version` as the name and the version, when it names one.
fn split_version(segment: &str) -> (&str, Option<&str>) {
    match segment.rsplit_once('@') {
        Some((name, version)) if !name.is_empty() => (name, Some(version)),
        _ => (segment, None),
    }
}

fn read_request(stream: &mut TcpStream) -> std::io::Result<Request> {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 8192];
    let head_end = loop {
        if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break end;
        }
        let read = stream.read(&mut buffer)?;
        if read == 0 {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        bytes.extend_from_slice(&buffer[..read]);
    };
    let head = String::from_utf8_lossy(&bytes[..head_end]).into_owned();
    let mut lines = head.lines();
    let line = lines.next().unwrap_or_default().to_owned();
    let mut content_length = 0;
    let mut authorized = false;
    for header in lines {
        let Some((name, value)) = header.split_once(':') else {
            continue;
        };
        if name.eq_ignore_ascii_case("content-length") {
            content_length = value.trim().parse().unwrap_or(0);
        } else if name.eq_ignore_ascii_case("authorization") {
            authorized = value
                .trim()
                .strip_prefix("Bearer ")
                .is_some_and(|token| !token.trim().is_empty());
        }
    }
    let mut body = bytes[head_end + 4..].to_vec();
    while body.len() < content_length {
        let read = stream.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        body.extend_from_slice(&buffer[..read]);
    }
    let mut parts = line.split(' ');
    let method = parts.next().unwrap_or_default().to_owned();
    let target = parts.next().unwrap_or_default();
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    Ok(Request {
        method,
        segments: path
            .split('/')
            .filter(|segment| !segment.is_empty())
            .map(decode)
            .collect(),
        query: query
            .split('&')
            .filter_map(|pair| pair.split_once('='))
            .map(|(name, value)| (decode(name), decode(&value.replace('+', " "))))
            .collect(),
        authorized,
        body,
        line,
    })
}

/// A percent-encoded URL component, decoded.
fn decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let hex = bytes
            .get(index + 1..index + 3)
            .filter(|_| bytes[index] == b'%')
            .and_then(|hex| std::str::from_utf8(hex).ok())
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match hex {
            Some(byte) => {
                decoded.push(byte);
                index += 3;
            }
            None => {
                decoded.push(bytes[index]);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}
