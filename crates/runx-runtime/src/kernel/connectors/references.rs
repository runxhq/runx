//! What a provider locator names, from the reference kinds connectors
//! declare. The runtime classifies references this way instead of knowing
//! any provider's locator shapes.

use std::sync::Arc;

use runx_contracts::ReferenceType;
use runx_contracts::connector::ConnectorManifest;

use super::LoadedConnector;

/// The kind `locator` names. The admitted connector owns its locator
/// vocabulary, including a connector supplied by an external publisher; an act
/// may also target another provider's resource, which is classified by the
/// first connector the run can reach that declares it. A locator none declares
/// is an external URL.
#[must_use]
pub(crate) fn bound_reference_kind(
    connector: &ConnectorManifest,
    reachable: &[Arc<LoadedConnector>],
    locator: &str,
) -> ReferenceType {
    std::iter::once(connector)
        .chain(reachable.iter().map(|reachable| &reachable.manifest))
        .flat_map(|manifest| manifest.references.iter())
        .find_map(|reference| {
            runx_parser::whole_match(reference.locator.as_str())
                .ok()
                .filter(|pattern| pattern.is_match(locator))
                .map(|_| ReferenceType::from(reference.kind))
        })
        .unwrap_or(ReferenceType::ExternalUrl)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locators_are_classified_by_what_the_reachable_connectors_declare()
    -> Result<(), super::super::ConnectorRegistryError> {
        let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let scope = super::super::ConnectorScope::at(&source)?;
        let github = scope.connector("github")?;
        let slack = scope.connector("slack")?;
        let reachable = scope.connectors()?;

        assert_eq!(
            bound_reference_kind(&github.manifest, &[], "github://acme/app/issues/7"),
            ReferenceType::Thread
        );
        assert_eq!(
            bound_reference_kind(&github.manifest, &[], "github://acme/app/pulls/7"),
            ReferenceType::ChangeRequest
        );
        assert_eq!(
            bound_reference_kind(&slack.manifest, &[], "slack://T123/C456/1750000000.000001"),
            ReferenceType::Thread
        );
        // Another provider's resource: unknown unless the run can reach the
        // connector that declares it.
        assert_eq!(
            bound_reference_kind(&slack.manifest, &[], "github://acme/app/issues/7"),
            ReferenceType::ExternalUrl
        );
        assert_eq!(
            bound_reference_kind(&slack.manifest, &reachable, "github://acme/app/issues/7"),
            ReferenceType::Thread
        );
        assert_eq!(
            bound_reference_kind(&slack.manifest, &reachable, "https://example.com/report"),
            ReferenceType::ExternalUrl
        );
        Ok(())
    }
}
