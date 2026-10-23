#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolAdmission {
    pub allowed: bool,
    pub reason: &'static str,
}

impl ToolAdmission {
    #[must_use]
    pub const fn allow() -> Self {
        Self {
            allowed: true,
            reason: "tool admitted",
        }
    }

    #[must_use]
    pub const fn deny(reason: &'static str) -> Self {
        Self {
            allowed: false,
            reason,
        }
    }
}

#[must_use]
pub fn admit_agent_tool(value: &str) -> ToolAdmission {
    let value = value.trim();
    if value.is_empty() {
        return ToolAdmission::deny("tool name must not be empty");
    }
    if value.starts_with('/') || value.starts_with('\\') {
        return ToolAdmission::deny("tool name must not be an absolute path");
    }
    if value.contains('/') || value.contains('\\') || value.contains("..") {
        return ToolAdmission::deny("tool name must not contain path traversal or separators");
    }
    let lower = value.to_ascii_lowercase();
    if lower == "manifest.json"
        || lower.ends_with(".json")
        || lower.ends_with(".yaml")
        || lower.ends_with(".yml")
        || lower.ends_with(".toml")
    {
        return ToolAdmission::deny("tool name must not look like a manifest or data file path");
    }
    let segments = value.split('.').collect::<Vec<_>>();
    if segments.len() < 2 {
        return ToolAdmission::deny("tool name must include a namespace, for example fs.read");
    }
    if segments
        .iter()
        .any(|segment| segment.is_empty() || !segment.bytes().all(is_catalog_ref_byte))
    {
        return ToolAdmission::deny(
            "tool name segments must contain only letters, numbers, hyphens, or underscores",
        );
    }
    ToolAdmission::allow()
}

const fn is_catalog_ref_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'
}

#[cfg(test)]
mod tests {
    use super::admit_agent_tool;

    #[test]
    fn admits_catalog_style_refs() {
        for value in [
            "fs.read",
            "git.current_branch",
            "git.diff_name_only",
            "command.execute",
            "cli.capture_help",
            "namespace.tool-name",
        ] {
            let admission = admit_agent_tool(value);
            assert!(admission.allowed, "{value}: {}", admission.reason);
        }
    }

    #[test]
    fn rejects_path_and_manifest_like_refs() {
        for value in [
            "",
            "read",
            "/tmp/tool/manifest.json",
            "../tool/manifest.json",
            "tools/read",
            r"tools\read",
            "manifest.json",
            "fs.json",
            "fs..read",
            "fs.read;rm",
            "fs.read all",
        ] {
            let admission = admit_agent_tool(value);
            assert!(!admission.allowed, "{value} unexpectedly admitted");
        }
    }
}
