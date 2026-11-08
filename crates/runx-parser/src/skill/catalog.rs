//! How a catalog files a package: whether it's listed and exported, the
//! registry's browsing category, and the tags it's searched by.
use std::collections::BTreeSet;

use runx_contracts::JsonObject;
use serde::{Deserialize, Serialize};

use crate::ValidationError;

use super::FIELDS;

const CATALOG_FIELDS: &[&str] = &["visibility", "category", "tags"];

/// Whether a package is listed, exported to hosts and publishable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogVisibility {
    Public,
    Internal,
}

impl CatalogVisibility {
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            CatalogVisibility::Public => "public",
            CatalogVisibility::Internal => "internal",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogMetadata {
    pub visibility: CatalogVisibility,
    /// The registry category the skill declares: a skill category of the
    /// registry taxonomy (`registry/taxonomy.json`), checked here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

pub(crate) fn validate_catalog_metadata(
    value: Option<JsonObject>,
    label: &str,
) -> Result<Option<CatalogMetadata>, ValidationError> {
    let Some(value) = value else {
        return Ok(None);
    };
    FIELDS.reject_unknown_fields(&value, label, CATALOG_FIELDS)?;
    let visibility = match FIELDS
        .optional_string(value.get("visibility"), &format!("{label}.visibility"))?
        .as_deref()
    {
        Some("public") | None => CatalogVisibility::Public,
        Some("internal") => CatalogVisibility::Internal,
        Some(_) => {
            return Err(
                FIELDS.validation_error(format!("{label}.visibility must be public or internal."))
            );
        }
    };
    let category =
        FIELDS.optional_non_empty_string(value.get("category"), &format!("{label}.category"))?;
    if let Some(category) = &category {
        runx_contracts::validate_registry_category(
            runx_contracts::TaxonomySubject::Skill,
            category,
        )
        .map_err(|reason| FIELDS.validation_error(format!("{label}.category: {reason}")))?;
    }
    Ok(Some(CatalogMetadata {
        visibility,
        category,
        tags: parse_catalog_tags(&value, label)?,
    }))
}

fn parse_catalog_tags(value: &JsonObject, label: &str) -> Result<Vec<String>, ValidationError> {
    let field = format!("{label}.tags");
    let tags = FIELDS
        .optional_string_array(value.get("tags"), &field)?
        .unwrap_or_default();
    let mut seen = BTreeSet::new();
    for tag in &tags {
        if tag.trim().is_empty() || tag.trim() != tag {
            return Err(FIELDS.validation_error(format!(
                "{field} entries must be non-empty and carry no surrounding whitespace."
            )));
        }
        if !seen.insert(tag.as_str()) {
            return Err(FIELDS.validation_error(format!("{field} repeats '{tag}'.")));
        }
    }
    Ok(tags)
}
