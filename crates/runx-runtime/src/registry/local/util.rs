use std::fs;
use std::io::{self, Write};
use std::path::Path;

pub(super) use crate::time::now_iso8601;

use super::super::types::{
    RegistryPublisher, RegistrySkillVersion, RegistrySourceMetadata, TrustTier,
};
use super::LocalRegistryError;

pub(super) fn validate_publisher(
    publisher: RegistryPublisher,
    label: &str,
) -> Result<RegistryPublisher, LocalRegistryError> {
    if !matches!(
        publisher.kind.as_str(),
        "organization" | "user" | "team" | "service" | "publisher"
    ) {
        return Err(LocalRegistryError::InvalidVersionPayload {
            field: format!("{label}.kind"),
            message: "must be one of organization, user, team, service, or publisher".to_owned(),
        });
    }
    if publisher.id.is_empty() {
        return Err(LocalRegistryError::InvalidVersionPayload {
            field: format!("{label}.id"),
            message: "must be a non-empty string".to_owned(),
        });
    }
    Ok(publisher)
}

pub(super) fn validate_source_metadata(
    source_metadata: RegistrySourceMetadata,
) -> Result<RegistrySourceMetadata, LocalRegistryError> {
    // The hosted index names the repository host it read; runx does not
    // restrict which.
    if source_metadata.provider.trim().is_empty() {
        return Err(LocalRegistryError::InvalidVersionPayload {
            field: "registry_version.source_metadata.provider".to_owned(),
            message: "must name the repository host".to_owned(),
        });
    }
    if !matches!(
        source_metadata.event.as_str(),
        "enrollment" | "push" | "tag" | "tombstone"
    ) {
        return Err(LocalRegistryError::InvalidVersionPayload {
            field: "registry_version.source_metadata.event".to_owned(),
            message: "must be one of enrollment, push, tag, or tombstone".to_owned(),
        });
    }
    Ok(source_metadata)
}

pub(super) fn required_string(
    value: Option<String>,
    field: &str,
) -> Result<String, LocalRegistryError> {
    match value {
        Some(value) if !value.is_empty() => Ok(value),
        _ => Err(missing_field(field)),
    }
}

pub(super) fn missing_field(field: &str) -> LocalRegistryError {
    LocalRegistryError::InvalidVersionPayload {
        field: field.to_owned(),
        message: "missing required field".to_owned(),
    }
}

pub(super) fn safe_read_dir_names(path: &Path) -> Result<Vec<String>, LocalRegistryError> {
    match fs::read_dir(path) {
        Ok(entries) => entries
            .map(|entry| {
                let entry = entry.map_err(|source| io_error("reading", path, source))?;
                Ok(entry.file_name().to_string_lossy().into_owned())
            })
            .collect(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(source) => Err(io_error("reading", path, source)),
    }
}

pub(super) fn write_registry_json(
    path: &Path,
    version: &RegistrySkillVersion,
    create_new: bool,
) -> Result<(), LocalRegistryError> {
    let mut contents =
        serde_json::to_string_pretty(version).map_err(|source| LocalRegistryError::JsonWrite {
            path: path.to_path_buf(),
            source,
        })?;
    contents.push('\n');

    let mut options = fs::OpenOptions::new();
    options.write(true);
    if create_new {
        options.create_new(true);
    } else {
        options.create(true).truncate(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|source| io_error("writing", path, source))?;
    file.write_all(contents.as_bytes())
        .map_err(|source| io_error("writing", path, source))
}

pub(super) fn io_error(action: &'static str, path: &Path, source: io::Error) -> LocalRegistryError {
    LocalRegistryError::Io {
        action,
        path: path.to_path_buf(),
        source,
    }
}

pub(super) fn encode_part(value: &str) -> String {
    encode_uri_component(value)
}

pub(super) fn decode_part(value: &str) -> Result<String, LocalRegistryError> {
    let decoded =
        percent_decode(value).map_err(|message| LocalRegistryError::InvalidVersionPayload {
            field: "registry_path".to_owned(),
            message,
        })?;
    if is_unsafe_path_component(&decoded) {
        return Err(LocalRegistryError::UnsafePathComponent(decoded));
    }
    Ok(decoded)
}

pub(super) fn is_unsafe_path_component(value: &str) -> bool {
    matches!(value, "." | "..") || value.contains('/') || value.contains('\\')
}

pub(super) fn reject_unsafe_path_component(value: &str) -> Result<(), LocalRegistryError> {
    if is_unsafe_path_component(value) {
        Err(LocalRegistryError::UnsafePathComponent(value.to_owned()))
    } else {
        Ok(())
    }
}

pub(super) fn encode_uri_component(value: &str) -> String {
    let mut output = String::new();
    for byte in value.bytes() {
        let keep = byte.is_ascii_alphanumeric()
            || matches!(
                byte,
                b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')'
            );
        if keep {
            output.push(char::from(byte));
        } else {
            output.push_str(&format!("%{byte:02X}"));
        }
    }
    output
}

pub(super) fn percent_decode(value: &str) -> Result<String, String> {
    let mut decoded = Vec::with_capacity(value.len());
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len() {
                return Err(format!("invalid percent encoding in '{value}'"));
            }
            let Some(high) = hex_value(bytes[index + 1]) else {
                return Err(format!("invalid percent encoding in '{value}'"));
            };
            let Some(low) = hex_value(bytes[index + 2]) else {
                return Err(format!("invalid percent encoding in '{value}'"));
            };
            decoded.push((high << 4) | low);
            index += 3;
            continue;
        }
        decoded.push(bytes[index]);
        index += 1;
    }
    String::from_utf8(decoded).map_err(|error| error.to_string())
}

pub(super) fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

pub(super) fn display_sha256(digest: &str) -> String {
    if digest.starts_with("sha256:") {
        digest.to_owned()
    } else {
        format!("sha256:{digest}")
    }
}

pub(super) fn trust_tier_string(value: &TrustTier) -> &'static str {
    match value {
        TrustTier::FirstParty => "first_party",
        TrustTier::Verified => "verified",
        TrustTier::Community => "community",
    }
}
