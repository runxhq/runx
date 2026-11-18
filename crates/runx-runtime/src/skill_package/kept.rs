//! A run's closure, kept by its digest (the loop contract's E5). A run bound
//! fresh keeps every package its closure admits, so whatever continues it
//! later, a person's answer, a time that came or a process that died, binds
//! the code it was admitted with, however the files change meanwhile. An
//! improved skill never strands a run that waits, and every new run binds the
//! improvement.
//!
//! A bundled skill is runx's own, never a package a closure carries (D30):
//! it isn't kept, it's bound from the runtime as it is now, and a run whose
//! root is one keeps nothing, so a change of runx itself refuses it, as the
//! loop contract says.
//!
//! Files are kept once by their own digest under the project's `kept/files`,
//! and a closure by its digest under `kept/closures`, naming each package it
//! binds, where it was admitted and which files it holds. So a closure's bytes
//! are kept once, however many runs bind it, and an unchanged file is kept
//! once across every version of its package. A closure is written after its
//! files, so a kept closure never names a file that isn't there.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use runx_parser::SkillPackageSource;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{AdmittedPackages, ExecutionBinding, LoadedSkillPackage, SkillInspectionError};
use crate::RuntimeError;

const KEPT_DIR: &str = "kept";
const SCHEMA: &str = "runx.kept_closure.v1";

/// The digests a binding is pinned to, and whether they're its run's own.
/// A run continuing, or calling a method it loaded, binds its kept closure.
/// A caller's pins on a fresh run bind what's on disk now, refused if it
/// changed, so no pin outlives an upgrade.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Pins<'a> {
    pub(crate) package_digest: Option<&'a str>,
    pub(crate) closure_digest: Option<&'a str>,
    pub(crate) continuation: bool,
}

/// Why a package couldn't be bound: it couldn't be read, its binding didn't
/// verify, or its kept closure couldn't be read or written.
#[derive(Debug)]
pub(crate) enum BindError {
    Load(RuntimeError),
    Binding(SkillInspectionError),
    Kept(RuntimeError),
}

impl std::fmt::Display for BindError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Load(error) | Self::Kept(error) => error.fmt(f),
            Self::Binding(error) => error.fmt(f),
        }
    }
}

/// The package a binding starts from, in the project whose runx directory
/// is `project`. Pinned to a closure that's kept, it's the kept root, with
/// the kept packages its children are read from, and nothing of what's on
/// disk now is read; otherwise, as for a run from before closures were kept,
/// it's the package at `directory` as it is.
pub(crate) fn package(
    directory: &Path,
    project: &Path,
    pins: Pins<'_>,
) -> Result<Bound, BindError> {
    match kept(project, pins)? {
        Some(bound) => Ok(bound),
        None => Ok(Bound {
            loaded: Arc::new(
                super::load_validated_skill_package(directory).map_err(BindError::Load)?,
            ),
            kept: None,
        }),
    }
}

/// The kept root `pins` name, when its closure is kept: nothing is resolved
/// or read from where the package lies now, since the kept bytes are checked
/// against their digests as they're read.
pub(crate) fn kept(project: &Path, pins: Pins<'_>) -> Result<Option<Bound>, BindError> {
    let Some(closure_digest) = pins.closure_digest.filter(|_| pins.continuation) else {
        return Ok(None);
    };
    Ok(read(project, closure_digest)
        .map_err(BindError::Kept)?
        .map(|kept| Bound {
            loaded: kept.root,
            kept: Some(kept.packages),
        }))
}

/// A package a binding starts from, and the kept packages it binds from when
/// a kept closure gave it.
pub(crate) struct Bound {
    pub(crate) loaded: Arc<LoadedSkillPackage>,
    kept: Option<AdmittedPackages>,
}

