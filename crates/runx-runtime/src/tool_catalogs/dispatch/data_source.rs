use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use runx_contracts::{JsonObject, JsonValue, sha256_hex};

use super::{string_input, workspace_root};
const RUNX_DATA_SOURCES_ENV: &str = "RUNX_DATA_SOURCES";
const PROJECT_DATA_SOURCES_PATH: &str = ".runx/data-sources.json";

pub(super) struct Target {
    pub tool_ref: String,
    pub binding: JsonObject,
    pub operation: &'static str,
}

#[derive(Clone, Debug)]
struct ConfigSource {
    value: String,
    required: bool,
}

/// Resolve one typed data operation to its operator-owned storage binding.
/// Native adapters keep the typed operation target; tool-backed data adapters
/// receive the same operation through their supervised tool boundary.
pub(super) fn target(
    tool_ref: &str,
    inputs: &JsonObject,
    env: &BTreeMap<String, String>,
    skill_directory: &Path,
) -> Result<Option<Target>, String> {
    let Some(operation) = crate::tool_catalogs::native::data_operation_name(tool_ref) else {
        return Ok(None);
    };

    let data_source_ref = string_input(inputs, "data_source_ref")
        .ok_or_else(|| format!("{tool_ref} requires input data_source_ref."))?
        .to_owned();
    let binding = match binding(&data_source_ref, env, skill_directory)? {
        Some(binding) => binding,
        None if data_source_ref.starts_with("local://") => default_local_binding(&data_source_ref),
        None => {
            return Err(format!(
                "Data source '{data_source_ref}' is not bound to a data adapter. Add it to {PROJECT_DATA_SOURCES_PATH} or set {RUNX_DATA_SOURCES_ENV}."
            ));
        }
    };

    let adapter = string_input(&binding, "adapter")
        .ok_or_else(|| format!("Data source '{data_source_ref}' binding is missing adapter."))?;
    if crate::tool_catalogs::native::data_operation_name(adapter).is_some() {
        return Err(format!(
            "Data source '{data_source_ref}' cannot bind to operation capability '{adapter}'; choose a concrete adapter."
        ));
    }
    if !adapter.contains('.') {
        return Err(format!(
            "Data source '{data_source_ref}' adapter '{adapter}' must be a namespaced tool ref such as data.redis."
        ));
    }
    let target_ref = if crate::tool_catalogs::native::is_native_data_adapter(adapter) {
        tool_ref
    } else {
        adapter
    };
    Ok(Some(Target {
        tool_ref: target_ref.to_owned(),
        binding,
        operation,
    }))
}

fn default_local_binding(data_source_ref: &str) -> JsonObject {
    let mut object = JsonObject::new();
    object.insert(
        "data_source_ref".to_owned(),
        JsonValue::String(data_source_ref.to_owned()),
    );
    let source_digest = sha256_hex(data_source_ref.as_bytes());
    let source_id = &source_digest[..16];
    object.insert(
        "adapter".to_owned(),
        JsonValue::String("data.sqlite".to_owned()),
    );
    object.insert(
        "profile".to_owned(),
        JsonValue::String("local-durable".to_owned()),
    );
    object.insert(
        "database_path".to_owned(),
        JsonValue::String(format!(
            ".runx/data/local-sources/source-{source_id}.sqlite"
        )),
    );
    object.insert(
        "storage_class".to_owned(),
        JsonValue::String("sqlite".to_owned()),
    );
    object.insert("resources".to_owned(), JsonValue::Object(JsonObject::new()));
    object
}

fn binding(
    data_source_ref: &str,
    env: &BTreeMap<String, String>,
    skill_directory: &Path,
) -> Result<Option<JsonObject>, String> {
    for source in config_sources(env, skill_directory) {
        let Some(document) = read_config_source(&source)? else {
            continue;
        };
        let parsed: JsonValue = serde_json::from_str(&document).map_err(|error| {
            format!(
                "Data source config {} is not valid JSON: {error}",
                source.value
            )
        })?;
        let Some(binding) = binding_from_config(&parsed, data_source_ref) else {
            continue;
        };
        reject_secret_material(&binding, data_source_ref)?;
        return Ok(Some(binding));
    }
    Ok(None)
}

fn config_sources(env: &BTreeMap<String, String>, skill_directory: &Path) -> Vec<ConfigSource> {
    let mut sources = Vec::new();
    let root = workspace_root(env, skill_directory);
    if let Some(config) = env.get(RUNX_DATA_SOURCES_ENV) {
        let trimmed = config.trim();
        if !trimmed.is_empty() {
            let value = if trimmed.starts_with('{') || Path::new(trimmed).is_absolute() {
                trimmed.to_owned()
            } else {
                root.join(trimmed).to_string_lossy().into_owned()
            };
            sources.push(ConfigSource {
                value,
                required: true,
            });
        }
    }
    sources.push(ConfigSource {
        value: root
            .join(PROJECT_DATA_SOURCES_PATH)
            .to_string_lossy()
            .into_owned(),
        required: false,
    });
    sources
}

fn read_config_source(source: &ConfigSource) -> Result<Option<String>, String> {
    let trimmed = source.value.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if trimmed.starts_with('{') {
        return Ok(Some(trimmed.to_owned()));
    }
    match fs::read_to_string(trimmed) {
        Ok(document) => Ok(Some(document)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && !source.required => Ok(None),
        Err(error) => Err(format!(
            "Failed to read data source config {trimmed}: {error}"
        )),
    }
}

fn binding_from_config(config: &JsonValue, data_source_ref: &str) -> Option<JsonObject> {
    let JsonValue::Object(root) = config else {
        return None;
    };
    let JsonValue::Object(sources) = root.get("data_sources")? else {
        return None;
    };
    let JsonValue::Object(binding) = sources.get(data_source_ref)? else {
        return None;
    };
    let mut normalized = binding.clone();
    normalized.insert(
        "data_source_ref".to_owned(),
        JsonValue::String(data_source_ref.to_owned()),
    );
    Some(normalized)
}

fn reject_secret_material(binding: &JsonObject, data_source_ref: &str) -> Result<(), String> {
    let Some(key) =
        crate::credentials::first_unregistered_secret_field(&JsonValue::Object(binding.clone()))
    else {
        return Ok(());
    };
    Err(format!(
        "Data source '{data_source_ref}' binding contains secret-like field '{key}'. Put provider credentials behind a runx credential profile or hosted grant instead."
    ))
}
