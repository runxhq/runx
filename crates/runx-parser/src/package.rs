//! Aggregate, filesystem-independent validation for a complete skill package.
//!
//! Runtime owns directory traversal and supplies normalized text files. This
//! module is the only place that turns those files into package truth.

mod javascript;
mod path;
mod validate;

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    ParseError, SkillRunnerManifest, ValidatedSkill, ValidatedTool, ValidationError,
    harness_fixture::HarnessFixture,
};

pub use validate::validate_skill_package;

/// Extract static ECMAScript module imports from one admitted source file.
/// Dynamic imports are intentionally rejected: the deterministic worker must
/// know the complete module graph before execution.
pub fn javascript_module_imports(
    path: &str,
    source: &str,
) -> Result<Vec<String>, SkillPackageError> {
    javascript::module_imports(path, source)
}

/// Extract the statically bound imports and CommonJS requires used by a
/// process-backed JavaScript source. Process modules may use Node APIs, but
/// dynamic dependencies are rejected because package/source hashing must bind
/// the complete local closure.
pub fn javascript_process_module_imports(
    path: &str,
    source: &str,
) -> Result<Vec<String>, SkillPackageError> {
    javascript::process_module_imports(path, source)
}

/// Resolve one relative ECMAScript import using the same portable path rules
/// used by aggregate package admission and the deterministic worker.
pub fn resolve_javascript_module_import(
    importer: &str,
    specifier: &str,
) -> Result<String, SkillPackageError> {
    path::normalize_module_import(importer, specifier)
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillPackageSource {
    /// Package bytes keyed by normalized package-relative POSIX path. Binary
    /// fixtures and assets remain digest-bound without being interpreted as
    /// parser input.
    pub files: BTreeMap<String, Vec<u8>>,
    /// Paths observed as symbolic links by the filesystem loader. Links are
    /// represented explicitly so pure validation can fail closed.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub symlinks: BTreeSet<String>,
}

/// The execution profile beside a skill's `SKILL.md`.
pub const PROFILE_FILE: &str = "run.yaml";

/// The package-relative directory a profile path names: empty for the root
/// profile, `graph/plan` for `graph/plan/run.yaml`, and `None` for a path that
/// is not a profile.
#[must_use]
pub fn profile_directory(profile_path: &str) -> Option<&str> {
    let directory = profile_path.strip_suffix(PROFILE_FILE)?;
    if directory.is_empty() {
        return Some("");
    }
    directory.strip_suffix('/')
}

impl SkillPackageSource {
    #[must_use]
    pub fn from_documents(skill_markdown: impl Into<String>, profile: Option<String>) -> Self {
        let mut files =
            BTreeMap::from([("SKILL.md".to_owned(), skill_markdown.into().into_bytes())]);
        if let Some(profile) = profile {
            files.insert(PROFILE_FILE.to_owned(), profile.into_bytes());
        }
        Self {
            files,
            symlinks: BTreeSet::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidatedJavaScriptModule {
    pub path: String,
    pub digest: String,
    pub imports: Vec<String>,
}

/// One tool owned by a skill package. The manifest path is package-relative
/// truth; `source_files` is the complete local process source closure admitted
/// with that manifest.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidatedPackageTool {
    pub manifest_path: String,
    pub tool: ValidatedTool,
    pub source_files: BTreeSet<String>,
}

/// A packet schema a package carries in `packets/`: its own, or the pinned
/// copy of another package's packet it reads.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackagePacket {
    /// Package-relative path, `packets/...`.
    pub path: String,
    pub schema: crate::ValidatedPacketSchema,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidatedSkillPackage {
    pub skill: ValidatedSkill,
    /// Every execution profile owned by this manual, keyed by package-relative
    /// path (`run.yaml` for the root profile, `graph/plan/run.yaml` for a stage).
    pub profiles: BTreeMap<String, SkillRunnerManifest>,
    /// Exact `SKILL.md` bytes presented to an operating agent.
    pub manual_markdown: String,
    pub manual_digest: String,
    pub package_digest: String,
    pub source_digests: BTreeMap<String, String>,
    pub javascript_modules: BTreeMap<String, ValidatedJavaScriptModule>,
    /// Bundled local tools keyed by package-relative manifest path. Tool names
    /// may repeat in separate nested profile tool roots, so paths—not names—are
    /// the stable package identity.
    pub tools: BTreeMap<String, ValidatedPackageTool>,
    /// Package-relative files consumed directly by process-backed sources,
    /// including their typed manifests. These remain outside the deterministic
    /// JavaScript module bundle.
    pub execution_files: BTreeSet<String>,
    /// Explicit, parser-validated files needed only while replaying the inline
    /// package harness. These are never inferred from arbitrary input values.
    pub harness_files: BTreeSet<String>,
    /// Complete parser-owned package material, including runtime dependencies,
    /// operator references, and nested manual packages. Registry projection
    /// uses this instead of walking or interpreting package source again.
    pub consumed_files: BTreeSet<String>,
    pub harness_fixtures: BTreeMap<String, HarnessFixture>,
    pub context_skills: Vec<String>,
    /// Every packet schema in `packets/`, by id.
    pub packets: BTreeMap<runx_contracts::PacketId, PackagePacket>,
    pub source: SkillPackageSource,
}

impl ValidatedSkillPackage {
    #[must_use]
    pub fn file_bytes(&self, path: &str) -> Option<&[u8]> {
        self.source.files.get(path).map(Vec::as_slice)
    }

    #[must_use]
    pub fn file_text(&self, path: &str) -> Option<&str> {
        std::str::from_utf8(self.file_bytes(path)?).ok()
    }

    #[must_use]
    pub fn runner(&self, name: &str) -> Option<&crate::SkillRunnerDefinition> {
        self.root_manifest()?.runners.get(name)
    }

    #[must_use]
    pub fn root_manifest(&self) -> Option<&SkillRunnerManifest> {
        self.profiles.get(PROFILE_FILE)
    }

    #[must_use]
    pub fn manifest_at(&self, path: &str) -> Option<&SkillRunnerManifest> {
        self.profiles.get(path)
    }

    #[must_use]
    pub fn tool_at(&self, manifest_path: &str) -> Option<&ValidatedPackageTool> {
        self.tools.get(manifest_path)
    }

    #[must_use]
    pub fn default_runner(&self) -> Option<&crate::SkillRunnerDefinition> {
        let manifest = self.root_manifest()?;
        manifest
            .runners
            .values()
            .find(|runner| runner.default)
            .or_else(|| {
                (manifest.runners.len() == 1)
                    .then(|| manifest.runners.values().next())
                    .flatten()
            })
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SkillPackageError {
    #[error("{path}: {source}")]
    Parse {
        path: String,
        #[source]
        source: ParseError,
    },
    #[error("{path}: {source}")]
    Validation {
        path: String,
        #[source]
        source: ValidationError,
    },
    #[error("{path}: {message}")]
    Invalid { path: String, message: String },
}

impl SkillPackageError {
    #[must_use]
    pub fn invalid(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Invalid {
            path: path.into(),
            message: message.into(),
        }
    }

    #[must_use]
    pub(crate) fn with_path_prefix(self, prefix: &str) -> Self {
        let qualify = |path: String| format!("{prefix}/{path}");
        match self {
            Self::Parse { path, source } => Self::Parse {
                path: qualify(path),
                source,
            },
            Self::Validation { path, source } => Self::Validation {
                path: qualify(path),
                source,
            },
            Self::Invalid { path, message } => Self::Invalid {
                path: qualify(path),
                message,
            },
        }
    }
}
