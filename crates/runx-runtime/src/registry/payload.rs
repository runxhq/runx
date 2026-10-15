use serde::Deserialize;
use serde::de::DeserializeOwned;

use super::connector_install::RegistryConnectorVersion;
use super::http::RegistryClientError;
use super::types::{
    AcquiredRegistrySkill, RegistryConnectorDetail, RegistryConnectorSearchResult,
    RegistrySearchResult, RegistrySkillDetail, RegistrySkillVersion,
};

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum SuccessStatus {
    Success,
}

#[derive(Deserialize)]
struct SearchEnvelope {
    #[serde(rename = "status")]
    _status: SuccessStatus,
    skills: Vec<RegistrySearchResult>,
}

#[derive(Deserialize)]
struct ReadEnvelope {
    #[serde(rename = "status")]
    _status: SuccessStatus,
    skill: RegistrySkillDetail,
}

#[derive(Deserialize)]
struct AcquireEnvelope {
    #[serde(rename = "status")]
    _status: SuccessStatus,
    install_count: u64,
    acquisition: AcquiredRegistrySkill,
}

#[derive(Deserialize)]
struct RecordEnvelope {
    #[serde(rename = "status")]
    _status: SuccessStatus,
    record: RegistrySkillVersion,
}

#[derive(Deserialize)]
struct ConnectorEnvelope {
    #[serde(rename = "status")]
    _status: SuccessStatus,
    connector: RegistryConnectorDetail,
}

#[derive(Deserialize)]
struct ConnectorSearchEnvelope {
    #[serde(rename = "status")]
    _status: SuccessStatus,
    connectors: Vec<RegistryConnectorSearchResult>,
}

#[derive(Deserialize)]
struct ConnectorPackageEnvelope {
    #[serde(rename = "status")]
    _status: SuccessStatus,
    package: RegistryConnectorVersion,
}

/// Only whether a served package was withdrawn, read before its contract.
#[derive(Deserialize)]
struct WithdrawnProbe {
    package: WithdrawnPackage,
}

#[derive(Deserialize)]
struct WithdrawnPackage {
    #[serde(rename = "ref")]
    name: String,
    version: String,
    withdrawn_at: Option<String>,
}

pub(crate) fn parse_search(
    route: &str,
    body: &str,
) -> Result<Vec<RegistrySearchResult>, RegistryClientError> {
    decode::<SearchEnvelope>(route, body).map(|envelope| envelope.skills)
}

pub(crate) fn parse_read(
    route: &str,
    body: &str,
) -> Result<RegistrySkillDetail, RegistryClientError> {
    decode::<ReadEnvelope>(route, body).map(|envelope| envelope.skill)
}

pub(crate) fn parse_acquire(
    route: &str,
    body: &str,
) -> Result<AcquiredRegistrySkill, RegistryClientError> {
    decode::<AcquireEnvelope>(route, body).map(|envelope| AcquiredRegistrySkill {
        install_count: envelope.install_count,
        ..envelope.acquisition
    })
}

pub(crate) fn parse_record(
    route: &str,
    body: &str,
) -> Result<RegistrySkillVersion, RegistryClientError> {
    decode::<RecordEnvelope>(route, body).map(|envelope| envelope.record)
}

pub(crate) fn parse_connector(
    route: &str,
    body: &str,
) -> Result<RegistryConnectorDetail, RegistryClientError> {
    decode::<ConnectorEnvelope>(route, body).map(|envelope| envelope.connector)
}

pub(crate) fn parse_connector_search(
    route: &str,
    body: &str,
) -> Result<Vec<RegistryConnectorSearchResult>, RegistryClientError> {
    decode::<ConnectorSearchEnvelope>(route, body).map(|envelope| envelope.connectors)
}

pub(crate) fn parse_connector_package(
    route: &str,
    body: &str,
) -> Result<RegistryConnectorVersion, RegistryClientError> {
    // A withdrawn version is served as its stored bytes, which no current
    // contract reads and nothing installs.
    if let Ok(WithdrawnProbe {
        package:
            WithdrawnPackage {
                name,
                version,
                withdrawn_at: Some(withdrawn_at),
            },
    }) = serde_json::from_str::<WithdrawnProbe>(body)
    {
        return Err(RegistryClientError::Withdrawn {
            subject: format!("{name}@{version}"),
            withdrawn_at,
        });
    }
    decode::<ConnectorPackageEnvelope>(route, body).map(|envelope| envelope.package)
}

fn decode<T: DeserializeOwned>(route: &str, body: &str) -> Result<T, RegistryClientError> {
    let mut deserializer = serde_json::Deserializer::from_str(body);
    serde_path_to_error::deserialize(&mut deserializer).map_err(|error| {
        let field_path = json_path(error.path());
        let error = error.into_inner();
        if error.classify() == serde_json::error::Category::Data {
            RegistryClientError::Contract {
                route: route.to_owned(),
                field_path,
                message: error.to_string(),
            }
        } else {
            RegistryClientError::InvalidJson {
                route: route.to_owned(),
                message: error.to_string(),
            }
        }
    })
}

fn json_path(path: &serde_path_to_error::Path) -> String {
    let path = path.to_string();
    if path.is_empty() || path == "." {
        "$".to_owned()
    } else if path.starts_with('[') {
        format!("${path}")
    } else {
        format!("$.{path}")
    }
}
