//! The one spelling of a registry ref in a skill package: `registry:` then
//! `<owner>/<name>`, optionally `@<version>`.

/// What marks a package reference as a registry ref.
pub const REGISTRY_REF_PREFIX: &str = "registry:";

/// The `<owner>/<name>[@<version>]` a registry ref names; `None` for any
/// other reference.
#[must_use]
pub fn registry_ref(reference: &str) -> Option<&str> {
    reference.strip_prefix(REGISTRY_REF_PREFIX)
}
