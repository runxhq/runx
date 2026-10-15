//! A publish binds the package's closure, including skills other publishers
//! own. A registry ref the package names that no registry directory holds is
//! read, as the registry signed it, from the registry the publish targets,
//! into a directory of this publish's own; the package is inspected again, a
//! level at a time, until every ref it names is held.

use std::collections::{BTreeMap, BTreeSet};

use super::http::{RegistryClient, RegistryClientError};
use super::local::{FileRegistryStore, LocalRegistryError, PutVersionOptions};
use super::publish_package::{
    RegistryPublishPackageError, RegistryPublishPackageRequest, unheld_publish_registry_refs,
};
use super::refs::parse_registry_ref;
use crate::http::RuntimeHttpTransport;

/// A closure deeper than this is a cycle or a fault, not a real graph.
const MAX_CLOSURE_DEPTH: usize = 16;

#[derive(Debug, thiserror::Error)]
pub enum PublishClosureError {
    #[error(transparent)]
    Package(#[from] RegistryPublishPackageError),
    #[error(transparent)]
    Client(#[from] RegistryClientError),
    #[error(transparent)]
    Store(#[from] LocalRegistryError),
    #[error("{0}")]
    Unresolved(String),
    #[error("creating the publish's registry directory: {0}")]
    Directory(std::io::Error),
}

/// The registry directory a publish binds its closure from, held for as long
/// as the publish runs.
pub struct PublishClosure {
    directory: tempfile::TempDir,
    /// The environment the publish runs in: its own directory, and the
    /// registry the records came from.
    pub env: BTreeMap<String, String>,
}

impl PublishClosure {
    #[must_use]
    pub fn directory(&self) -> &std::path::Path {
        self.directory.path()
    }
}

/// Supply every registry ref the package names from `client`'s registry.
/// A publish whose environment already names a registry directory binds from
/// that directory alone, so `None` is returned and nothing is read.
pub fn materialize_publish_closure<T: RuntimeHttpTransport>(
    client: &RegistryClient<T>,
    registry_url: &str,
    request: RegistryPublishPackageRequest<'_>,
) -> Result<Option<PublishClosure>, PublishClosureError> {
    let paths = crate::RunxPaths::resolve(request.env, request.cwd);
    if paths.registry_dir().is_some() {
        return Ok(None);
    }
    let directory = tempfile::Builder::new()
        .prefix("runx-publish-closure-")
        .tempdir()
        .map_err(PublishClosureError::Directory)?;
    let mut env = request.env.clone();
    env.insert(
        crate::paths::RUNX_REGISTRY_DIR_ENV.to_owned(),
        directory.path().to_string_lossy().into_owned(),
    );
    // The records' source authority is the registry they came from.
    env.entry("RUNX_REGISTRY_URL".to_owned())
        .or_insert_with(|| registry_url.to_owned());
    let store = FileRegistryStore::new(directory.path());
    let mut supplied = BTreeSet::new();
    for _ in 0..=MAX_CLOSURE_DEPTH {
        let unheld = unheld_publish_registry_refs(RegistryPublishPackageRequest {
            env: &env,
            ..request
        })?;
        if unheld.is_empty() {
            return Ok(Some(PublishClosure { directory, env }));
        }
        for reference in unheld {
            // A package binds an exact closure, so its refs name the canonical
            // `<owner>/<name>`; a root name may move and is never one.
            let resolved = parse_registry_ref(&reference);
            let version = match resolved.version {
                Some(version) => version,
                None => client
                    .read(&resolved.skill_id, None)?
                    .map(|detail| detail.version)
                    .ok_or_else(|| {
                        PublishClosureError::Unresolved(format!(
                            "registry ref '{reference}' names no published skill"
                        ))
                    })?,
            };
            let key = format!("{}@{version}", resolved.skill_id);
            if !supplied.insert(key.clone()) {
                return Err(PublishClosureError::Unresolved(format!(
                    "registry ref '{reference}' is still unresolved after {key} was supplied"
                )));
            }
            let record = client
                .version_record(&resolved.skill_id, &version)?
                .ok_or_else(|| {
                    PublishClosureError::Unresolved(format!(
                        "registry ref '{reference}' names no executable published version"
                    ))
                })?;
            store.put_version(record, PutVersionOptions::default())?;
        }
    }
    Err(PublishClosureError::Unresolved(format!(
        "the package's registry closure exceeds {MAX_CLOSURE_DEPTH} levels"
    )))
}
