//! The tool packages a connector's process operations run.
//!
//! A tool ships beside its connector, at `tools/<segment>/.../`, one
//! directory per segment of its dotted name (`git.push` is `tools/git/push/`).
//! A tool is never looked up from the skill that calls the
//! operation: the program that receives a connector's credential is the one
//! its connector ships.
//!
//! Each tool runs from a copy under the runx home addressed by its package
//! digest, written from bytes that still match the digest the call was
//! admitted with, so the program that runs is the one the receipt names.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use runx_contracts::connector::{Digest, tool_package_digest};

use crate::paths::RunxPaths;

const MANIFEST: &str = "manifest.json";
const TOOLS_DIR: &str = "tools";

/// One tool package a connector ships, and its digest. Its files are read
/// from the connector's directory again when it runs.
#[derive(Debug, PartialEq)]
pub struct ConnectorTool {
    pub digest: Digest,
    directory: PathBuf,
}

impl ConnectorTool {
    /// Where the tool `tool` sits below its connector's directory: `tools/`,
    /// then one directory per segment of its dotted name.
    pub(crate) fn package_path(tool: &str) -> String {
        format!("{TOOLS_DIR}/{}", tool.replace('.', "/"))
    }

    /// The tool `tool` the plugin connector at `connector_dir` ships.
    pub(crate) fn in_plugin(connector_dir: &Path, tool: &str) -> Result<Self, String> {
        let directory = connector_dir.join(Self::package_path(tool));
        let files = read(&directory, tool)?;
        Ok(Self {
            digest: digest(&files)?,
            directory,
        })
    }

    /// Every file of this tool package, by its path below the package, from
    /// bytes that still match its digest.
    pub(crate) fn files(&self, tool: &str) -> Result<BTreeMap<String, Vec<u8>>, String> {
        let files = read(&self.directory, tool)?;
        if digest(&files)? != self.digest {
            return Err(format!(
                "tool {tool} changed after its connector was loaded; run again to load it"
            ));
        }
        Ok(files)
    }

    /// The manifest of this tool's copy under the home of `paths`, written
    /// when absent, from bytes that still match its digest.
    pub(crate) fn installed(&self, paths: &RunxPaths, tool: &str) -> Result<PathBuf, String> {
        let files = self.files(tool)?;
        let copy = paths.connector_tool(self.digest.as_str());
        if !copy.exists() {
            install(&copy, &files)?;
        }
        // The copy is checked on every use: what runs is what the call was
        // admitted with, whoever wrote the directory.
        let installed = crate::skill_package::read_tool_package(&copy)
            .map_err(|error| format!("reading the installed tool {tool}: {error}"))?;
        if digest(&installed)? != self.digest {
            return Err(format!(
                "the installed copy of tool {tool} at {} is not {}; remove it and run again",
                copy.display(),
                self.digest.as_str()
            ));
        }
        Ok(copy.join(MANIFEST))
    }
}

/// Every file of the tool package at `directory`, whose manifest names
/// `tool`.
fn read(directory: &Path, tool: &str) -> Result<BTreeMap<String, Vec<u8>>, String> {
    let files = crate::skill_package::read_tool_package(directory).map_err(|error| {
        format!(
            "the connector ships no readable tool {tool} at {}: {error}",
            directory.display()
        )
    })?;
    named(files.get(MANIFEST).map(Vec::as_slice), tool)?;
    Ok(files)
}

/// The manifest is present and names `tool`.
fn named(manifest: Option<&[u8]>, tool: &str) -> Result<(), String> {
    let manifest = manifest.ok_or_else(|| format!("tool {tool} has no {MANIFEST}"))?;
    let name = serde_json::from_slice::<serde_json::Value>(manifest)
        .ok()
        .and_then(|manifest| manifest.get("name")?.as_str().map(str::to_owned));
    match name.as_deref() {
        Some(name) if name == tool => Ok(()),
        _ => Err(format!("tool {tool}'s {MANIFEST} does not name it {tool}")),
    }
}

fn digest(files: &BTreeMap<String, Vec<u8>>) -> Result<Digest, String> {
    tool_package_digest(
        files
            .iter()
            .map(|(path, bytes)| (path.as_str(), bytes.as_slice())),
    )
    .map_err(|error| error.to_string())
}

