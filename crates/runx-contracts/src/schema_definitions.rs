//! The published schema set, with each named contract type defined once.
//!
//! A contract type's own schema holds the types it reaches inline, each
//! carrying its type's name as its `title` (`schema::titled`). Published, every
//! one of those becomes a `$ref` to its single definition: the document of its
//! own when the type has one, and otherwise its entry in the definitions
//! document, which defines every named type that has no document of its own.
//! A reference keeps the description its field gives the type, when the field
//! says something of its own.
//!
//! The in-process schema stays inline: validators, hosts and model providers
//! read [`SchemaArtifact::schema`], which is the published document with every
//! reference expanded.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value};

use crate::schema::NamedSchemas;
use crate::schema_artifacts::SchemaArtifact;

/// The file the definitions document is published as.
pub const DEFINITIONS_FILE: &str = "definitions.schema.json";

/// The `$id` of the definitions document. A named type without a document of
/// its own is `<this>#/$defs/<its name>`.
pub const DEFINITIONS_ID: &str = "https://schemas.runx.ai/runx/definitions/v1.json";

/// The keywords whose value is one schema.
const SCHEMA_KEYWORDS: &[&str] = &[
    "additionalItems",
    "additionalProperties",
    "contains",
    "else",
    "if",
    "items",
    "not",
    "propertyNames",
    "then",
    "unevaluatedItems",
    "unevaluatedProperties",
];

/// The keywords whose value is a list of schemas.
const SCHEMA_LIST_KEYWORDS: &[&str] = &["allOf", "anyOf", "oneOf", "prefixItems"];

/// The keywords whose value maps names to schemas.
const SCHEMA_MAP_KEYWORDS: &[&str] = &[
    "$defs",
    "dependentSchemas",
    "patternProperties",
    "properties",
];

/// The identity a document carries, which a schema inside another never does.
const IDENTITY_KEYWORDS: &[&str] = &["$id", "$schema"];

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SchemaDefinitionError {
    #[error("two contract types are named {0}; rename one")]
    DuplicateName(String),
    #[error("{0} is published twice")]
    DuplicateDocument(String),
    #[error("{file_name} has no $id")]
    MissingId { file_name: &'static str },
    #[error("a schema in {at} is titled {title}, which records no schema")]
    UnknownTitle { at: String, title: String },
    #[error(
        "a schema in {at} is titled {title} but is not its shape; a narrowed schema drops the title"
    )]
    NotItsShape { at: String, title: String },
}

/// The published schema set: each artifact's document, and the definitions
/// document, by file name.
#[derive(Clone, Debug, PartialEq)]
pub struct PublishedSchemas {
    documents: BTreeMap<&'static str, Value>,
}

impl PublishedSchemas {
    /// The document published as `file_name`.
    #[must_use]
    pub fn document(&self, file_name: &str) -> Option<&Value> {
        self.documents.get(file_name)
    }

    /// The exact bytes published as `schemas/<file_name>`. A core packet's
    /// digest is the digest of these bytes, so a sealed packet check names a
    /// schema anyone can fetch and hash.
    pub fn text(&self, file_name: &str) -> Option<Result<String, serde_json::Error>> {
        self.document(file_name).map(document_text)
    }

    /// Every published document, by file name.
    pub fn documents(&self) -> impl Iterator<Item = (&'static str, &Value)> {
        self.documents
            .iter()
            .map(|(name, document)| (*name, document))
    }
}

/// The bytes a published document is written as.
pub fn document_text(document: &Value) -> Result<String, serde_json::Error> {
    serde_json::to_string_pretty(document).map(|text| format!("{text}\n"))
}