impl Bound {
    /// Bind its `method` against `pins`. Bound fresh and fully, the closure
    /// is kept under `project`, so whatever continues the run binds this code.
    pub(crate) fn bind(
        &self,
        method: &str,
        env: &BTreeMap<String, String>,
        project: &Path,
        pins: Pins<'_>,
    ) -> Result<ExecutionBinding, BindError> {
        let binding = super::verify_binding(
            Arc::clone(&self.loaded),
            method,
            env,
            pins,
            self.kept.as_ref(),
        )
        .map_err(BindError::Binding)?;
        if self.kept.is_none()
            && pins.closure_digest.is_none()
            && let Some(closure_digest) = &binding.closure_digest
        {
            keep(project, closure_digest, &self.loaded, &binding.packages)
                .map_err(BindError::Kept)?;
        }
        Ok(binding)
    }
}

/// A kept closure's record: its root's directory, and each package it binds.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    schema: String,
    root: PathBuf,
    packages: Vec<Package>,
}

/// One package as a closure admitted it: where, and the digest of each file.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Package {
    directory: PathBuf,
    package_root: PathBuf,
    profile_path: Option<String>,
    package_digest: String,
    files: BTreeMap<String, String>,
    symlinks: Vec<String>,
    /// The digest of the `signature.json` it carried, kept beside its files,
    /// so its identity is verified against what's kept.
    signature: Option<String>,
}

/// A kept closure, read back: its root and every package it binds.
struct Kept {
    root: Arc<LoadedSkillPackage>,
    packages: AdmittedPackages,
}

fn closure_path(project: &Path, closure_digest: &str) -> PathBuf {
    project
        .join(KEPT_DIR)
        .join("closures")
        .join(format!("{}.json", hex(closure_digest)))
}

fn file_path(project: &Path, digest: &str) -> PathBuf {
    project.join(KEPT_DIR).join("files").join(digest)
}

/// The hex of a digest, without its `sha256:` prefix.
fn hex(digest: &str) -> &str {
    digest.strip_prefix("sha256:").unwrap_or(digest)
}

fn file_digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn io(what: String) -> impl FnOnce(std::io::Error) -> RuntimeError {
    move |source| RuntimeError::io(what, source)
}

/// Keep the closure `closure_digest` names, `root` and the packages it binds,
/// unless it's kept already.
fn keep(
    project: &Path,
    closure_digest: &str,
    root: &LoadedSkillPackage,
    packages: &AdmittedPackages,
) -> Result<(), RuntimeError> {
    let record_path = closure_path(project, closure_digest);
    let bundled = |package_root: &Path| {
        matches!(
            packages.identity(package_root),
            Some(crate::identity::PackageIdentity::Bundled { .. })
        )
    };
    if record_path.is_file() || bundled(&root.package_root) {
        return Ok(());
    }
    let files_dir = project.join(KEPT_DIR).join("files");
    std::fs::create_dir_all(&files_dir).map_err(io(format!("making {}", files_dir.display())))?;
    let mut kept = Vec::new();
    for (directory, package) in packages.packages.iter() {
        if bundled(&package.package_root) {
            continue;
        }
        let mut files = BTreeMap::new();
        let kept_file = |bytes: &[u8]| -> Result<String, RuntimeError> {
            let digest = file_digest(bytes);
            let at = file_path(project, &digest);
            if !at.is_file() {
                crate::durable::replace_file(&at, bytes)
                    .map_err(io(format!("keeping {}", at.display())))?;
            }
            Ok(digest)
        };
        for (path, bytes) in &package.package.source.files {
            files.insert(path.clone(), kept_file(bytes)?);
        }
        let signature = match &package.kept_signature {
            Some(document) => Some(kept_file(document.as_bytes())?),
            None => {
                let at = package.package_root.join(crate::registry::SIGNATURE_FILE);
                match std::fs::read(&at) {
                    Ok(bytes) => Some(kept_file(&bytes)?),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                    Err(error) => return Err(io(format!("reading {}", at.display()))(error)),
                }
            }
        };
        kept.push(Package {
            directory: directory.clone(),
            package_root: package.package_root.clone(),
            profile_path: package.profile_path.clone(),
            package_digest: package.package.package_digest.clone(),
            files,
            symlinks: package.package.source.symlinks.iter().cloned().collect(),
            signature,
        });
    }
    let record = Record {
        schema: SCHEMA.to_owned(),
        root: root.directory.clone(),
        packages: kept,
    };
    let bytes = serde_json::to_vec_pretty(&record).map_err(|error| {
        RuntimeError::failed("keeping a closure", format!("encoding it: {error}"))
    })?;
    if let Some(parent) = record_path.parent() {
        std::fs::create_dir_all(parent).map_err(io(format!("making {}", parent.display())))?;
    }
    crate::durable::replace_file(&record_path, &bytes)
        .map_err(io(format!("keeping {}", record_path.display())))
}

