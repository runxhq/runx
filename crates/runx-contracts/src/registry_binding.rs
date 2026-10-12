//! Upstream registry binding contract (`runx.registry_binding.v1`): the open
//! (`additionalProperties: true`) document tying a skill to its upstream source,
//! registry placement, and harness verification status.
//!
//! Identity is the legacy bare `runx.ai/schemas` `$id` (no `x-runx-schema`).
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::schema::{Property, RunxSchema, deserialize_true_bool, object_schema};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
pub enum RegistryBindingSchema {
    #[serde(rename = "runx.registry_binding.v1")]
    V1,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum RegistryBindingState {
    RegistryBindingDrafted,
    RegistryBound,
    HarnessVerified,
    Published,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum RegistryTrustTier {
    FirstParty,
    Verified,
    Community,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum RegistryHarnessStatus {
    Pending,
    Failed,
    HarnessVerified,
}

/// The skill identity for a registry binding. Open (`additionalProperties:
/// true`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
pub struct RegistryBindingSkill {
    pub id: String,
    pub name: String,
    pub description: String,
}

/// The upstream source of truth for a registry binding. Open.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryBindingUpstream {
    pub host: String,
    pub owner: String,
    pub repo: String,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    pub commit: String,
    pub blob_sha: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pr_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub merged_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub html_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_url: Option<String>,
    #[serde(deserialize_with = "deserialize_true_bool")]
    pub source_of_truth: bool,
}

impl RunxSchema for RegistryBindingUpstream {
    fn json_schema() -> Value {
        object_schema(
            vec![
                Property::new("host", String::json_schema(), true),
                Property::new("owner", String::json_schema(), true),
                Property::new("repo", String::json_schema(), true),
                Property::new("path", String::json_schema(), true),
                Property::new("branch", String::json_schema(), false),
                Property::new("commit", String::json_schema(), true),
                Property::new("blob_sha", String::json_schema(), true),
                Property::new("pr_url", String::json_schema(), false),
                Property::new("merged_at", String::json_schema(), false),
                Property::new("html_url", String::json_schema(), false),
                Property::new("raw_url", String::json_schema(), false),
                Property::new("source_of_truth", json!({ "const": true }), true),
            ],
            false,
            None,
        )
    }
}

/// The registry placement for a binding. Open.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryBindingRegistry {
    pub owner: String,
    pub trust_tier: RegistryTrustTier,
    pub version: String,
    pub profile_path: String,
    #[serde(deserialize_with = "deserialize_true_bool")]
    pub materialized_package_is_registry_artifact: bool,
}

impl RunxSchema for RegistryBindingRegistry {
    fn json_schema() -> Value {
        object_schema(
            vec![
                Property::new("owner", String::json_schema(), true),
                Property::new("trust_tier", RegistryTrustTier::json_schema(), true),
                Property::new("version", String::json_schema(), true),
                Property::new("profile_path", String::json_schema(), true),
                Property::new(
                    "materialized_package_is_registry_artifact",
                    json!({ "const": true }),
                    true,
                ),
            ],
            false,
            None,
        )
    }
}

/// The harness verification status for a binding. Open.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
pub struct RegistryBindingHarness {
    pub status: RegistryHarnessStatus,
    pub case_count: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assertion_count: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub case_names: Option<Vec<String>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[runx_schema(id = "runx.registry_binding.v1")]
pub struct RegistryBinding {
    pub schema: RegistryBindingSchema,
    pub state: RegistryBindingState,
    pub skill: RegistryBindingSkill,
    pub upstream: RegistryBindingUpstream,
    pub registry: RegistryBindingRegistry,
    pub harness: RegistryBindingHarness,
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::RegistryBinding;

    fn valid_binding() -> Value {
        json!({
            "schema": "runx.registry_binding.v1",
            "state": "registry_bound",
            "skill": {
                "id": "runx/sourcey",
                "name": "sourcey",
                "description": "Docs skill."
            },
            "upstream": {
                "host": "github.com",
                "owner": "runxhq",
                "repo": "runx",
                "path": "skills/sourcey",
                "commit": "abc123",
                "blob_sha": "def456",
                "source_of_truth": true
            },
            "registry": {
                "owner": "runx",
                "trust_tier": "first_party",
                "version": "1.0.0",
                "profile_path": "run.yaml",
                "materialized_package_is_registry_artifact": true
            },
            "harness": {
                "status": "harness_verified",
                "case_count": 1
            }
        })
    }

    #[test]
    fn registry_binding_rejects_false_ownership_markers() {
        let mut source_false = valid_binding();
        source_false["upstream"]["source_of_truth"] = json!(false);
        assert!(serde_json::from_value::<RegistryBinding>(source_false).is_err());

        let mut artifact_false = valid_binding();
        artifact_false["registry"]["materialized_package_is_registry_artifact"] = json!(false);
        assert!(serde_json::from_value::<RegistryBinding>(artifact_false).is_err());
    }
}