/// Publish `artifacts`, each named type defined once.
pub fn publish_schema_artifacts(
    artifacts: &[SchemaArtifact],
) -> Result<PublishedSchemas, SchemaDefinitionError> {
    let named = all_named_schemas(artifacts)?;
    let mut homes = BTreeMap::new();
    for artifact in artifacts {
        let id = artifact.schema.get("$id").and_then(Value::as_str).ok_or(
            SchemaDefinitionError::MissingId {
                file_name: artifact.file_name,
            },
        )?;
        if homes.insert(artifact.type_name, id.to_owned()).is_some() {
            return Err(SchemaDefinitionError::DuplicateDocument(
                artifact.type_name.to_owned(),
            ));
        }
    }

    let mut publisher = Publisher {
        named: &named,
        homes: &homes,
        defined: BTreeSet::new(),
    };
    let mut documents = BTreeMap::new();
    for artifact in artifacts {
        let document = publisher.root(&artifact.schema, artifact.file_name)?;
        if documents.insert(artifact.file_name, document).is_some() {
            return Err(SchemaDefinitionError::DuplicateDocument(
                artifact.file_name.to_owned(),
            ));
        }
    }

    // Each definition is a schema of its own and may reach further types
    // without a document, so define until nothing new is reached.
    let mut definitions = Map::new();
    loop {
        let pending = publisher
            .defined
            .iter()
            .filter(|name| !definitions.contains_key(**name))
            .copied()
            .collect::<Vec<_>>();
        if pending.is_empty() {
            break;
        }
        for name in pending {
            let at = format!("{DEFINITIONS_FILE}#/$defs/{name}");
            let mut definition = publisher.root(&named[name], &at)?;
            if let Value::Object(object) = &mut definition {
                for keyword in IDENTITY_KEYWORDS {
                    object.remove(*keyword);
                }
            }
            definitions.insert(name.to_owned(), definition);
        }
    }
    documents.insert(DEFINITIONS_FILE, definitions_document(definitions));
    Ok(PublishedSchemas { documents })
}

/// Every named type any artifact reaches, each with its own schema.
fn all_named_schemas(artifacts: &[SchemaArtifact]) -> Result<NamedSchemas, SchemaDefinitionError> {
    let mut named = NamedSchemas::new();
    for artifact in artifacts {
        for (name, schema) in &artifact.named_schemas {
            match named.get(name) {
                Some(known) if known != schema => {
                    return Err(SchemaDefinitionError::DuplicateName((*name).to_owned()));
                }
                Some(_) => {}
                None => {
                    named.insert(name, schema.clone());
                }
            }
        }
    }
    Ok(named)
}

fn definitions_document(definitions: Map<String, Value>) -> Value {
    let mut document = Map::new();
    document.insert(
        "$schema".to_owned(),
        Value::String("https://json-schema.org/draft/2020-12/schema".to_owned()),
    );
    document.insert("$id".to_owned(), Value::String(DEFINITIONS_ID.to_owned()));
    document.insert(
        "description".to_owned(),
        Value::String(
            "Every runx contract type that has no document of its own, each defined once. The \
             documents beside this one refer to these definitions."
                .to_owned(),
        ),
    );
    document.insert("$defs".to_owned(), Value::Object(definitions));
    Value::Object(document)
}

struct Publisher<'a> {
    named: &'a NamedSchemas,
    /// The `$id` of the document each type that has one is published as.
    homes: &'a BTreeMap<&'static str, String>,
    /// Each type referenced into the definitions document.
    defined: BTreeSet<&'static str>,
}

