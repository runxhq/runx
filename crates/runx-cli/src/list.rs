use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

use crate::router::{FilterMode, ListKind, ListPlan};
use runx_runtime::{
    RunxListItem, RunxListItemKind, RunxListOptions, RunxListRequestedKind, RunxListStatus,
    list_authoring_primitives_with_effects,
};

#[derive(Debug)]
pub enum ListCliError {
    #[cfg(test)]
    Io {
        context: &'static str,
        source: std::io::Error,
    },
    Effect(runx_runtime::RuntimeEffectError),
    Runtime(runx_runtime::RuntimeError),
    Serialize(serde_json::Error),
}

impl fmt::Display for ListCliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            #[cfg(test)]
            Self::Io { context, source } => {
                write!(formatter, "test I/O failed while {context}: {source}")
            }
            Self::Runtime(error) => write!(formatter, "{error}"),
            Self::Effect(error) => {
                write!(formatter, "failed to initialize runtime effects: {error}")
            }
            Self::Serialize(error) => write!(formatter, "failed to serialize list output: {error}"),
        }
    }
}

impl std::error::Error for ListCliError {}

impl From<runx_runtime::RuntimeError> for ListCliError {
    fn from(error: runx_runtime::RuntimeError) -> Self {
        Self::Runtime(error)
    }
}

impl From<runx_runtime::RuntimeEffectError> for ListCliError {
    fn from(error: runx_runtime::RuntimeEffectError) -> Self {
        Self::Effect(error)
    }
}

impl From<serde_json::Error> for ListCliError {
    fn from(error: serde_json::Error) -> Self {
        Self::Serialize(error)
    }
}

pub fn run_list_command(
    plan: &ListPlan,
    _env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<String, ListCliError> {
    let options = RunxListOptions {
        root: cwd.to_path_buf(),
        requested_kind: requested_kind(plan.kind),
    };
    let effects = crate::runtime::runtime_effect_registry()?;
    let mut report = list_authoring_primitives_with_effects(&options, &effects)?;
    report
        .items
        .retain(|item| item_visible_for_filter(item, plan.filter));

    if plan.json {
        return Ok(format!("{}\n", serde_json::to_string_pretty(&report)?));
    }

    Ok(render_list_items(&report.items))
}

fn requested_kind(kind: ListKind) -> RunxListRequestedKind {
    match kind {
        ListKind::All => RunxListRequestedKind::All,
        ListKind::Tools => RunxListRequestedKind::Tools,
        ListKind::Skills => RunxListRequestedKind::Skills,
        ListKind::Graphs => RunxListRequestedKind::Graphs,
        ListKind::Packets => RunxListRequestedKind::Packets,
        ListKind::Overlays => RunxListRequestedKind::Overlays,
    }
}

fn item_visible_for_filter(item: &RunxListItem, filter: FilterMode) -> bool {
    match filter {
        FilterMode::All => true,
        FilterMode::OkOnly => item.status == RunxListStatus::Ok,
        FilterMode::InvalidOnly => item.status == RunxListStatus::Invalid,
    }
}

fn render_list_items(items: &[RunxListItem]) -> String {
    if items.is_empty() {
        return "No runx authoring primitives found.\n".to_owned();
    }

    let mut output = String::new();
    for item in items {
        output.push_str(&format!(
            "{}\t{}\t{}\t{}\n",
            kind_label(item.kind),
            status_label(item.status),
            item.name,
            item.path
        ));
        if let Some(diagnostics) = &item.diagnostics {
            for diagnostic in diagnostics {
                output.push_str(&format!("  diagnostic\t{diagnostic}\n"));
            }
        }
    }
    output
}

fn kind_label(kind: RunxListItemKind) -> &'static str {
    match kind {
        RunxListItemKind::Tool => "tool",
        RunxListItemKind::Skill => "skill",
        RunxListItemKind::Graph => "graph",
        RunxListItemKind::Packet => "packet",
        RunxListItemKind::Overlay => "overlay",
    }
}

fn status_label(status: RunxListStatus) -> &'static str {
    match status {
        RunxListStatus::Ok => "ok",
        RunxListStatus::Invalid => "invalid",
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[derive(serde::Deserialize)]
    struct TestListReport {
        schema: String,
        items: Vec<TestListItem>,
    }

    #[derive(serde::Deserialize)]
    struct TestListItem {
        kind: String,
        name: String,
    }

    #[test]
    fn json_lists_each_package_packet() -> Result<(), ListCliError> {
        let root = temp_workspace("packets");
        let _ignored = fs::remove_dir_all(&root);
        let package = root.join("skills/example");
        fs::create_dir_all(package.join("packets"))
            .map_err(runtime_io("creating packet fixture"))?;
        fs::write(
            package.join("SKILL.md"),
            "---\nname: example\ndescription: Example packets.\n---\n# Example\n",
        )
        .map_err(runtime_io("writing package fixture"))?;
        fs::write(
            package.join("packets/result.v1.schema.json"),
            r#"{"$id":"https://schemas.runx.ai/packages/acme/example/result/v1.json","x-runx-packet-id":"acme/example.result.v1","type":"object"}"#,
        )
        .map_err(runtime_io("writing packet fixture"))?;

        let output = run_list_command(
            &ListPlan {
                kind: ListKind::Packets,
                filter: FilterMode::OkOnly,
                json: true,
            },
            &std::collections::BTreeMap::new(),
            &root,
        )?;

        let report = serde_json::from_str::<TestListReport>(&output)?;
        assert_eq!(report.schema, "runx.list.v1");
        assert_eq!(report.items[0].kind, "packet");
        assert_eq!(report.items[0].name, "acme/example.result.v1");

        fs::remove_dir_all(root).map_err(runtime_io("removing packet fixture"))?;
        Ok(())
    }

    #[test]
    fn human_empty_list_is_stable() -> Result<(), ListCliError> {
        let root = temp_workspace("empty");
        let _ignored = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).map_err(runtime_io("creating empty fixture"))?;

        let output = run_list_command(
            &ListPlan {
                kind: ListKind::Overlays,
                filter: FilterMode::All,
                json: false,
            },
            &std::collections::BTreeMap::new(),
            &root,
        )?;

        assert_eq!(output, "No runx authoring primitives found.\n");

        fs::remove_dir_all(root).map_err(runtime_io("removing empty fixture"))?;
        Ok(())
    }

    #[test]
    fn json_lists_native_tools_without_local_manifests() -> Result<(), ListCliError> {
        let root = temp_workspace("native-tools");
        let _ignored = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).map_err(runtime_io("creating native tool fixture"))?;

        let output = run_list_command(
            &ListPlan {
                kind: ListKind::Tools,
                filter: FilterMode::OkOnly,
                json: true,
            },
            &std::collections::BTreeMap::new(),
            &root,
        )?;

        let report = serde_json::from_str::<TestListReport>(&output)?;
        assert!(
            report
                .items
                .iter()
                .any(|item| item.kind == "tool" && item.name == "fs.read")
        );

        fs::remove_dir_all(root).map_err(runtime_io("removing native tool fixture"))?;
        Ok(())
    }

    fn runtime_io(context: &'static str) -> impl FnOnce(std::io::Error) -> ListCliError {
        move |source| ListCliError::Io { context, source }
    }

    fn temp_workspace(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("runx-list-{name}-{}", std::process::id()))
    }
}
