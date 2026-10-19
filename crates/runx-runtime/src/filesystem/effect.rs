//! What applying a text bundle would change, before it is applied: the edit
//! its approval shows.

use std::fs;
use std::path::Path;

use runx_contracts::{JsonObject, JsonValue, sha256_prefixed};

use super::admission::admit_text_bundle;
use super::{LocalEffect, TextBundle};
use crate::RuntimeError;
use crate::services::WorkspaceBounds;

/// What applying `bundle` beneath `root` would change, its paths admitted
/// exactly as applying it admits them: each file with the digest of what it
/// replaces (none when it is created), the digest of what it becomes (none
/// when it is deleted), and a unified diff between them. An empty bundle
/// changes nothing.
pub(crate) fn text_bundle_effect(
    operation: &str,
    root: &Path,
    bundle: &TextBundle,
    bounds: WorkspaceBounds,
) -> Result<Option<LocalEffect>, RuntimeError> {
    if bundle.writes.is_empty() && bundle.deletes.is_empty() {
        return Ok(None);
    }
    let admitted = admit_text_bundle(operation, root, bundle, bounds)?;
    let mut files = Vec::new();
    let mut paths = Vec::new();
    let changes = admitted
        .writes
        .iter()
        .map(|write| {
            (
                &write.relative,
                &write.absolute,
                Some(write.contents.as_str()),
            )
        })
        .chain(
            admitted
                .deletes
                .iter()
                .map(|(relative, absolute)| (relative, absolute, None)),
        );
    for (relative, absolute, after) in changes {
        let before = match fs::read(absolute) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(source) => {
                return Err(RuntimeError::io(
                    format!("reading {} before it changes", absolute.display()),
                    source,
                ));
            }
        };
        let mut file = JsonObject::from([("path".to_owned(), JsonValue::String(relative.clone()))]);
        if let Some(before) = &before {
            file.insert(
                "before_digest".to_owned(),
                JsonValue::String(sha256_prefixed(before)),
            );
        }
        if let Some(after) = after {
            file.insert(
                "after_digest".to_owned(),
                JsonValue::String(sha256_prefixed(after.as_bytes())),
            );
        }
        file.insert(
            "diff".to_owned(),
            JsonValue::String(unified_diff(relative, before.as_deref(), after)),
        );
        files.push(JsonValue::Object(file));
        paths.push(relative.clone());
    }
    Ok(Some(LocalEffect {
        payload: JsonValue::Object(JsonObject::from([
            ("kind".to_owned(), JsonValue::String("edit".to_owned())),
            ("files".to_owned(), JsonValue::Array(files)),
        ])),
        paths,
        argv: None,
    }))
}

/// The unified diff from what a file held (`None` when it did not exist) to
/// what it becomes (`None` when it is deleted), as an approval shows it and
/// the applied change returns it.
pub(crate) fn unified_diff(relative: &str, before: Option<&[u8]>, after: Option<&str>) -> String {
    let before = before.map(String::from_utf8_lossy).unwrap_or_default();
    similar::TextDiff::from_lines(before.as_ref(), after.unwrap_or_default())
        .unified_diff()
        .context_radius(3)
        .header(relative, relative)
        .to_string()
}

#[cfg(test)]
mod tests;
