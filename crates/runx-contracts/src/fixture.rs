//! Fixture contract for dev/replay inputs.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::JsonObject;
use crate::schema::{NamedSchemas, Property, RunxSchema, object_schema, record_named, titled};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "kebab-case")]
pub enum FixtureLane {
    Deterministic,
    Agent,
    RepoIntegration,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fixture {
    pub name: String,
    pub lane: FixtureLane,
    pub target: JsonObject,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inputs: Option<JsonObject>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env: Option<JsonObject>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<JsonObject>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo: Option<JsonObject>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub execution: Option<JsonObject>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permissions: Option<JsonObject>,
    pub expect: JsonObject,
}

impl RunxSchema for Fixture {
    fn json_schema() -> Value {
        let mut schema = object_schema(
            vec![
                Property::new("name", String::json_schema(), true),
                Property::new("lane", FixtureLane::json_schema(), true),
                Property::new("target", JsonObject::json_schema(), true),
                Property::new("inputs", JsonObject::json_schema(), false),
                Property::new("env", JsonObject::json_schema(), false),
                Property::new("agent", JsonObject::json_schema(), false),
                Property::new("repo", JsonObject::json_schema(), false),
                Property::new("execution", JsonObject::json_schema(), false),
                Property::new("permissions", JsonObject::json_schema(), false),
                Property::new("expect", JsonObject::json_schema(), true),
            ],
            true,
            None,
        );
        if let Some(object) = schema.as_object_mut() {
            object.insert(
                "$schema".to_owned(),
                json!("https://json-schema.org/draft/2020-12/schema"),
            );
            object.insert(
                "$id".to_owned(),
                json!("https://schemas.runx.ai/runx/fixture/v1.json"),
            );
            object.insert("x-runx-schema".to_owned(), json!("runx.fixture.v1"));
        }
        titled(schema, "Fixture")
    }

    fn named_schemas(named: &mut NamedSchemas) {
        if record_named(named, "Fixture", Self::json_schema) {
            FixtureLane::named_schemas(named);
        }
    }
}
