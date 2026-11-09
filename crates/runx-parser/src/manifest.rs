//! A package's `run.yaml`: its version, the oldest runtime that can run it,
//! how a catalog files it, the inputs its methods share, the connectors only
//! it can pin or choose, and its methods.

use std::collections::BTreeMap;

use runx_contracts::{JsonObject, JsonValue};
use serde::{Deserialize, Serialize};

use crate::skill::{
    CatalogMetadata, ConnectorRequirement, SkillInput, SkillMethodDefinition, bind_operation_steps,
    connector_requirements, validate_catalog_metadata, validate_input_fields,
    validate_method_definition,
};
use crate::{
    ParseError, ValidationError, assert_execution_profile_yaml_subset, json_fields::JsonFieldReader,
};

const FIELDS: JsonFieldReader = JsonFieldReader::new("run.yaml");
const MANIFEST_FIELDS: &[&str] = &[
    "version",
    "minimum_runx",
    "default_method",
    "catalog",
    "shared_inputs",
    "connectors",
    "methods",
];

/// The runx release whose format this parser reads. A package naming a newer
/// `minimum_runx` is refused by name, before anything else of it is read.
pub const RUNX_VERSION: &str = "0.12.0";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RawMethodManifestIr {
    pub document: JsonObject,
    pub raw: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SkillMethodManifest {
    /// The skill's name, from its `SKILL.md`; it names every method's
    /// workflow.
    pub skill: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// The oldest runx that can run the package.
    pub minimum_runx: String,
    /// The method a reference with no `#method` runs, when there's more than
    /// one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_method: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub catalog: Option<CatalogMetadata>,
    /// The connector operations the package's steps name, by requirement.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub connectors: BTreeMap<String, ConnectorRequirement>,
    /// Input definitions several methods take. Each method's inputs already
    /// hold the ones it names, so runtime consumers never resolve them again.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub shared_inputs: BTreeMap<String, SkillInput>,
    pub methods: BTreeMap<String, SkillMethodDefinition>,
    pub raw: RawMethodManifestIr,
}

impl SkillMethodManifest {
    /// The method a reference with no `#method` runs: the package's one
    /// method, or its `default_method`.
    #[must_use]
    pub fn default_method(&self) -> Option<&SkillMethodDefinition> {
        match &self.default_method {
            Some(name) => self.methods.get(name),
            None if self.methods.len() == 1 => self.methods.values().next(),
            None => None,
        }
    }

    /// What `method` says it does, as a host is shown it: its own
    /// `description`, or `package`'s, the `SKILL.md` description, only when
    /// the package has this one method. A package with several methods tells
    /// each apart by its own words, so it never borrows the package's.
    #[must_use]
    pub fn method_description<'a>(
        &self,
        method: &'a SkillMethodDefinition,
        package: Option<&'a str>,
    ) -> Option<&'a str> {
        method
            .description
            .as_deref()
            .or_else(|| package.filter(|_| self.methods.len() == 1))
    }
}

pub fn parse_method_manifest_yaml(yaml: &str) -> Result<RawMethodManifestIr, ParseError> {
    assert_execution_profile_yaml_subset("run.yaml", yaml)?;
    let parsed: JsonValue =
        serde_norway::from_str(yaml).map_err(|error| ParseError::InvalidYaml {
            field: "run.yaml".to_owned(),
            message: error.to_string(),
        })?;
    let JsonValue::Object(document) = parsed else {
        return Err(ParseError::InvalidDocument {
            field: "run.yaml".to_owned(),
            message: "run.yaml must parse to an object.".to_owned(),
        });
    };
    Ok(RawMethodManifestIr {
        document,
        raw: yaml.to_owned(),
    })
}