impl Publisher<'_> {
    /// A schema whose top is the type it defines: everything below refers to
    /// the types it holds.
    fn root(&mut self, schema: &Value, at: &str) -> Result<Value, SchemaDefinitionError> {
        let Value::Object(object) = schema else {
            return Ok(schema.clone());
        };
        let mut published = Map::new();
        for (keyword, value) in object {
            published.insert(keyword.clone(), self.under(keyword, value, at)?);
        }
        Ok(Value::Object(published))
    }

    /// `value` under `keyword` of a schema: schemas are published, anything
    /// else is data and stays as it is.
    fn under(
        &mut self,
        keyword: &str,
        value: &Value,
        at: &str,
    ) -> Result<Value, SchemaDefinitionError> {
        if SCHEMA_KEYWORDS.contains(&keyword) {
            return self.nested(value, at);
        }
        if SCHEMA_LIST_KEYWORDS.contains(&keyword)
            && let Value::Array(items) = value
        {
            return items
                .iter()
                .map(|item| self.nested(item, at))
                .collect::<Result<Vec<_>, _>>()
                .map(Value::Array);
        }
        if SCHEMA_MAP_KEYWORDS.contains(&keyword)
            && let Value::Object(members) = value
        {
            let mut published = Map::new();
            for (name, member) in members {
                published.insert(name.clone(), self.nested(member, at)?);
            }
            return Ok(Value::Object(published));
        }
        Ok(value.clone())
    }

    /// A schema inside another: a reference when it is a named type, and
    /// otherwise the schema with the types it holds published, without the
    /// identity only a document carries.
    fn nested(&mut self, schema: &Value, at: &str) -> Result<Value, SchemaDefinitionError> {
        let Value::Object(object) = schema else {
            return Ok(schema.clone());
        };
        if let Some(title) = object.get("title").and_then(Value::as_str) {
            return self.reference(object, title, at);
        }
        let mut published = Map::new();
        for (keyword, value) in object {
            if IDENTITY_KEYWORDS.contains(&keyword.as_str()) {
                continue;
            }
            published.insert(keyword.clone(), self.under(keyword, value, at)?);
        }
        Ok(Value::Object(published))
    }

    fn reference(
        &mut self,
        schema: &Map<String, Value>,
        title: &str,
        at: &str,
    ) -> Result<Value, SchemaDefinitionError> {
        let Some((&name, definition)) = self.named.get_key_value(title) else {
            return Err(SchemaDefinitionError::UnknownTitle {
                at: at.to_owned(),
                title: title.to_owned(),
            });
        };
        let Value::Object(definition) = definition else {
            return Err(SchemaDefinitionError::NotItsShape {
                at: at.to_owned(),
                title: title.to_owned(),
            });
        };
        if without_description(schema) != without_description(definition) {
            return Err(SchemaDefinitionError::NotItsShape {
                at: at.to_owned(),
                title: title.to_owned(),
            });
        }
        // A type that holds itself refers to its definition like any other.
        let target = if let Some(home) = self.homes.get(name) {
            home.clone()
        } else {
            self.defined.insert(name);
            format!("{DEFINITIONS_ID}#/$defs/{name}")
        };
        let mut reference = Map::new();
        reference.insert("$ref".to_owned(), Value::String(target));
        if let Some(description) = schema.get("description")
            && definition.get("description") != Some(description)
        {
            reference.insert("description".to_owned(), description.clone());
        }
        Ok(Value::Object(reference))
    }
}

