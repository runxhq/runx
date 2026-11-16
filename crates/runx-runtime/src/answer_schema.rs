//! The answer a turn's model is offered (rule 1). A turn's answer is checked
//! against its whole contract: its outputs, each packet's schema with its
//! references and runx's own keywords. A model is offered the same answer as
//! one self-contained schema it can read cold: every reference resolved in
//! place (one that recurses kept once under `$defs`), runx's keywords and
//! resource ids left out, a packet wrapped around its output merged into it,
//! and no field required that can hold only one value. runx states such a
//! field itself (`fill`), as it states a packet's id, before it checks the
//! answer; a model that states it at its one value is answered as one that
//! leaves it out, and one that states another is refused.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value as JsonValue;

type JsonObject = serde_json::Map<String, JsonValue>;

/// The keywords that say what a schema means without constraining it: one
/// beside a reference replaces the target's own.
const ANNOTATIONS: &[&str] = &[
    "description",
    "title",
    "examples",
    "default",
    "$comment",
    "deprecated",
    "readOnly",
    "writeOnly",
];

/// The schema keywords whose value is a map of schemas.
const SCHEMA_MAPS: &[&str] = &[
    "properties",
    "patternProperties",
    "$defs",
    "definitions",
    "dependentSchemas",
];
/// The schema keywords whose value is a list of schemas.
const SCHEMA_LISTS: &[&str] = &["allOf", "anyOf", "oneOf", "prefixItems"];
/// The schema keywords whose value is one schema.
const SCHEMA_ONE: &[&str] = &[
    "items",
    "additionalProperties",
    "unevaluatedProperties",
    "unevaluatedItems",
    "not",
    "if",
    "then",
    "else",
    "contains",
    "propertyNames",
    "additionalItems",
];

/// The answer as its model is offered it, from `full`, its whole contract.
#[must_use]
pub(crate) fn offered(full: &runx_contracts::JsonValue) -> runx_contracts::JsonValue {
    let Ok(full) = serde_json::to_value(full) else {
        return full.clone();
    };
    serde_json::from_value(shaped(&full).0).unwrap_or(runx_contracts::JsonValue::Null)
}

/// State in `answer` each field its contract `full` allows one value of and
/// the offered schema left out, where the answer doesn't state it.
pub(crate) fn fill(answer: &mut runx_contracts::JsonValue, full: &runx_contracts::JsonValue) {
    let Ok(full) = serde_json::to_value(full) else {
        return;
    };
    let (_, fills) = shaped(&full);
    let runx_contracts::JsonValue::Object(answer) = answer else {
        return;
    };
    let own = |value: JsonValue| {
        serde_json::from_value::<runx_contracts::JsonValue>(value)
            .unwrap_or(runx_contracts::JsonValue::Null)
    };
    for (field, value) in fills.root {
        answer.entry(field).or_insert_with(|| own(value));
    }
    for (output, fields) in fills.outputs {
        if let Some(runx_contracts::JsonValue::Object(output)) = answer.get_mut(&output) {
            for (field, value) in fields {
                output.entry(field).or_insert_with(|| own(value));
            }
        }
    }
}

/// The fields runx states for a model: the answer's own, and each output's.
#[derive(Default)]
struct Fills {
    root: Vec<(String, JsonValue)>,
    outputs: BTreeMap<String, Vec<(String, JsonValue)>>,
}

fn shaped(full: &JsonValue) -> (JsonValue, Fills) {
    let mut resolver = Resolver::new(full);
    let resolved = resolver.resolve(full, &Base::root());
    let mut schema = clean(&resolved);
    if !resolver.hoisted.is_empty()
        && let JsonValue::Object(root) = &mut schema
    {
        let defs = resolver
            .hoisted
            .iter()
            .map(|(name, def)| (name.clone(), clean(def)))
            .collect::<JsonObject>();
        root.insert("$defs".to_owned(), JsonValue::Object(defs));
    }
    let mut fills = Fills::default();
    if let JsonValue::Object(root) = &mut schema {
        merge_all_of(root);
        fills.root = unrequire_single_valued(root);
        if let Some(JsonValue::Object(properties)) = root.get_mut("properties") {
            for (name, property) in properties.iter_mut() {
                if let JsonValue::Object(property) = property {
                    merge_all_of(property);
                    let dropped = unrequire_single_valued(property);
                    if !dropped.is_empty() {
                        fills.outputs.insert(name.clone(), dropped);
                    }
                }
            }
        }
    }
    (schema, fills)
}

