//! The one grammar for a registry version label. The CLI checks it before it
//! packages or publishes, the hosted registry checks it again before any write,
//! and `@runxhq/contracts` mirrors it against the same vectors
//! (`fixtures/contracts/registry-version`). A label always starts with an ASCII
//! letter or digit, so it can never read as a flag, a path or a shell word.

/// `^[A-Za-z0-9][A-Za-z0-9._+-]{0,127}$`
pub const REGISTRY_VERSION_PATTERN: &str = "^[A-Za-z0-9][A-Za-z0-9._+-]{0,127}$";
pub const REGISTRY_VERSION_MAX_LENGTH: usize = 128;

#[must_use]
pub fn is_registry_version(value: &str) -> bool {
    let bytes = value.as_bytes();
    let Some(first) = bytes.first() else {
        return false;
    };
    bytes.len() <= REGISTRY_VERSION_MAX_LENGTH
        && first.is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'+' | b'-'))
}

/// Checks a version label, naming what a refused label breaks.
///
/// # Errors
///
/// Returns a message when `value` is not a registry version label.
pub fn validate_registry_version(value: &str) -> Result<(), String> {
    if is_registry_version(value) {
        return Ok(());
    }
    Err(format!(
        "registry version {value:?} is invalid: a version starts with an ASCII letter or digit, \
         uses only letters, digits, '.', '_', '+' and '-', and has at most {REGISTRY_VERSION_MAX_LENGTH} characters"
    ))
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn matches_the_shared_vectors() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/contracts/registry-version/registry-version-v1.vectors.json"
        );
        let fixture: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).expect("vectors"))
                .expect("vectors json");
        assert_eq!(fixture["pattern"], REGISTRY_VERSION_PATTERN);
        let cases = fixture["cases"].as_array().expect("cases");
        assert!(cases.len() > 20);
        for case in cases {
            let input = case["input"].as_str().expect("input");
            let valid = case["expectation"] == "valid";
            assert_eq!(is_registry_version(input), valid, "{}", case["name"]);
            assert_eq!(
                validate_registry_version(input).is_ok(),
                valid,
                "{}",
                case["name"]
            );
        }
    }
}
