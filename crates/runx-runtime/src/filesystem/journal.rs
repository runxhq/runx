//! The journal of every change a run makes to its workspace. Before a
//! bundle's first rename, what each file held before and holds after, and
//! the directories it creates, are recorded in the run's kernel state; when
//! it ends, how. A bundle a crash left begun is rolled back by the run's next
//! driver, so a bundle is applied whole or not at all. An undo reads the
//! journal alone: what each change restores, what it is bound to, and the
//! order the changes were made.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::admission::{AdmittedBundle, PriorFile};
use super::apply::{remove_created_directories, restore_file};
use crate::RuntimeError;
use crate::kernel::store::{Append, Blob, Expected, KernelStore, StreamEvent};

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
enum BundleEvent {
    Begun(Box<Begun>),
    Ended(Box<Ended>),
}

impl StreamEvent for BundleEvent {
    const KIND: &'static str = "bundle";

    fn event_type(&self) -> &'static str {
        match self {
            Self::Begun(_) => "begun",
            Self::Ended(_) => "ended",
        }
    }
}

/// A bundle about to be applied: what each file it changes held before and
/// will hold after, and the directories it will create.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Begun {
    operation: String,
    root: String,
    files: Vec<JournaledFile>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    created: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct JournaledFile {
    /// Relative to the bundle's root.
    path: String,
    /// Blob: what the file held; none when it did not exist.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    before: Option<String>,
    /// Its Unix mode, when it existed there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mode: Option<u32>,
    /// The digest of what the bundle leaves it holding; none when the
    /// bundle removes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    after: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Ended {
    /// The version of the bundle's `begun` event.
    bundle: u64,
    outcome: Outcome,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Outcome {
    Applied,
    RolledBack,
}

/// Where a run's changes are journaled: its stream in the project's kernel
/// store.
pub(crate) struct Journal {
    store: Arc<KernelStore>,
    key: String,
}

impl Journal {
    /// The journal of the run `env` names, when it names one.
    pub(crate) fn of_run(
        env: &BTreeMap<String, String>,
        cwd: &Path,
    ) -> Result<Option<Self>, RuntimeError> {
        let Some(run_id) = env.get(crate::RUNX_RUN_ID_ENV) else {
            return Ok(None);
        };
        let store = KernelStore::for_project(&crate::paths::RunxPaths::resolve(env, cwd)).map_err(
            |error| RuntimeError::effect_state("opening the change journal", error.to_string()),
        )?;
        Ok(Some(Self {
            store,
            key: run_id.clone(),
        }))
    }

    /// Record a bundle before its first rename; its id is the version of
    /// that record.
    pub(super) fn begin(
        &self,
        bundle: &AdmittedBundle,
        snapshots: &BTreeMap<String, PriorFile>,
        created: &[PathBuf],
    ) -> Result<u64, RuntimeError> {
        let root = bundle.root.as_path();
        let written = bundle
            .writes
            .iter()
            .map(|write| {
                (
                    write.relative.as_str(),
                    runx_contracts::sha256_prefixed(write.contents.as_bytes()),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let mut blobs = Vec::new();
        let mut files = Vec::with_capacity(snapshots.len());
        for (path, prior) in snapshots {
            let (before, mode) = match prior {
                PriorFile::Absent => (None, None),
                PriorFile::File {
                    contents,
                    permissions,
                } => {
                    let blob = Blob::new(contents.clone()).map_err(journal_error)?;
                    let digest = blob.digest().to_owned();
                    blobs.push(blob);
                    (Some(digest), mode_of(permissions))
                }
            };
            files.push(JournaledFile {
                path: path.clone(),
                before,
                mode,
                after: written.get(path.as_str()).cloned(),
            });
        }
        let event = BundleEvent::Begun(Box::new(Begun {
            operation: bundle.operation.clone(),
            root: root.to_string_lossy().into_owned(),
            files,
            created: created
                .iter()
                .filter_map(|directory| directory.strip_prefix(root).ok())
                .map(|directory| directory.to_string_lossy().into_owned())
                .collect(),
        }));
        self.append(event, blobs)
    }

    /// Record how bundle `bundle` ended.
    pub(super) fn end(&self, bundle: u64, outcome: Outcome) -> Result<(), RuntimeError> {
        self.append(
            BundleEvent::Ended(Box::new(Ended { bundle, outcome })),
            Vec::new(),
        )
        .map(|_| ())
    }

    fn append(&self, event: BundleEvent, blobs: Vec<Blob>) -> Result<u64, RuntimeError> {
        let version = self
            .store
            .version::<BundleEvent>(&self.key)
            .map_err(journal_error)?;
        let expected = match version {
            0 => Expected::Absent,
            version => Expected::Version(version),
        };
        let append = Append::kernel(self.key.clone(), expected, &event)
            .map_err(journal_error)?
            .with_blobs(blobs);
        self.store
            .append(vec![append])
            .map_err(journal_error)?
            .first()
            .copied()
            .ok_or_else(|| journal_error("the journal recorded nothing"))
    }
}

/// One change a run's tree applied, as its journal records it.
pub(crate) struct AppliedChange {
    /// The run that made it, which journals it.
    pub(crate) run: String,
    /// Its id in that run's journal.
    pub(crate) bundle: u64,
    /// Its place in the store's commit order: the order the changes were
    /// made in.
    pub(crate) sequence: u64,
    pub(crate) root: PathBuf,
    pub(crate) files: Vec<ChangedFile>,
}

/// One file a change made: what it held before, by blob, none when it did
/// not exist; and the digest of what the change left, none when it removed
/// it.
pub(crate) struct ChangedFile {
    pub(crate) path: String,
    pub(crate) before: Option<String>,
    pub(crate) after: Option<String>,
}

/// Every change the run `run_id`, or a run below it, applied, in the order
/// they were made. A run below its root journals under the root's key, the
/// run id it inherits, so a tree's changes are its root's.
pub(crate) fn applied_changes(
    store: &KernelStore,
    run_id: &str,
) -> Result<Vec<AppliedChange>, RuntimeError> {
    const PAGE: usize = 512;
    let tree = crate::execution::run_record::root_run_id(run_id);
    let mine = |key: &str| {
        key == run_id
            || key
                .strip_prefix(run_id)
                .is_some_and(|rest| rest.starts_with('/'))
    };
    let mut begun = BTreeMap::<(String, u64), (u64, Box<Begun>)>::new();
    let mut applied = Vec::new();
    let mut after = 0;
    loop {
        let page = store
            .tree_after::<BundleEvent>(tree, after, PAGE)
            .map_err(journal_error)?;
        let Some(last) = page.last() else {
            break;
        };
        after = last.sequence;
        for event in page.into_iter().filter(|event| mine(&event.key)) {
            match event.recorded.event {
                BundleEvent::Begun(opened) => {
                    begun.insert(
                        (event.key, event.recorded.version),
                        (event.sequence, opened),
                    );
                }
                BundleEvent::Ended(ended) if ended.outcome == Outcome::Applied => {
                    applied.push((event.key, ended.bundle));
                }
                BundleEvent::Ended(_) => {}
            }
        }
    }
    let mut changes = applied
        .into_iter()
        .map(|(run, bundle)| {
            let (sequence, opened) = begun.remove(&(run.clone(), bundle)).ok_or_else(|| {
                journal_error(format!("run {run} journals an end to no change {bundle}"))
            })?;
            Ok(AppliedChange {
                run,
                bundle,
                sequence,
                root: PathBuf::from(opened.root),
                files: opened
                    .files
                    .into_iter()
                    .map(|file| ChangedFile {
                        path: file.path,
                        before: file.before,
                        after: file.after,
                    })
                    .collect(),
            })
        })
        .collect::<Result<Vec<_>, RuntimeError>>()?;
    changes.sort_by_key(|change| change.sequence);
    Ok(changes)
}

/// Roll back every bundle the run `run_id` or any run below it began and
/// never ended, restoring each file to what it held and removing the
/// directories the bundle created, then record it rolled back.
pub(crate) fn recover(store: &Arc<KernelStore>, run_id: &str) -> Result<(), RuntimeError> {
    for head in store
        .tree_heads::<BundleEvent>(run_id)
        .map_err(journal_error)?
    {
        let events = store
            .read::<BundleEvent>(&head.key)
            .map_err(journal_error)?;
        let mut open = BTreeMap::new();
        for recorded in events {
            match recorded.event {
                BundleEvent::Begun(begun) => {
                    open.insert(recorded.version, begun);
                }
                BundleEvent::Ended(ended) => {
                    open.remove(&ended.bundle);
                }
            }
        }
        let journal = Journal {
            store: Arc::clone(store),
            key: head.key.clone(),
        };
        for (bundle, begun) in open {
            let root = PathBuf::from(&begun.root);
            for file in begun.files.iter().rev() {
                let contents = file
                    .before
                    .as_deref()
                    .map(|digest| store.blob(digest))
                    .transpose()
                    .map_err(journal_error)?;
                restore_file(
                    &begun.operation,
                    &root.join(&file.path),
                    contents.as_deref(),
                    file.mode,
                )?;
            }
            remove_created_directories(
                &begun
                    .created
                    .iter()
                    .map(|directory| root.join(directory))
                    .collect::<Vec<_>>(),
            )?;
            journal.end(bundle, Outcome::RolledBack)?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn mode_of(permissions: &std::fs::Permissions) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    Some(permissions.mode())
}

#[cfg(not(unix))]
fn mode_of(_permissions: &std::fs::Permissions) -> Option<u32> {
    None
}

fn journal_error(error: impl std::fmt::Display) -> RuntimeError {
    RuntimeError::effect_state("journaling a change", error.to_string())
}

#[cfg(test)]
mod tests;