/// Validate the `run.yaml` of the skill `skill`, the name its `SKILL.md`
/// holds.
pub fn validate_method_manifest(
    skill: &str,
    raw: RawMethodManifestIr,
) -> Result<SkillMethodManifest, ValidationError> {
    // Read before anything else, so a package in a newer format is refused
    // by the runtime it needs, not by a key this one doesn't know.
    let minimum_runx = validate_minimum_runx(raw.document.get("minimum_runx"))?;
    FIELDS.reject_unknown_fields(&raw.document, "run.yaml", MANIFEST_FIELDS)?;
    let shared_inputs_raw = FIELDS
        .optional_object(raw.document.get("shared_inputs"), "shared_inputs")?
        .unwrap_or_default();
    let shared_inputs = validate_input_fields(&shared_inputs_raw, "shared_inputs")?;
    let methods_record = FIELDS.required_object(raw.document.get("methods"), "methods")?;
    if methods_record.is_empty() {
        return Err(FIELDS.validation_error("methods names at least one method."));
    }
    let mut methods = BTreeMap::new();
    for (name, value) in methods_record {
        let JsonValue::Object(method) = value else {
            return Err(FIELDS.validation_error(format!("methods.{name} must be an object.")));
        };
        let inputs = method_inputs(name, method, &shared_inputs_raw)?;
        methods.insert(
            name.clone(),
            validate_method_definition(skill, name, method.clone(), &inputs)?,
        );
    }
    let default_method =
        FIELDS.optional_non_empty_string(raw.document.get("default_method"), "default_method")?;
    match &default_method {
        Some(_) if methods.len() == 1 => {
            return Err(FIELDS.validation_error(
                "default_method has no job in a package with one method: a reference with no #method runs it.",
            ));
        }
        Some(name) if !methods.contains_key(name) => {
            return Err(FIELDS.validation_error(format!(
                "default_method names {name}, which is not one of the package's methods."
            )));
        }
        _ => {}
    }
    let connectors = connector_requirements(raw.document.get("connectors"), &methods)?;
    bind_operation_steps(&mut methods, &connectors)?;
    // A paid method's buyer holds a lease in one of the package's own
    // connectors.
    for (name, method) in &methods {
        if let Some(lease) = method
            .offer
            .as_ref()
            .and_then(|offer| offer.requires_lease.as_ref())
            && !connectors.contains_key(lease.connector.as_str())
        {
            return Err(FIELDS.validation_error(format!(
                "methods.{name}.offer.requires_lease names connector {}, which no step of the package calls.",
                lease.connector
            )));
        }
    }
    let catalog = validate_catalog_metadata(
        FIELDS.optional_object(raw.document.get("catalog"), "catalog")?,
        "catalog",
    )?;
    Ok(SkillMethodManifest {
        skill: skill.to_owned(),
        version: FIELDS.optional_string(raw.document.get("version"), "version")?,
        minimum_runx,
        default_method,
        catalog,
        connectors,
        shared_inputs,
        methods,
        raw,
    })
}

/// `minimum_runx: <major>.<minor>.<patch>`, no newer than this runx.
fn validate_minimum_runx(value: Option<&JsonValue>) -> Result<String, ValidationError> {
    let minimum = FIELDS.required_string(value, "minimum_runx")?;
    let version = |text: &str| -> Option<(u64, u64, u64)> {
        let mut parts = text.split('.').map(|part| {
            (!part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
                .then(|| part.parse::<u64>().ok())
                .flatten()
        });
        let parsed = (parts.next()??, parts.next()??, parts.next()??);
        parts.next().is_none().then_some(parsed)
    };
    let Some(needed) = version(&minimum) else {
        return Err(FIELDS.validation_error(format!(
            "minimum_runx {minimum:?} names a runx release as <major>.<minor>.<patch>."
        )));
    };
    if version(RUNX_VERSION).is_some_and(|this| needed > this) {
        return Err(FIELDS.validation_error(format!(
            "this package needs runx {minimum} or newer, and this is runx {RUNX_VERSION}: upgrade runx to run it."
        )));
    }
    Ok(minimum)
}

/// A method's inputs as written, each one naming a shared input taken from
/// `shared_inputs`, with any key beside it overriding that key.
fn method_inputs(
    method_name: &str,
    method: &JsonObject,
    shared: &JsonObject,
) -> Result<JsonObject, ValidationError> {
    let Some(inputs) = method.get("inputs") else {
        return Ok(JsonObject::new());
    };
    let inputs = FIELDS.required_object(Some(inputs), &format!("methods.{method_name}.inputs"))?;
    let mut resolved = JsonObject::new();
    for (name, declaration) in inputs {
        let field = format!("methods.{method_name}.inputs.{name}");
        let declaration = FIELDS.required_object(Some(declaration), &field)?;
        let mut input = match shared.get(name) {
            Some(JsonValue::Object(definition)) => definition.clone(),
            Some(_) => {
                return Err(
                    FIELDS.validation_error(format!("shared_inputs.{name} must be an object."))
                );
            }
            None => JsonObject::new(),
        };
        input.extend(
            declaration
                .iter()
                .map(|(key, value)| (key.clone(), value.clone())),
        );
        resolved.insert(name.clone(), JsonValue::Object(input));
    }
    Ok(resolved)
}