/// Where a reference resolves from: the document it's in, by its `$id`, and
/// that document's root.
#[derive(Clone)]
struct Base {
    id: Option<String>,
}

impl Base {
    fn root() -> Self {
        Self { id: None }
    }
}

/// What resolves the references of one answer: the answer's own document,
/// the packets embedded in it by their `$id`, and the schemas runx
/// publishes.
struct Resolver<'a> {
    root: &'a JsonValue,
    /// The documents embedded in the answer, by their `$id`.
    documents: BTreeMap<String, JsonValue>,
    /// The schemas runx publishes, for a reference to one by its `$id`.
    published: Option<&'static crate::packet_schemas::PublishedSchemaRetriever>,
    /// The references being resolved, outermost first: one met again
    /// recurses.
    open: Vec<String>,
    /// The definitions a recursive reference names, by the name it's kept
    /// under in `$defs`.
    hoisted: BTreeMap<String, JsonValue>,
    names: BTreeMap<String, String>,
}

impl<'a> Resolver<'a> {
    fn new(root: &'a JsonValue) -> Self {
        let mut documents = BTreeMap::new();
        collect_documents(root, &mut documents);
        Self {
            root,
            documents,
            published: crate::packet_schemas::PublishedSchemaRetriever::shared().ok(),
            open: Vec::new(),
            hoisted: BTreeMap::new(),
            names: BTreeMap::new(),
        }
    }

    fn resolve(&mut self, schema: &JsonValue, base: &Base) -> JsonValue {
        let JsonValue::Object(object) = schema else {
            return schema.clone();
        };
        let base = match object.get("$id").and_then(JsonValue::as_str) {
            Some(id) => Base {
                id: Some(id.split('#').next().unwrap_or(id).to_owned()),
            },
            None => base.clone(),
        };
        if let Some(JsonValue::String(reference)) = object.get("$ref") {
            let key = self.key(reference, &base);
            if self.open.contains(&key) {
                let name = self.name(&key);
                return JsonValue::Object(JsonObject::from_iter([(
                    "$ref".to_owned(),
                    JsonValue::String(format!("#/$defs/{name}")),
                )]));
            }
            if let Some((target, target_base)) = self.target(&key) {
                self.open.push(key.clone());
                let resolved = self.resolve(&target, &target_base);
                self.open.pop();
                if let Some(name) = self.names.get(&key).cloned() {
                    self.hoisted.insert(name, resolved.clone());
                }
                // A reference's own keywords beside it say more of what it
                // names: a description is merged in, and a constraint the
                // target also states is kept beside it, both to hold.
                let siblings = object
                    .iter()
                    .filter(|(keyword, _)| keyword.as_str() != "$ref")
                    .map(|(keyword, value)| {
                        (keyword.clone(), self.resolve_keyword(keyword, value, &base))
                    })
                    .collect::<JsonObject>();
                let JsonValue::Object(mut merged) = resolved else {
                    return resolved;
                };
                if siblings.keys().any(|keyword| {
                    merged.contains_key(keyword) && !ANNOTATIONS.contains(&keyword.as_str())
                }) {
                    return JsonValue::Object(JsonObject::from_iter([(
                        "allOf".to_owned(),
                        JsonValue::Array(vec![
                            JsonValue::Object(merged),
                            JsonValue::Object(siblings),
                        ]),
                    )]));
                }
                merged.extend(siblings);
                return JsonValue::Object(merged);
            }
        }
        JsonValue::Object(
            object
                .iter()
                .map(|(keyword, value)| {
                    (keyword.clone(), self.resolve_keyword(keyword, value, &base))
                })
                .collect(),
        )
    }

