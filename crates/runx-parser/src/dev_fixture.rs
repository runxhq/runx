//! Pure parsing and validation for `runx dev` fixtures.

use std::str::FromStr;

use runx_contracts::{JsonObject, JsonValue};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::harness_fixture::HarnessCaller;
use crate::{ParseError, parse_yaml_document};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DevFixtureLane {
    #[default]
    Deterministic,
    RepoIntegration,
    Agent,
}

impl DevFixtureLane {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Deterministic => "deterministic",
            Self::RepoIntegration => "repo-integration",
            Self::Agent => "agent",
        }
    }
}

impl FromStr for DevFixtureLane {
    type Err = DevFixtureError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "deterministic" => Ok(Self::Deterministic),
            "repo-integration" => Ok(Self::RepoIntegration),
            "agent" => Ok(Self::Agent),
            _ => Err(DevFixtureError::Invalid {
                field: "lane".to_owned(),
                message: format!(
                    "must be deterministic, repo-integration, or agent; got {value:?}"
                ),
            }),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DevFixtureTargetKind {
    Tool,
    Skill,
    Graph,
}

impl DevFixtureTargetKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Tool => "tool",
            Self::Skill => "skill",
            Self::Graph => "graph",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DevFixtureTarget {
    pub kind: DevFixtureTargetKind,
    #[serde(rename = "ref")]
    pub reference: String,
}

impl DevFixtureTarget {
    #[must_use]
    pub fn to_json_object(&self) -> JsonObject {
        JsonObject::from_iter([
            (
                "kind".to_owned(),
                JsonValue::String(self.kind.as_str().to_owned()),
            ),
            ("ref".to_owned(), JsonValue::String(self.reference.clone())),
        ])
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DevExpectedStatus {
    #[default]
    Success,
    Failure,
}

impl DevExpectedStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failure => "failure",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DevOutputExpectation {
    pub exact: Option<JsonValue>,
    pub subset: Option<JsonValue>,
    pub matches_packet: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DevFixtureExpectation {
    #[serde(default)]
    pub status: DevExpectedStatus,
    pub output: Option<DevOutputExpectation>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DevFixtureGitConfig {
    pub initial_branch: Option<String>,
    pub commit: Option<bool>,
    #[serde(default)]
    pub dirty_files: JsonObject,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DevFixtureGit {
    Enabled(bool),
    Config(DevFixtureGitConfig),
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DevFixtureWorkspace {
    #[serde(default)]
    pub files: JsonObject,
    #[serde(default)]
    pub json_files: JsonObject,
    #[serde(default)]
    pub executable_files: JsonObject,
    pub git: Option<DevFixtureGit>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DevFixture {
    pub name: String,
    pub lane: DevFixtureLane,
    pub target: DevFixtureTarget,
    pub inputs: JsonObject,
    pub env: JsonObject,
    pub caller: HarnessCaller,
    pub workspace: Option<DevFixtureWorkspace>,
    pub expect: DevFixtureExpectation,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDevFixture {
    name: String,
    #[serde(default)]
    lane: DevFixtureLane,
    target: DevFixtureTarget,
    #[serde(default)]
    inputs: JsonObject,
    #[serde(default)]
    env: JsonObject,
    #[serde(default)]
    caller: HarnessCaller,
    workspace: Option<DevFixtureWorkspace>,
    #[serde(default)]
    expect: DevFixtureExpectation,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum DevFixtureError {
    #[error("failed to parse dev fixture YAML: {0}")]
    Parse(#[from] ParseError),
    #[error("dev fixture {field} must not be empty")]
    Empty { field: &'static str },
    #[error("invalid dev fixture {field}: {message}")]
    Invalid { field: String, message: String },
}

pub fn parse_dev_fixture(contents: &str) -> Result<DevFixture, DevFixtureError> {
    validate_fixture(parse_yaml_document::<RawDevFixture>(contents)?)
}

fn validate_fixture(fixture: RawDevFixture) -> Result<DevFixture, DevFixtureError> {
    require_non_empty(&fixture.name, "name")?;
    require_non_empty(&fixture.target.reference, "target.ref")?;
    if let Some(workspace) = &fixture.workspace {
        validate_workspace(workspace)?;
    }
    if let Some(output) = &fixture.expect.output
        && let Some(packet) = &output.matches_packet
    {
        require_non_empty(packet, "expect.output.matches_packet")?;
    }
    Ok(DevFixture {
        name: fixture.name,
        lane: fixture.lane,
        target: fixture.target,
        inputs: fixture.inputs,
        env: fixture.env,
        caller: fixture.caller,
        workspace: fixture.workspace,
        expect: fixture.expect,
    })
}

fn validate_workspace(workspace: &DevFixtureWorkspace) -> Result<(), DevFixtureError> {
    for (field, files) in [
        ("workspace.files", &workspace.files),
        ("workspace.json_files", &workspace.json_files),
        ("workspace.executable_files", &workspace.executable_files),
    ] {
        validate_file_map(field, files)?;
    }
    if let Some(DevFixtureGit::Config(config)) = &workspace.git {
        if let Some(branch) = &config.initial_branch {
            require_non_empty(branch, "workspace.git.initial_branch")?;
        }
        validate_file_map("workspace.git.dirty_files", &config.dirty_files)?;
    }
    Ok(())
}

fn validate_file_map(field: &str, files: &JsonObject) -> Result<(), DevFixtureError> {
    for path in files.keys() {
        if path.starts_with('/')
            || path.contains('\\')
            || path
                .split('/')
                .any(|segment| segment.is_empty() || matches!(segment, "." | ".."))
        {
            return Err(DevFixtureError::Invalid {
                field: format!("{field}.{path}"),
                message: "must be a normalized workspace-relative path".to_owned(),
            });
        }
    }
    Ok(())
}

fn require_non_empty(value: &str, field: &'static str) -> Result<(), DevFixtureError> {
    if value.trim().is_empty() {
        return Err(DevFixtureError::Empty { field });
    }
    Ok(())
}
