//! The published schema set as a validator reads it: a published document
//! refers to the definitions it shares with the rest of the set, so each
//! reference resolves to the document beside it that carries that `$id`.

use std::path::PathBuf;

use serde_json::Value;

pub(crate) struct PublishedSchemas {
    pub(crate) dir: PathBuf,
}

impl jsonschema::Retrieve for PublishedSchemas {
    fn retrieve(
        &self,
        uri: &jsonschema::Uri<String>,
    ) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
        let expected = uri.to_string();
        for entry in std::fs::read_dir(&self.dir)? {
            let entry = entry?;
            if entry.path().extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let raw = std::fs::read_to_string(entry.path())?;
            let schema: Value = serde_json::from_str(&raw)?;
            if schema.get("$id").and_then(Value::as_str) == Some(expected.as_str()) {
                return Ok(schema);
            }
        }
        Err(format!("schema reference not found: {expected}").into())
    }
}

/// `oss/schemas`, where the published set is committed.
pub(crate) fn published_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../schemas")
}

/// A validator for `schema`, its references resolved against the published set.
pub(crate) fn published_validator(
    schema: &Value,
) -> Result<jsonschema::Validator, jsonschema::ValidationError<'static>> {
    jsonschema::draft202012::options()
        .with_retriever(PublishedSchemas {
            dir: published_dir(),
        })
        .build(schema)
}