    fn resolve_keyword(&mut self, keyword: &str, value: &JsonValue, base: &Base) -> JsonValue {
        match value {
            JsonValue::Object(map) if SCHEMA_MAPS.contains(&keyword) => JsonValue::Object(
                map.iter()
                    .map(|(name, schema)| (name.clone(), self.resolve(schema, base)))
                    .collect(),
            ),
            JsonValue::Array(list) if SCHEMA_LISTS.contains(&keyword) => JsonValue::Array(
                list.iter()
                    .map(|schema| self.resolve(schema, base))
                    .collect(),
            ),
            JsonValue::Object(_) if SCHEMA_ONE.contains(&keyword) => self.resolve(value, base),
            other => other.clone(),
        }
    }

    /// A reference as the document and pointer it names.
    fn key(&self, reference: &str, base: &Base) -> String {
        let (document, pointer) = reference.split_once('#').unwrap_or((reference, ""));
        let document = if document.is_empty() {
            base.id.clone().unwrap_or_default()
        } else {
            document.to_owned()
        };
        format!("{document}#{pointer}")
    }

    fn target(&self, key: &str) -> Option<(JsonValue, Base)> {
        let (document, pointer) = key.split_once('#')?;
        let root = if document.is_empty() {
            self.root
        } else {
            match self.documents.get(document) {
                Some(root) => root,
                None => self.published?.document(document)?,
            }
        };
        let target = if pointer.is_empty() {
            root
        } else {
            root.pointer(pointer)?
        };
        Some((
            target.clone(),
            Base {
                id: (!document.is_empty()).then(|| document.to_owned()),
            },
        ))
    }

    /// The name a recursive reference's definition is kept under.
    fn name(&mut self, key: &str) -> String {
        if let Some(name) = self.names.get(key) {
            return name.clone();
        }
        let tail = key
            .rsplit('/')
            .next()
            .filter(|tail| !tail.is_empty() && !tail.contains('#'))
            .unwrap_or("definition");
        let taken = self.names.values().cloned().collect::<BTreeSet<_>>();
        let mut name = tail.to_owned();
        let mut suffix = 1;
        while taken.contains(&name) {
            suffix += 1;
            name = format!("{tail}_{suffix}");
        }
        self.names.insert(key.to_owned(), name.clone());
        name
    }
}

/// Each embedded document of `schema`, by its `$id`.
fn collect_documents(schema: &JsonValue, documents: &mut BTreeMap<String, JsonValue>) {
    match schema {
        JsonValue::Object(object) => {
            if let Some(JsonValue::String(id)) = object.get("$id") {
                documents
                    .entry(id.split('#').next().unwrap_or(id).to_owned())
                    .or_insert_with(|| schema.clone());
            }
            for value in object.values() {
                collect_documents(value, documents);
            }
        }
        JsonValue::Array(list) => list
            .iter()
            .for_each(|value| collect_documents(value, documents)),
        _ => {}
    }
}

/// `schema` without what only runx reads: resource ids, the dialect, runx's
/// own keywords, and the definitions its references were resolved from.
fn clean(schema: &JsonValue) -> JsonValue {
    let JsonValue::Object(object) = schema else {
        return schema.clone();
    };
    JsonValue::Object(
        object
            .iter()
            .filter(|(keyword, _)| {
                !matches!(
                    keyword.as_str(),
                    "$id" | "$schema" | "$comment" | "$defs" | "definitions" | "title"
                ) && !keyword.starts_with("x-runx")
            })
            .map(|(keyword, value)| {
                let value = match value {
                    JsonValue::Object(map) if SCHEMA_MAPS.contains(&keyword.as_str()) => {
                        JsonValue::Object(
                            map.iter()
                                .map(|(name, schema)| (name.clone(), clean(schema)))
                                .collect(),
                        )
                    }
                    JsonValue::Array(list) if SCHEMA_LISTS.contains(&keyword.as_str()) => {
                        JsonValue::Array(list.iter().map(clean).collect())
                    }
                    JsonValue::Object(_) if SCHEMA_ONE.contains(&keyword.as_str()) => clean(value),
                    other => other.clone(),
                };
                (keyword.clone(), value)
            })
            .collect(),
    )
}

