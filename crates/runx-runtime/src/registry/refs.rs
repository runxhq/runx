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

pub fn parse_registry_ref(value: &str) -> ParsedRegistryRef {
    let without_protocol = value
        .strip_prefix("runx://skill/")
        .and_then(|encoded| urlencoding_decode(encoded).ok())
        .unwrap_or_else(|| value.to_owned());
    let without_prefix = without_protocol
        .strip_prefix("registry:")
        .or_else(|| without_protocol.strip_prefix("runx-registry:"))
        .unwrap_or(&without_protocol);
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

fn urlencoding_decode(value: &str) -> Result<String, std::str::Utf8Error> {
    let mut decoded = Vec::with_capacity(value.len());
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let high = hex_value(bytes[index + 1]);
            let low = hex_value(bytes[index + 2]);
            if let (Some(high), Some(low)) = (high, low) {
                decoded.push((high << 4) | low);
                index += 3;
                continue;
            }
        }
        decoded.push(bytes[index]);
        index += 1;
    }
    std::str::from_utf8(&decoded).map(ToOwned::to_owned)
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn non_empty(value: &str) -> Option<String> {
    if value.is_empty() {
        None
    } else {
        Some(value.to_owned())
    }
}
