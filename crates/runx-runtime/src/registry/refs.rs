use crate::http::RuntimeHttpTransport;

use super::http::{RegistryClient, RegistryClientError};
use super::types::ResolvedRegistryRef;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedRegistryRef {
    pub skill_id: String,
    pub version: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum RegistryResolveError {
    #[error("{0}")]
    Client(#[from] RegistryClientError),
    #[error("Registry ref '{0}' is ambiguous. Use '<owner>/<name>' instead.")]
    Ambiguous(String),
}

/// A registry ref, `registry:<owner>/<name>[@<version>]`, or the same
/// without its prefix where a caller already knows it names the registry.
pub fn parse_registry_ref(value: &str) -> ParsedRegistryRef {
    let without_prefix = runx_parser::registry_ref(value).unwrap_or(value);
    let Some(at_index) = without_prefix.rfind('@') else {
        return ParsedRegistryRef {
            skill_id: without_prefix.to_owned(),
            version: None,
        };
    };
    if at_index == 0 {
        return ParsedRegistryRef {
            skill_id: without_prefix.to_owned(),
            version: None,
        };
    }
    ParsedRegistryRef {
        skill_id: without_prefix[..at_index].to_owned(),
        version: non_empty(&without_prefix[at_index + 1..]),
    }
}

pub fn resolve_remote_registry_ref<T: RuntimeHttpTransport>(
    client: &RegistryClient<T>,
    registry_ref: &str,
    version_override: Option<&str>,
) -> Result<Option<ResolvedRegistryRef>, RegistryResolveError> {
    let parsed = parse_registry_ref(registry_ref);
    if parsed.skill_id.contains('/') {
        return Ok(Some(ResolvedRegistryRef {
            skill_id: parsed.skill_id,
            version: version_override.map(ToOwned::to_owned).or(parsed.version),
        }));
    }

    let normalized = parsed.skill_id.trim().to_lowercase();
    let matches = client
        .search_with_limit(&parsed.skill_id, 100)?
        .into_iter()
        .filter(|candidate| candidate.name == normalized)
        .collect::<Vec<_>>();
    match matches.len() {
        0 => Ok(None),
        1 => {
            let candidate = &matches[0];
            Ok(Some(ResolvedRegistryRef {
                skill_id: candidate.skill_id.clone(),
                version: version_override
                    .map(ToOwned::to_owned)
                    .or(parsed.version)
                    .or_else(|| candidate.version.clone()),
            }))
        }
        _ => Err(RegistryResolveError::Ambiguous(parsed.skill_id)),
    }
}

pub fn safe_skill_package_parts(
    registry_ref: &str,
    skill_name: &str,
    resolved_version: Option<&str>,
) -> Vec<String> {
    let normalized = normalize_install_ref(registry_ref);
    let parsed = parse_registry_ref(registry_ref);
    let version = parsed.version.as_deref().or(resolved_version);
    let raw_parts = if normalized.contains('/') {
        let mut parts = normalized.split('/').collect::<Vec<_>>();
        if let Some(version) = version {
            parts.push(version);
        }
        parts
    } else {
        let mut parts = vec![skill_name];
        if let Some(version) = version {
            parts.push(version);
        }
        parts
    };
    let parts = raw_parts
        .into_iter()
        .map(safe_path_part)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if parts.is_empty() {
        vec![safe_path_part(skill_name)]
    } else {
        parts
    }
}

fn normalize_install_ref(registry_ref: &str) -> String {
    let parsed = parse_registry_ref(registry_ref);
    parsed.skill_id
}

fn safe_path_part(value: &str) -> String {
    let mut output = String::new();
    let mut last_dash = false;
    for ch in value.trim().to_lowercase().chars() {
        let keep = ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-');
        if keep {
            output.push(ch);
            last_dash = false;
        } else if !last_dash {
            output.push('-');
            last_dash = true;
        }
    }
    let trimmed = output.trim_matches('-').to_owned();
    if trimmed.is_empty() || trimmed == "." || trimmed == ".." {
        "skill".to_owned()
    } else {
        trimmed
    }
}

fn non_empty(value: &str) -> Option<String> {
    if value.is_empty() {
        None
    } else {
        Some(value.to_owned())
    }
}