/// Merge into `schema` each member of its `allOf` that describes the same
/// object: its fields, what it requires, and whether it admits others. A
/// member that is no plain object schema stays in `allOf`.
fn merge_all_of(schema: &mut JsonObject) {
    let Some(JsonValue::Array(members)) = schema.remove("allOf") else {
        return;
    };
    let mut kept = Vec::new();
    for member in members {
        let JsonValue::Object(mut member) = member else {
            kept.push(member);
            continue;
        };
        merge_all_of(&mut member);
        let plain = member.keys().all(|keyword| {
            matches!(
                keyword.as_str(),
                "type" | "properties" | "required" | "additionalProperties" | "description"
            )
        }) && member
            .get("type")
            .is_none_or(|kind| kind.as_str() == Some("object"))
            && schema
                .get("type")
                .is_none_or(|kind| kind.as_str() == Some("object"));
        if !plain {
            kept.push(JsonValue::Object(member));
            continue;
        }
        for (keyword, value) in member {
            match (keyword.as_str(), value) {
                ("properties", JsonValue::Object(properties)) => {
                    let JsonValue::Object(into) = schema
                        .entry("properties".to_owned())
                        .or_insert_with(|| JsonValue::Object(JsonObject::new()))
                    else {
                        continue;
                    };
                    for (name, property) in properties {
                        match into.get_mut(&name) {
                            // A field both state is the narrower of the two:
                            // a member's schema says more than a bare type of
                            // its own kind. Kinds that differ both hold.
                            Some(JsonValue::Object(existing))
                                if existing.keys().all(|keyword| {
                                    matches!(keyword.as_str(), "type" | "description")
                                }) && existing.get("type").is_none_or(|kind| {
                                    property.get("type").is_none_or(|other| other == kind)
                                }) =>
                            {
                                let description = existing.get("description").cloned();
                                let mut property = property;
                                if let (Some(description), JsonValue::Object(property)) =
                                    (description, &mut property)
                                {
                                    property
                                        .entry("description".to_owned())
                                        .or_insert(description);
                                }
                                into.insert(name, property);
                            }
                            Some(existing) => {
                                let both = JsonValue::Array(vec![existing.clone(), property]);
                                *existing = JsonValue::Object(JsonObject::from_iter([(
                                    "allOf".to_owned(),
                                    both,
                                )]));
                            }
                            None => {
                                into.insert(name, property);
                            }
                        }
                    }
                }
                ("required", JsonValue::Array(required)) => {
                    let JsonValue::Array(into) = schema
                        .entry("required".to_owned())
                        .or_insert_with(|| JsonValue::Array(Vec::new()))
                    else {
                        continue;
                    };
                    for field in required {
                        if !into.contains(&field) {
                            into.push(field);
                        }
                    }
                }
                ("additionalProperties", JsonValue::Bool(false)) => {
                    schema.insert("additionalProperties".to_owned(), JsonValue::Bool(false));
                }
                ("description", description) => {
                    schema
                        .entry("description".to_owned())
                        .or_insert(description);
                }
                ("type", kind) => {
                    schema.entry("type".to_owned()).or_insert(kind);
                }
                _ => {}
            }
        }
    }
    if !kept.is_empty() {
        schema.insert("allOf".to_owned(), JsonValue::Array(kept));
    }
}

/// Require of `schema` no field that can hold only one value, which runx
/// states: its name and its value. The field stays, at its one value, so an
/// answer that states it rightly is not refused for it.
fn unrequire_single_valued(schema: &mut JsonObject) -> Vec<(String, JsonValue)> {
    let Some(JsonValue::Object(properties)) = schema.get_mut("properties") else {
        return Vec::new();
    };
    let single = properties
        .iter()
        .filter_map(|(name, property)| Some((name.clone(), only_value(property)?)))
        .collect::<Vec<_>>();
    if let Some(JsonValue::Array(required)) = schema.get_mut("required") {
        required.retain(|field| {
            !field
                .as_str()
                .is_some_and(|field| single.iter().any(|(name, _)| name == field))
        });
    }
    single
}

