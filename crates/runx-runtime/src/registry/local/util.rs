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

    // Replacing a directory entry must not bypass the record's own write
    // access. Check without truncating, so a refusal leaves its bytes intact.
    if !create_new {
        match fs::OpenOptions::new().write(true).open(path) {
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(io_error("writing", path, error)),
        }
    }

    // The record is written whole beside its path, then put in place in one
    // step, so a reader sees the old record or the new one and never a mix;
    // a failed write leaves the published record as it was.
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let staged = tempfile::Builder::new()
        .prefix(".runx-registry-")
        .suffix(".json.tmp")
        .make_in(parent, |staged_path| {
            let mut options = fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            options.open(staged_path)
        })
        .map_err(|source| io_error("staging", path, source))?;
    staged
        .as_file()
        .write_all(contents.as_bytes())
        .and_then(|()| staged.as_file().sync_all())
        .map_err(|source| io_error("writing", path, source))?;
    if create_new {
        // A hard link publishes the record only if nothing holds the path.
        fs::hard_link(staged.path(), path).map_err(|source| io_error("writing", path, source))
    } else {
        // Rust's rename supports replacing an open Windows destination;
        // tempfile::persist uses MoveFileExW alone. Keep the cleanup guard,
        // close the staged handle, and use the same primitive as our other
        // atomic writers. The custom creator above uses normal Windows file
        // attributes so the published record is never marked temporary.
        let staged_path = staged.into_temp_path();
        fs::rename(&staged_path, path).map_err(|source| io_error("writing", path, source))
    }
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

fn encode_uri_component(value: &str) -> String {
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

#[cfg(test)]
mod tests {
    use std::io::Read;

    use super::*;

    fn version(description: &str) -> Result<RegistrySkillVersion, serde_json::Error> {
        serde_json::from_value(serde_json::json!({
            "skill_id": "acme/echo",
            "publisher": "acme",
            "name": "echo",
            "description": description,
            "version": "1.0.0",
            "digest": "sha256:0000000000000000000000000000000000000000000000000000000000000000",
            "markdown": "# Echo\n",
            "runner_names": [],
            "trust_tier": "community",
            "required_scopes": [],
            "tags": [],
            "publisher": { "kind": "organization", "id": "acme" },
            "created_at": "2026-10-13T00:00:00Z",
            "updated_at": "2026-10-13T00:00:00Z",
        }))
    }

    /// A reader part way through a record finishes reading the record it
    /// opened, whole, while the record is replaced.
    #[test]
    fn a_replaced_record_never_tears_under_a_reader() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let path = root.path().join("1.0.0.json");
        write_registry_json(&path, &version("short")?, true)?;

        let mut reader = fs::File::open(&path)?;
        let mut head = [0_u8; 16];
        reader.read_exact(&mut head)?;
        write_registry_json(
            &path,
            &version(&"a much longer description ".repeat(64))?,
            false,
        )?;
        let mut rest = Vec::new();
        reader.read_to_end(&mut rest)?;

        let read = [head.as_slice(), rest.as_slice()].concat();
        let record: RegistrySkillVersion = serde_json::from_slice(&read)?;
        assert_eq!(record.description.as_deref(), Some("short"));
        let current: RegistrySkillVersion = serde_json::from_slice(&fs::read(&path)?)?;
        assert!(
            current
                .description
                .is_some_and(|text| text.starts_with("a much longer"))
        );
        Ok(())
    }

    #[test]
    fn a_new_record_is_refused_when_one_is_already_published()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let path = root.path().join("1.0.0.json");
        write_registry_json(&path, &version("first")?, true)?;
        assert!(write_registry_json(&path, &version("second")?, true).is_err());
        let current: RegistrySkillVersion = serde_json::from_slice(&fs::read(&path)?)?;
        assert_eq!(current.description.as_deref(), Some("first"));
        let leftovers = fs::read_dir(root.path())?.count();
        assert_eq!(leftovers, 1, "no staged file is left behind");
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn published_records_are_not_marked_as_windows_temporary_files()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::os::windows::fs::MetadataExt;

        const FILE_ATTRIBUTE_TEMPORARY: u32 = 0x100;
        let root = tempfile::tempdir()?;
        let path = root.path().join("1.0.0.json");
        for create_new in [true, false] {
            write_registry_json(&path, &version("published")?, create_new)?;
            assert_eq!(
                fs::metadata(&path)?.file_attributes() & FILE_ATTRIBUTE_TEMPORARY,
                0,
                "published records must have persistent file attributes"
            );
        }
        assert_eq!(fs::read_dir(root.path())?.count(), 1);
        Ok(())
    }

    #[test]
    fn replacement_respects_the_existing_records_write_access()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let path = root.path().join("1.0.0.json");
        write_registry_json(&path, &version("original")?, true)?;
        let original = fs::read(&path)?;
        let permissions = fs::metadata(&path)?.permissions();
        let mut read_only = permissions.clone();
        read_only.set_readonly(true);
        fs::set_permissions(&path, read_only)?;

        // Use the OS's access verdict: a privileged operator can still open
        // a read-only file, while ordinary users must retain the old record.
        let access_error = fs::OpenOptions::new().write(true).open(&path).err();
        let result = write_registry_json(&path, &version("updated")?, false);
        fs::set_permissions(&path, permissions)?;

        if let Some(expected) = access_error {
            assert!(matches!(
                result,
                Err(LocalRegistryError::Io { source, .. }) if source.kind() == expected.kind()
            ));
            assert_eq!(fs::read(&path)?, original);
        } else {
            result?;
            let current: RegistrySkillVersion = serde_json::from_slice(&fs::read(&path)?)?;
            assert_eq!(current.description.as_deref(), Some("updated"));
        }
        assert_eq!(fs::read_dir(root.path())?.count(), 1);
        Ok(())
    }
}
