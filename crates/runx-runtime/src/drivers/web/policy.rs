use std::collections::BTreeSet;

use url::Url;

pub(super) fn normalize_allowlist(values: &[String]) -> Result<Vec<String>, String> {
    let mut unique = BTreeSet::new();
    for value in values {
        let entry = value.trim().trim_end_matches('.').to_ascii_lowercase();
        if entry.is_empty() {
            return Err("allowlist entries must be non-empty strings".to_owned());
        }
        if entry == "*" {
            unique.insert(entry);
            continue;
        }
        let host = entry.strip_prefix("*.").unwrap_or(&entry);
        if host.is_empty()
            || host.contains(['/', ':', '*', '?', '#', '@'])
            || host.starts_with('.')
            || host.ends_with('.')
        {
            return Err(format!("invalid allowlist host pattern {entry:?}"));
        }
        unique.insert(entry);
    }
    Ok(unique.into_iter().collect())
}

pub(super) fn host_allowed(host: &str, allowlist: &[String]) -> bool {
    allowlist.iter().any(|entry| {
        entry == "*"
            || entry == host
            || entry.strip_prefix("*.").is_some_and(|suffix| {
                host.len() > suffix.len()
                    && host.ends_with(suffix)
                    && host.as_bytes()[host.len() - suffix.len() - 1] == b'.'
            })
    })
}

pub(super) fn parse_web_url(value: &str) -> Result<Url, String> {
    let url = Url::parse(value).map_err(|error| format!("invalid URL: {error}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("url must use http or https".to_owned());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("url must not contain credentials".to_owned());
    }
    if url.host_str().is_none() {
        return Err("url must contain a host".to_owned());
    }
    Ok(url)
}

pub(super) fn normalized_host(url: &Url) -> Option<String> {
    url.host_str()
        .map(|host| host.trim_end_matches('.').to_ascii_lowercase())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::{host_allowed, normalize_allowlist};

    #[test]
    fn wildcard_allowlist_requires_a_subdomain() {
        let allowlist = vec!["*.example.com".to_owned()];
        assert!(host_allowed("docs.example.com", &allowlist));
        assert!(!host_allowed("example.com", &allowlist));
        assert!(!host_allowed("notexample.com", &allowlist));
    }

    #[test]
    fn global_wildcard_allows_any_public_host_name() {
        let allowlist = normalize_allowlist(&["*".to_owned()]).unwrap();
        assert!(host_allowed("example.com", &allowlist));
        assert!(host_allowed("docs.example.net", &allowlist));
    }

    #[test]
    fn allowlist_rejects_non_host_patterns() {
        assert!(normalize_allowlist(&["https://example.com".to_owned()]).is_err());
        assert!(normalize_allowlist(&[]).unwrap().is_empty());
    }
}