fn without_description(schema: &Map<String, Value>) -> Map<String, Value> {
    let mut shape = schema.clone();
    shape.remove("description");
    shape
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::{Map, Value, json};

    use super::{
        DEFINITIONS_FILE, IDENTITY_KEYWORDS, SCHEMA_KEYWORDS, SCHEMA_LIST_KEYWORDS,
        SCHEMA_MAP_KEYWORDS, publish_schema_artifacts,
    };
    use crate::generated_schema_artifacts;

    /// Every reference in `schema` replaced by what it refers to, expanded in
    /// turn, with the description the reference gives kept.
    fn expand(schema: &Value, documents: &BTreeMap<&str, &Value>) -> Result<Value, String> {
        let Value::Object(object) = schema else {
            return Ok(schema.clone());
        };
        if let Some(Value::String(target)) = object.get("$ref") {
            let (id, pointer) = target.split_once('#').unwrap_or((target, ""));
            let mut definition = documents
                .get(id)
                .ok_or_else(|| format!("{target} names no published document"))?
                .pointer(pointer)
                .ok_or_else(|| format!("{target} is not in its document"))?
                .clone();
            if let Value::Object(definition) = &mut definition {
                // A document's packet marker is its own, not its type's.
                definition.remove("x-runx-packet");
            }
            let mut expanded = expand(&definition, documents)?;
            if let (Value::Object(expanded), Some(description)) =
                (&mut expanded, object.get("description"))
            {
                expanded.insert("description".to_owned(), description.clone());
            }
            return Ok(expanded);
        }
        let mut result = Map::new();
        for (keyword, value) in object {
            let value = match value {
                Value::Array(items) if SCHEMA_LIST_KEYWORDS.contains(&keyword.as_str()) => {
                    Value::Array(
                        items
                            .iter()
                            .map(|item| expand(item, documents))
                            .collect::<Result<_, _>>()?,
                    )
                }
                Value::Object(members) if SCHEMA_MAP_KEYWORDS.contains(&keyword.as_str()) => {
                    let mut expanded = Map::new();
                    for (name, member) in members {
                        expanded.insert(name.clone(), expand(member, documents)?);
                    }
                    Value::Object(expanded)
                }
                _ if SCHEMA_KEYWORDS.contains(&keyword.as_str()) => expand(value, documents)?,
                _ => value.clone(),
            };
            result.insert(keyword.clone(), value);
        }
        Ok(Value::Object(result))
    }

    /// `schema` without the identity a schema inside a document carries.
    fn without_nested_identity(schema: &Value, root: bool) -> Value {
        match schema {
            Value::Object(object) => Value::Object(
                object
                    .iter()
                    .filter(|(keyword, _)| root || !IDENTITY_KEYWORDS.contains(&keyword.as_str()))
                    .map(|(keyword, value)| {
                        let value = if keyword == "const" || keyword == "enum" {
                            value.clone()
                        } else {
                            without_nested_identity(value, false)
                        };
                        (keyword.clone(), value)
                    })
                    .collect(),
            ),
            Value::Array(items) => Value::Array(
                items
                    .iter()
                    .map(|item| without_nested_identity(item, false))
                    .collect(),
            ),
            other => other.clone(),
        }
    }

    /// Sharing definitions changes no contract's meaning: each published
    /// document, with every reference expanded, is exactly the inline schema
    /// its type emits, but for the identity a schema inside a document
    /// carries, which only names it.
    #[test]
    fn every_published_document_expands_to_its_inline_schema() -> Result<(), String> {
        let artifacts = generated_schema_artifacts();
        let published = publish_schema_artifacts(&artifacts).map_err(|error| error.to_string())?;
        let documents = published
            .documents()
            .filter_map(|(_, document)| {
                document
                    .get("$id")
                    .and_then(Value::as_str)
                    .map(|id| (id, document))
            })
            .collect::<BTreeMap<_, _>>();
        for artifact in &artifacts {
            let document = published
                .document(artifact.file_name)
                .ok_or_else(|| format!("{} is not published", artifact.file_name))?;
            assert_eq!(
                without_nested_identity(&expand(document, &documents)?, true),
                without_nested_identity(&artifact.schema, true),
                "{}",
                artifact.file_name
            );
        }
        Ok(())
    }

    /// Each named type is defined once: a type with a document of its own is
    /// never in the definitions document, nothing is defined there that no
    /// document refers to, and no published schema below a document's top
    /// still carries a type's title.
    #[test]
    fn each_named_type_is_defined_once() -> Result<(), String> {
        let artifacts = generated_schema_artifacts();
        let published = publish_schema_artifacts(&artifacts).map_err(|error| error.to_string())?;
        let Some(Value::Object(definitions)) = published
            .document(DEFINITIONS_FILE)
            .and_then(|document| document.get("$defs"))
        else {
            return Err("the definitions document has no $defs".to_owned());
        };
        for artifact in &artifacts {
            assert!(
                !definitions.contains_key(artifact.type_name),
                "{} has a document and a definition",
                artifact.type_name
            );
        }
        let text = published
            .documents()
            .map(|(_, document)| document.to_string())
            .collect::<String>();
        for name in definitions.keys() {
            assert!(
                text.contains(&format!("#/$defs/{name}\"")),
                "{name} is defined but never referred to"
            );
        }
        fn titled_below_top(schema: &Value, top: bool, found: &mut Vec<String>) {
            if let Value::Object(object) = schema {
                if !top && let Some(Value::String(title)) = object.get("title") {
                    found.push(title.clone());
                }
                for (keyword, value) in object {
                    if keyword == "const" || keyword == "enum" {
                        continue;
                    }
                    match value {
                        Value::Object(members) if keyword == "$defs" => {
                            for member in members.values() {
                                titled_below_top(member, true, found);
                            }
                        }
                        _ => titled_below_top(value, false, found),
                    }
                }
            } else if let Value::Array(items) = schema {
                for item in items {
                    titled_below_top(item, false, found);
                }
            }
        }
        for (file_name, document) in published.documents() {
            let mut found = Vec::new();
            titled_below_top(document, true, &mut found);
            assert!(found.is_empty(), "{file_name} still inlines {found:?}");
        }
        Ok(())
    }

    #[test]
    fn a_titled_schema_that_is_not_its_type_is_refused() {
        let mut artifact = crate::schema_artifact::<crate::Reference>("reference.schema.json");
        artifact.schema["properties"]["type"] =
            json!({ "title": "ReferenceType", "type": "string" });
        assert!(publish_schema_artifacts(&[artifact]).is_err());
    }
}