/// The closure `closure_digest` names, when it's kept: each package rebuilt
/// from its kept files, each file checked against its digest and each package
/// against its own, so what's read back is what was admitted.
fn read(project: &Path, closure_digest: &str) -> Result<Option<Kept>, RuntimeError> {
    let record_path = closure_path(project, closure_digest);
    let bytes = match std::fs::read(&record_path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io(format!("reading {}", record_path.display()))(error)),
    };
    let invalid = |why: String| {
        RuntimeError::failed(
            format!("the kept closure {closure_digest}"),
            format!("{why}; delete {} to bind from disk", record_path.display()),
        )
    };
    let record: Record = serde_json::from_slice(&bytes)
        .map_err(|error| invalid(format!("it isn't {SCHEMA}: {error}")))?;
    if record.schema != SCHEMA {
        return Err(invalid(format!(
            "its schema is {}, not {SCHEMA}",
            record.schema
        )));
    }
    let mut packages = BTreeMap::new();
    for kept in record.packages {
        let mut source = SkillPackageSource::default();
        for (path, digest) in &kept.files {
            let at = file_path(project, digest);
            let bytes = std::fs::read(&at).map_err(io(format!("reading {}", at.display())))?;
            if file_digest(&bytes) != *digest {
                return Err(invalid(format!("its file {path} doesn't match its digest")));
            }
            source.files.insert(path.clone(), bytes);
        }
        source.symlinks = kept.symlinks.into_iter().collect();
        let kept_signature = match &kept.signature {
            Some(digest) => {
                let at = file_path(project, digest);
                let bytes = std::fs::read(&at).map_err(io(format!("reading {}", at.display())))?;
                if file_digest(&bytes) != *digest {
                    return Err(invalid("its signature doesn't match its digest".to_owned()));
                }
                Some(Arc::<str>::from(String::from_utf8(bytes).map_err(
                    |_| invalid("its signature isn't UTF-8 text".to_owned()),
                )?))
            }
            None => None,
        };
        let mut package = super::validated_package(source)?;
        let resolved_input_packet_schemas = crate::packet_schemas::hydrate_packet_input_contracts(
            &mut package,
            &kept.package_root,
        )?;
        if package.package_digest != kept.package_digest {
            return Err(invalid(format!(
                "its package at {} is {}, not the {} it kept",
                kept.directory.display(),
                package.package_digest,
                kept.package_digest
            )));
        }
        packages.insert(
            kept.directory.clone(),
            Arc::new(LoadedSkillPackage {
                directory: kept.directory,
                package_root: kept.package_root,
                profile_path: kept.profile_path,
                package,
                resolved_input_packet_schemas,
                kept_signature,
            }),
        );
    }
    let root = packages
        .get(&record.root)
        .cloned()
        .ok_or_else(|| invalid("it names no package at its root".to_owned()))?;
    Ok(Some(Kept {
        root,
        packages: AdmittedPackages {
            packages: Arc::new(packages),
            identities: Arc::default(),
            projects: Arc::default(),
        },
    }))
}
