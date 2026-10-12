//! What a provider locator names, from the reference kinds runx-maintained
//! connectors declare. The runtime classifies references this way instead of
//! knowing any provider's locator shapes. Receipts are classified the same
//! way whichever skill sealed them, so plugin connectors declare none.

use std::sync::OnceLock;

use regex::Regex;
use runx_contracts::ReferenceType;
use runx_contracts::connector::ConnectorManifest;

use super::ConnectorScope;

/// Every runx-maintained connector's declarations, compiled once: connector id,
/// kind, whole-locator pattern.
fn declarations() -> &'static [(String, ReferenceType, Regex)] {
    static DECLARATIONS: OnceLock<Vec<(String, ReferenceType, Regex)>> = OnceLock::new();
    DECLARATIONS.get_or_init(|| {
        let Ok(connectors) = ConnectorScope::runx_maintained_connectors() else {
            return Vec::new();
        };
        connectors
            .iter()
            .flat_map(|connector| {
                let id = connector.manifest.id.as_str().to_owned();
                connector
                    .manifest
                    .references
                    .iter()
                    .filter_map(move |reference| {
                        runx_parser::whole_match(reference.locator.as_str())
                            .ok()
                            .map(|pattern| {
                                (id.clone(), ReferenceType::from(reference.kind), pattern)
                            })
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    })
}

/// The kind `locator` names: the first declaration of `provider`'s connector
/// it matches, or of any runx-maintained connector when no provider is given, and an
/// external URL when none does.
#[must_use]
pub fn reference_kind(provider: Option<&str>, locator: &str) -> ReferenceType {
    declarations()
        .iter()
        .filter(|(id, _, _)| provider.is_none_or(|provider| id == provider))
        .find(|(_, _, pattern)| pattern.is_match(locator))
        .map_or(ReferenceType::ExternalUrl, |(_, kind, _)| kind.clone())
}

/// The admitted connector owns its locator vocabulary, including a connector
/// supplied by an external publisher. An act may also target a resource from
/// another provider, which retains the runtime's standard classification.
#[must_use]
pub(crate) fn bound_reference_kind(connector: &ConnectorManifest, locator: &str) -> ReferenceType {
    connector
        .references
        .iter()
        .find_map(|reference| {
            runx_parser::whole_match(reference.locator.as_str())
                .ok()
                .filter(|pattern| pattern.is_match(locator))
                .map(|_| ReferenceType::from(reference.kind))
        })
        .unwrap_or_else(|| reference_kind(None, locator))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locators_are_classified_by_what_their_connectors_declare() {
        assert_eq!(
            reference_kind(Some("github"), "github://acme/app/issues/7"),
            ReferenceType::Thread
        );
        assert_eq!(
            reference_kind(None, "github://acme/app/pulls/7"),
            ReferenceType::ChangeRequest
        );
        assert_eq!(
            reference_kind(Some("slack"), "slack://T123/C456/1750000000.000001"),
            ReferenceType::Thread
        );
        assert_eq!(
            reference_kind(Some("slack"), "github://acme/app/issues/7"),
            ReferenceType::ExternalUrl
        );
        assert_eq!(
            reference_kind(None, "https://example.com/report"),
            ReferenceType::ExternalUrl
        );
    }
}
