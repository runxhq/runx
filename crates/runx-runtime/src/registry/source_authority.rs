#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegistryManifestSourceAuthority {
    OfficialRunx,
    RegistrySource(String),
}

pub fn registry_manifest_source_authority_from_registry_url(
    value: &str,
) -> RegistryManifestSourceAuthority {
    if is_official_runx_registry_url(value) {
        RegistryManifestSourceAuthority::OfficialRunx
    } else {
        RegistryManifestSourceAuthority::RegistrySource(format!(
            "remote:{}",
            canonical_registry_url(value)
        ))
    }
}

pub fn registry_manifest_source_key(source: &RegistryManifestSourceAuthority) -> String {
    match source {
        RegistryManifestSourceAuthority::OfficialRunx => "official_runx".to_owned(),
        RegistryManifestSourceAuthority::RegistrySource(value) => value.clone(),
    }
}

pub fn is_official_runx_registry_url(value: &str) -> bool {
    matches!(
        canonical_registry_url(value).as_str(),
        "https://runx.ai" | "https://api.runx.ai"
    )
}

/// Returns a stable, credential-free registry URL for source identity, cache
/// partitioning, and operator presentation.
#[must_use]
pub fn canonical_registry_url(value: &str) -> String {
    let without_fragment = value.split_once('#').map_or(value, |(prefix, _)| prefix);
    let without_query = without_fragment
        .split_once('?')
        .map_or(without_fragment, |(prefix, _)| prefix);
    let Some((scheme, rest)) = without_query.split_once("://") else {
        return without_query.trim_end_matches('/').to_owned();
    };
    let (authority, path) = rest
        .split_once('/')
        .map_or((rest, ""), |(authority, path)| (authority, path));
    let authority = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let path = path.trim_matches('/');
    if path.is_empty() {
        format!("{scheme}://{authority}")
    } else {
        format!("{scheme}://{authority}/{path}")
    }
}

#[cfg(test)]
mod tests {
    use super::canonical_registry_url;

    #[test]
    fn canonical_registry_url_removes_credentials_query_fragment_and_trailing_slashes() {
        assert_eq!(
            canonical_registry_url(
                "https://user:secret@registry.runx.test/team/?token=secret#anchor"
            ),
            "https://registry.runx.test/team"
        );
        assert_eq!(
            canonical_registry_url("https://registry.runx.test///"),
            "https://registry.runx.test"
        );
    }
}
