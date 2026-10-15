use serde_json::json;
use url::Url;

use runx_contracts::connector::ConnectorName;

use super::connector_install::RegistryConnectorVersion;
use super::payload::{
    parse_acquire, parse_connector, parse_connector_package, parse_connector_search, parse_read,
    parse_record, parse_search,
};
use super::types::{
    AcquiredRegistrySkill, RegistryConnectorDetail, RegistryConnectorSearchResult,
    RegistrySearchResult, RegistrySkillDetail, RegistrySkillVersion,
};

use crate::http::{
    HttpMethod, ReqwestHttpTransport as DefaultRuntimeHttpTransport, RuntimeHttpError,
    RuntimeHttpHeader, RuntimeHttpRequest as HttpRequest, RuntimeHttpTransport as Transport,
    strip_one_trailing_slash,
};

#[derive(Clone, Debug)]
pub struct RegistryClient<T = DefaultRuntimeHttpTransport> {
    base_url: String,
    transport: T,
}

impl RegistryClient<DefaultRuntimeHttpTransport> {
    pub fn new(base_url: impl AsRef<str>) -> Result<Self, RegistryClientError> {
        Self::with_transport(base_url, DefaultRuntimeHttpTransport::new()?)
    }
}

impl<T: Transport> RegistryClient<T> {
    pub fn with_transport(
        base_url: impl AsRef<str>,
        transport: T,
    ) -> Result<Self, RegistryClientError> {
        let base_url = strip_one_trailing_slash(base_url.as_ref());
        let url = Url::parse(&base_url).map_err(|error| {
            RegistryClientError::RuntimeHttp(RuntimeHttpError::InvalidUrl(error))
        })?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(RegistryClientError::RuntimeHttp(
                RuntimeHttpError::UnsupportedUrlScheme {
                    scheme: url.scheme().to_owned(),
                },
            ));
        }
        Ok(Self {
            base_url,
            transport,
        })
    }

    pub fn search(&self, query: &str) -> Result<Vec<RegistrySearchResult>, RegistryClientError> {
        self.search_with_limit(query, 20)
    }

    pub fn search_with_limit(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<RegistrySearchResult>, RegistryClientError> {
        let mut url = Url::parse(&format!("{}/v1/skills", self.base_url)).map_err(|error| {
            RegistryClientError::RuntimeHttp(RuntimeHttpError::InvalidUrl(error))
        })?;
        {
            let mut pairs = url.query_pairs_mut();
            let trimmed = query.trim();
            if !trimmed.is_empty() {
                pairs.append_pair("q", trimmed);
            }
            pairs.append_pair("limit", &limit.to_string());
        }
        let route = route_path(url.path(), url.query());
        let response = self.transport.send(HttpRequest {
            method: HttpMethod::Get,
            url: url.to_string(),
            headers: Vec::new(),
            body: None,
        })?;
        ensure_success(&route, response.status)?;
        parse_search(&route, &response.body)
    }

    pub fn read(
        &self,
        skill_id: &str,
        version: Option<&str>,
    ) -> Result<Option<RegistrySkillDetail>, RegistryClientError> {
        let (owner, name) = validated_skill_id(skill_id)?;
        let suffix = version
            .map(|version| format!("{name}@{version}"))
            .unwrap_or_else(|| name.to_owned());
        let route = format!(
            "/v1/skills/{}/{}",
            encode_segment(owner),
            encode_segment(&suffix)
        );
        let response = self.transport.send(HttpRequest {
            method: HttpMethod::Get,
            url: format!("{}{}", self.base_url, route),
            headers: Vec::new(),
            body: None,
        })?;
        if response.status == 404 {
            return Ok(None);
        }
        ensure_success(&route, response.status)?;
        parse_read(&route, &response.body).map(Some)
    }

    /// The skill a root name addresses, read as its canonical
    /// `<owner>/<name>`. A root name is one segment the registry assigns to a
    /// verified publisher's skill, and may move to another; `None` when no
    /// skill holds it.
    pub fn read_root(
        &self,
        root: &str,
        version: Option<&str>,
    ) -> Result<Option<RegistrySkillDetail>, RegistryClientError> {
        if root.is_empty() || root.contains('/') {
            return Err(RegistryClientError::InvalidRootName(root.to_owned()));
        }
        let suffix = version.map_or_else(|| root.to_owned(), |version| format!("{root}@{version}"));
        let route = format!("/v1/skills/{}", encode_segment(&suffix));
        let Some(detail) = self.get(&route, parse_read)? else {
            return Ok(None);
        };
        if validated_skill_id(&detail.skill_id).is_err()
            || version.is_some_and(|version| detail.version != version)
        {
            return Err(RegistryClientError::Contract {
                route,
                field_path: "skill".to_owned(),
                message: format!(
                    "the registry answered {}@{} for {suffix}",
                    detail.skill_id, detail.version
                ),
            });
        }
        Ok(Some(detail))
    }

    pub fn acquire(
        &self,
        skill_id: &str,
        options: AcquireOptions<'_>,
    ) -> Result<AcquiredRegistrySkill, RegistryClientError> {
        if options.installation_id.trim().is_empty() {
            return Err(RegistryClientError::MissingInstallationId);
        }
        let (owner, name) = validated_skill_id(skill_id)?;
        let route = format!(
            "/v1/skills/{}/{}/acquire",
            encode_segment(owner),
            encode_segment(name)
        );
        let channel = options.channel.unwrap_or("cli");
        let body = json!({
            "installation_id": options.installation_id,
            "version": options.version,
            "channel": channel,
        })
        .to_string();
        let response = self.transport.send(HttpRequest {
            method: HttpMethod::Post,
            url: format!("{}{}", self.base_url, route),
            headers: vec![RuntimeHttpHeader::new("content-type", "application/json")],
            body: Some(body),
        })?;
        ensure_success(&route, response.status)?;
        parse_acquire(&route, &response.body)
    }

    /// One executable published version as the registry signed it: the
    /// record a registry directory holds, so a host can build a closure.
    pub fn version_record(
        &self,
        skill_id: &str,
        version: &str,
    ) -> Result<Option<RegistrySkillVersion>, RegistryClientError> {
        let (owner, name) = validated_skill_id(skill_id)?;
        let route = format!(
            "/v1/skills/{}/{}/versions/{}/record",
            encode_segment(owner),
            encode_segment(name),
            encode_segment(version)
        );
        let Some(record) = self.get(&route, parse_record)? else {
            return Ok(None);
        };
        if record.skill_id != skill_id || record.version != version {
            return Err(RegistryClientError::Contract {
                route,
                field_path: "record".to_owned(),
                message: format!(
                    "the registry answered {}@{} for {skill_id}@{version}",
                    record.skill_id, record.version
                ),
            });
        }
        Ok(Some(record))
    }

    /// The published connectors matching `query`, each at its current version.
    pub fn search_connectors(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<RegistryConnectorSearchResult>, RegistryClientError> {
        let mut url = Url::parse(&format!("{}/v1/connectors", self.base_url)).map_err(|error| {
            RegistryClientError::RuntimeHttp(RuntimeHttpError::InvalidUrl(error))
        })?;
        {
            let mut pairs = url.query_pairs_mut();
            let trimmed = query.trim();
            if !trimmed.is_empty() {
                pairs.append_pair("q", trimmed);
            }
            pairs.append_pair("limit", &limit.to_string());
        }
        let route = route_path(url.path(), url.query());
        let response = self.transport.send(HttpRequest {
            method: HttpMethod::Get,
            url: url.to_string(),
            headers: Vec::new(),
            body: None,
        })?;
        ensure_success(&route, response.status)?;
        parse_connector_search(&route, &response.body)
    }

    /// A published connector: `version`, or its current version.
    pub fn read_connector(
        &self,
        name: &ConnectorName,
        version: Option<&str>,
    ) -> Result<Option<RegistryConnectorDetail>, RegistryClientError> {
        let mut route = connector_route(name);
        if let Some(version) = version {
            route.push_str(&format!("/versions/{}", encode_segment(version)));
        }
        self.get(&route, parse_connector)
    }

    /// The files and signed manifest of one published connector version.
    pub fn connector_package(
        &self,
        name: &ConnectorName,
        version: &str,
    ) -> Result<Option<RegistryConnectorVersion>, RegistryClientError> {
        let route = format!(
            "{}/versions/{}/package",
            connector_route(name),
            encode_segment(version)
        );
        self.get(&route, parse_connector_package)
    }

    fn get<V>(
        &self,
        route: &str,
        parse: fn(&str, &str) -> Result<V, RegistryClientError>,
    ) -> Result<Option<V>, RegistryClientError> {
        let response = self.transport.send(HttpRequest {
            method: HttpMethod::Get,
            url: format!("{}{}", self.base_url, route),
            headers: Vec::new(),
            body: None,
        })?;
        if response.status == 404 {
            return Ok(None);
        }
        ensure_success(route, response.status)?;
        parse(route, &response.body).map(Some)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AcquireOptions<'a> {
    pub installation_id: &'a str,
    pub version: Option<&'a str>,
    pub channel: Option<&'a str>,
}

#[derive(Debug, thiserror::Error)]
pub enum RegistryClientError {
    #[error(transparent)]
    RuntimeHttp(#[from] RuntimeHttpError),
    #[error("invalid registry skill id '{0}'. Expected '<owner>/<name>'.")]
    InvalidSkillId(String),
    #[error("invalid registry root name '{0}'. A root name is one segment.")]
    InvalidRootName(String),
    #[error(
        "{subject} was withdrawn from the registry at {withdrawn_at}; it is kept as written and never installed"
    )]
    Withdrawn {
        subject: String,
        withdrawn_at: String,
    },
    #[error("registry route {route} failed with HTTP {status}")]
    HttpStatus { route: String, status: u16 },
    #[error("registry route {route} returned invalid JSON: {message}")]
    InvalidJson { route: String, message: String },
    #[error("registry route {route} contract error at {field_path}: {message}")]
    Contract {
        route: String,
        field_path: String,
        message: String,
    },
    #[error("remote registry installs require an installation id")]
    MissingInstallationId,
}

fn ensure_success(route: &str, status: u16) -> Result<(), RegistryClientError> {
    if (200..=299).contains(&status) {
        Ok(())
    } else {
        Err(RegistryClientError::HttpStatus {
            route: route.to_owned(),
            status,
        })
    }
}

fn validated_skill_id(skill_id: &str) -> Result<(&str, &str), RegistryClientError> {
    super::local::split_skill_id(skill_id)
        .map_err(|_| RegistryClientError::InvalidSkillId(skill_id.to_owned()))
}

/// `/v1/connectors/<name>`: a name is one or two path segments.
fn connector_route(name: &ConnectorName) -> String {
    let segments = name
        .as_str()
        .split('/')
        .map(encode_segment)
        .collect::<Vec<_>>()
        .join("/");
    format!("/v1/connectors/{segments}")
}

fn encode_segment(value: &str) -> String {
    let mut output = String::new();
    for byte in value.bytes() {
        // RFC 3986 unreserved characters stay as they are.
        let keep = byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~');
        if keep {
            output.push(char::from(byte));
        } else {
            output.push_str(&format!("%{byte:02X}"));
        }
    }
    output
}

fn route_path(path: &str, query: Option<&str>) -> String {
    match query {
        Some(query) if !query.is_empty() => format!("{path}?{query}"),
        _ => path.to_owned(),
    }
}
