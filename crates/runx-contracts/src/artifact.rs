//! The artifact contract: one retained value, where it is, what it is, where
//! it came from, and where it may be shared, and one page read of it.
use serde::{Deserialize, Serialize};

use crate::schema::{IsoDateTime, NonEmptyString, RunxSchema};
use crate::{ActAddress, ContextSharing, Digest, JsonValue, Reference};

pub const ARTIFACT_SCHEMA: &str = "runx.artifact.v1";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
pub enum ArtifactSchema {
    #[serde(rename = "runx.artifact.v1")]
    V1,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct ArtifactProducedBy {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receipt_ref: Option<Reference>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_ref: Option<Reference>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub act: Option<ActAddress>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decision_ref: Option<Reference>,
}

/// Where an artifact may be shared, in a context entry's own words: a
/// `local_only` artifact reaches only a model that runs on this machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct ArtifactLabels {
    pub sharing: ContextSharing,
}

impl ArtifactLabels {
    /// What the model the person configured may see, wherever it runs.
    pub const CONFIGURED_MODEL: Self = Self {
        sharing: ContextSharing::ConfiguredModel,
    };
    /// What only a model on this machine may see.
    pub const LOCAL_ONLY: Self = Self {
        sharing: ContextSharing::LocalOnly,
    };

    /// The labels of what is made from artifacts labelled `sources`: `local_only`
    /// when any of them is, and otherwise `configured_model`. A default, not a
    /// wall: what a step derives from a `local_only` value by its own means
    /// isn't followed.
    #[must_use]
    pub fn derived_from<'a>(sources: impl IntoIterator<Item = &'a Self>) -> Self {
        if sources.into_iter().any(|labels| labels.is_local_only()) {
            Self::LOCAL_ONLY
        } else {
            Self::CONFIGURED_MODEL
        }
    }

    /// The labels of what is made from `value`: derived from every artifact
    /// in it, at any depth. A value with none is `configured_model`.
    #[must_use]
    pub fn of_value(value: &JsonValue) -> Self {
        let mut found = Vec::new();
        collect_labels(value, &mut found);
        Self::derived_from(&found)
    }

    #[must_use]
    pub fn is_local_only(self) -> bool {
        self.sharing == ContextSharing::LocalOnly
    }
}

/// Every `runx.artifact.v1` descriptor's labels in `value`. An object that
/// names the schema but isn't an artifact adds none.
fn collect_labels(value: &JsonValue, found: &mut Vec<ArtifactLabels>) {
    match value {
        JsonValue::Object(object) => {
            if object.get("schema").and_then(JsonValue::as_str) == Some(ARTIFACT_SCHEMA)
                && let Ok(artifact) = value.clone().deserialize_into::<Artifact>()
            {
                found.push(artifact.labels);
            }
            for nested in object.values() {
                collect_labels(nested, found);
            }
        }
        JsonValue::Array(items) => {
            for nested in items {
                collect_labels(nested, found);
            }
        }
        _ => {}
    }
}

/// One retained value: where it is, what it is, where it came from, and
/// where it may be shared.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.artifact.v1")]
pub struct Artifact {
    pub schema: ArtifactSchema,
    /// Where it is: an opaque reference a native artifact tool resolves. It
    /// names the store and the content, and grants no authority.
    #[serde(rename = "ref")]
    pub reference: NonEmptyString,
    /// The digest of its bytes.
    pub digest: Digest,
    pub size_bytes: u64,
    pub media_type: NonEmptyString,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<IsoDateTime>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub produced_by: Option<ArtifactProducedBy>,
    /// Its path, when it lives in the project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<NonEmptyString>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<NonEmptyString>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<Reference>,
    pub labels: ArtifactLabels,
}

/// One bounded page read of an artifact.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.artifact_page.v1")]
pub struct ArtifactPage {
    /// The artifact the page is of.
    #[serde(rename = "ref")]
    pub reference: NonEmptyString,
    pub offset: u64,
    pub length: u64,
    pub next_offset: u64,
    pub eof: bool,
    /// The digest of this page's bytes.
    pub range_digest: Digest,
    /// The digest of the whole artifact's bytes.
    pub digest: Digest,
    pub encoding: NonEmptyString,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub records: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::{Artifact, ArtifactLabels};

    #[test]
    fn an_artifact_uses_its_wire_fields_and_refuses_the_old_ones() -> Result<(), serde_json::Error>
    {
        let digest = format!("sha256:{}", "a".repeat(64));
        let value = serde_json::json!({
            "schema": "runx.artifact.v1",
            "ref": format!("runx:project-artifact:{digest}"),
            "digest": digest,
            "size_bytes": 12,
            "media_type": "application/json",
            "path": "/project/.runx/artifacts/request.json",
            "labels": {"sharing": "configured_model"}
        });
        let artifact: Artifact = serde_json::from_value(value.clone())?;
        assert_eq!(serde_json::to_value(artifact)?, value);
        for (old, field) in [
            ("uri", "ref"),
            ("bytes", "size_bytes"),
            ("whole_digest", "digest"),
        ] {
            let mut renamed = value.clone();
            if let Some(object) = renamed.as_object_mut()
                && let Some(moved) = object.remove(field)
            {
                object.insert(old.to_owned(), moved);
            }
            assert!(
                serde_json::from_value::<Artifact>(renamed).is_err(),
                "{old}"
            );
        }
        Ok(())
    }

    #[test]
    fn what_is_made_from_a_local_only_artifact_is_local_only() {
        let local = ArtifactLabels::LOCAL_ONLY;
        let shared = ArtifactLabels::CONFIGURED_MODEL;
        assert!(ArtifactLabels::derived_from([&shared, &local]).is_local_only());
        assert!(!ArtifactLabels::derived_from([&shared]).is_local_only());
        assert!(!ArtifactLabels::derived_from([]).is_local_only());
    }

    #[test]
    fn a_value_with_a_local_only_artifact_at_any_depth_is_local_only()
    -> Result<(), serde_json::Error> {
        let artifact = |sharing: &str| {
            let digest = format!("sha256:{}", "b".repeat(64));
            serde_json::json!({
                "schema": "runx.artifact.v1",
                "ref": format!("runx:project-artifact:{digest}"),
                "digest": digest,
                "size_bytes": 2,
                "media_type": "application/json",
                "labels": {"sharing": sharing}
            })
        };
        let labels = |value: serde_json::Value| {
            serde_json::from_value::<crate::JsonValue>(value)
                .map(|value| ArtifactLabels::of_value(&value))
        };
        assert!(
            labels(serde_json::json!({"report": {"parts": [artifact("local_only")]}}))?
                .is_local_only()
        );
        assert!(
            !labels(serde_json::json!({"report": [artifact("configured_model")]}))?.is_local_only()
        );
        assert!(!labels(serde_json::json!({"text": "local_only"}))?.is_local_only());
        // An object that only names the schema isn't an artifact.
        assert!(
            !labels(serde_json::json!({"schema": "runx.artifact.v1", "labels": {"sharing": "local_only"}}))?
                .is_local_only()
        );
        Ok(())
    }
}