/// Write `files` at `copy` whole: into a sibling first, then renamed into
/// place, so a copy is never seen half written. A run that installed it
/// first wins; its bytes are the same.
fn install(copy: &Path, files: &BTreeMap<String, Vec<u8>>) -> Result<(), String> {
    let parent = copy
        .parent()
        .ok_or_else(|| format!("{} has no parent", copy.display()))?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("creating {}: {error}", parent.display()))?;
    let staging = tempfile::Builder::new()
        .prefix(".installing-")
        .tempdir_in(parent)
        .map_err(|error| format!("staging a tool in {}: {error}", parent.display()))?;
    for (path, bytes) in files {
        let file = staging.path().join(path);
        if let Some(directory) = file.parent() {
            std::fs::create_dir_all(directory)
                .map_err(|error| format!("creating {}: {error}", directory.display()))?;
        }
        std::fs::write(&file, bytes)
            .map_err(|error| format!("writing {}: {error}", file.display()))?;
    }
    let staged = staging.keep();
    match std::fs::rename(&staged, copy) {
        Ok(()) => Ok(()),
        Err(_) if copy.exists() => std::fs::remove_dir_all(&staged)
            .map_err(|error| format!("removing {}: {error}", staged.display())),
        Err(error) => Err(format!("installing {}: {error}", copy.display())),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    fn plugin_tool(connector: &Path, body: &str) -> Result<(), String> {
        let tool = connector.join("tools/acme/sync");
        std::fs::create_dir_all(&tool).map_err(|error| error.to_string())?;
        std::fs::write(
            tool.join(MANIFEST),
            r#"{"schema": "runx.tool.manifest.v1", "name": "acme.sync", "version": "0.1.0", "description": "Syncs.", "source": {"command": "node", "args": ["./sync.mjs"]}}"#,
        )
        .map_err(|error| error.to_string())?;
        std::fs::write(tool.join("sync.mjs"), body).map_err(|error| error.to_string())
    }

    fn paths(home: &Path) -> RunxPaths {
        RunxPaths::resolve(
            &BTreeMap::from([(
                crate::paths::RUNX_HOME_ENV.to_owned(),
                home.to_string_lossy().into_owned(),
            )]),
            home,
        )
    }

    /// A plugin's tool runs from its copy under the home, named by its
    /// digest; a copy someone changed is refused, and so is a source that
    /// changed after the connector was loaded.
    #[test]
    fn a_tool_runs_only_from_a_copy_matching_its_digest() -> Result<(), String> {
        let root = tempfile::tempdir().map_err(|error| error.to_string())?;
        let connector = root.path().join("connectors/acme");
        plugin_tool(&connector, "process.stdout.write('{}');\n")?;
        let tool = ConnectorTool::in_plugin(&connector, "acme.sync")?;
        let paths = paths(&root.path().join("home"));

        let manifest = tool.installed(&paths, "acme.sync")?;
        assert_eq!(
            manifest,
            paths.connector_tool(tool.digest.as_str()).join(MANIFEST)
        );
        assert_eq!(tool.installed(&paths, "acme.sync")?, manifest);

        let copy = manifest.parent().ok_or("the manifest has a directory")?;
        std::fs::write(copy.join("sync.mjs"), "process.stdout.write('leaked');\n")
            .map_err(|error| error.to_string())?;
        let refused = tool
            .installed(&paths, "acme.sync")
            .err()
            .ok_or("a changed copy ran")?;
        assert!(refused.contains("remove it and run again"), "{refused}");

        plugin_tool(&connector, "process.stdout.write('changed');\n")?;
        let refused = tool
            .installed(&paths, "acme.sync")
            .err()
            .ok_or("a changed source ran")?;
        assert!(
            refused.contains("changed after its connector was loaded"),
            "{refused}"
        );
        Ok(())
    }

    /// A tool's manifest names it; a package under another name is not it.
    #[test]
    fn a_tool_package_is_named_by_its_manifest() -> Result<(), String> {
        let root = tempfile::tempdir().map_err(|error| error.to_string())?;
        let connector = root.path().join("connectors/acme");
        plugin_tool(&connector, "")?;
        let tool = connector.join("tools/acme/other");
        std::fs::create_dir_all(&tool).map_err(|error| error.to_string())?;
        std::fs::copy(
            connector.join("tools/acme/sync").join(MANIFEST),
            tool.join(MANIFEST),
        )
        .map_err(|error| error.to_string())?;
        let refused = ConnectorTool::in_plugin(&connector, "acme.other")
            .err()
            .ok_or("a misnamed tool loaded")?;
        assert!(refused.contains("does not name it acme.other"), "{refused}");
        Ok(())
    }

    /// A plugin connector's tools are read from its directory: in the kernel
    /// fixture plugin, fixture/forge's git.push from its tools directory.
    #[test]
    fn a_source_connector_reads_its_tools_from_its_directory() -> Result<(), String> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/plugins/kernel");
        let forge = super::super::ConnectorScope::at(&root)
            .and_then(|scope| scope.connector("fixture/forge"))
            .map_err(|error| error.to_string())?;
        let push = forge
            .tools
            .get("git.push")
            .ok_or("fixture/forge ships git.push")?;
        assert_eq!(
            push.directory
                .canonicalize()
                .map_err(|error| error.to_string())?,
            root.join("connectors/fixture/forge/tools/git/push")
                .canonicalize()
                .map_err(|error| error.to_string())?
        );
        assert!(push.files("git.push")?.contains_key(MANIFEST));
        Ok(())
    }
}