/// The one value `schema` allows, when it allows one.
fn only_value(schema: &JsonValue) -> Option<JsonValue> {
    let schema = schema.as_object()?;
    if let Some(value) = schema.get("const") {
        return Some(value.clone());
    }
    match schema.get("enum") {
        Some(JsonValue::Array(values)) if values.len() == 1 => values.first().cloned(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{fill, offered};
    use runx_contracts::JsonValue;

    fn json(value: serde_json::Value) -> JsonValue {
        serde_json::from_value(value).unwrap_or(JsonValue::Null)
    }

    fn at(value: &JsonValue, pointer: &str) -> Option<serde_json::Value> {
        serde_json::to_value(value).ok()?.pointer(pointer).cloned()
    }

    /// A packet's schema as a model is offered it: one plain object, its
    /// references in place, one that recurses kept once, nothing of runx's,
    /// and no field it can hold only one value of, which runx states.
    #[test]
    fn a_packet_is_offered_as_one_plain_schema() {
        let full = json(serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["draft"],
            "properties": {
                "draft": {
                    "type": "object",
                    "description": "The draft.",
                    "allOf": [
                        {"type": "object", "description": "The draft."},
                        {
                            "$id": "https://example.test/draft.json",
                            "$schema": "https://json-schema.org/draft/2020-12/schema",
                            "x-runx-packet": true,
                            "title": "Draft",
                            "type": "object",
                            "additionalProperties": false,
                            "required": ["schema", "tree"],
                            "properties": {
                                "schema": {"const": "acme.draft.v1"},
                                "tree": {"$ref": "#/$defs/Node"}
                            },
                            "$defs": {
                                "Node": {
                                    "type": "object",
                                    "properties": {
                                        "name": {"$ref": "#/$defs/Name"},
                                        "children": {"type": "array", "items": {"$ref": "#/$defs/Node"}}
                                    }
                                },
                                "Name": {"type": "string", "minLength": 1}
                            }
                        }
                    ]
                }
            }
        }));
        let schema = offered(&full);
        let text = serde_json::to_string(&schema).unwrap_or_default();
        for absent in ["$id", "$schema", "x-runx", "allOf", "title"] {
            assert!(!text.contains(absent), "{absent} in {text}");
        }
        assert_eq!(
            at(&schema, "/properties/draft/properties/schema"),
            Some(serde_json::json!({"const": "acme.draft.v1"}))
        );
        assert_eq!(
            at(&schema, "/properties/draft/required"),
            Some(serde_json::json!(["tree"]))
        );
        assert_eq!(
            at(&schema, "/properties/draft/properties/tree/properties/name"),
            Some(serde_json::json!({"type": "string", "minLength": 1}))
        );
        assert_eq!(
            at(
                &schema,
                "/properties/draft/properties/tree/properties/children/items"
            ),
            Some(serde_json::json!({"$ref": "#/$defs/Node"}))
        );
        assert!(
            at(&schema, "/$defs/Node/properties/name").is_some(),
            "{text}"
        );

        let mut answer = json(serde_json::json!({"draft": {"tree": {"name": "root"}}}));
        fill(&mut answer, &full);
        assert_eq!(
            at(&answer, "/draft/schema"),
            Some(serde_json::json!("acme.draft.v1"))
        );
    }

    /// A reference's own constraint beside it holds with the target's:
    /// neither replaces the other.
    #[test]
    fn a_constraint_beside_a_reference_holds_with_its_target() {
        let full = json(serde_json::json!({
            "type": "object",
            "properties": {
                "pair": {"$ref": "#/$defs/Pair", "required": ["b"], "description": "Both."}
            },
            "$defs": {
                "Pair": {"type": "object", "required": ["a"], "properties": {"a": {}, "b": {}}}
            }
        }));
        let schema = offered(&full);
        assert_eq!(
            at(&schema, "/properties/pair/required"),
            Some(serde_json::json!(["a", "b"]))
        );
        assert_eq!(
            at(&schema, "/properties/pair/description"),
            Some(serde_json::json!("Both."))
        );
    }
}
