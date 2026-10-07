#![allow(clippy::expect_used)]

use std::collections::BTreeMap;
use std::fs;

use runx_contracts::{EnvironmentRequirements, ExecutionRequirements, JsonObject, JsonValue};
use runx_parser::{ArtifactPageFraming, ArtifactPageSource, SkillSource, SourceKind};
use runx_runtime::adapter::{InvocationOutput, InvocationStatus, SkillAdapter, SkillInvocation};
use runx_runtime::adapters::javascript::JavaScriptAdapter;
use runx_runtime::credentials::CredentialDelivery;

pub(super) struct JavaScriptPackage {
    _directory: tempfile::TempDir,
    root: std::path::PathBuf,
    adapter: JavaScriptAdapter,
}

impl JavaScriptPackage {
    pub(super) fn new(source: &str) -> Result<Self, Box<dyn std::error::Error>> {
        Self::with_max_concurrency(source, 1)
    }

    pub(super) fn with_max_concurrency(
        source: &str,
        max_concurrency: usize,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Self::with_modules_and_concurrency(
            source,
            std::iter::empty::<(&str, &str)>(),
            max_concurrency,
        )
    }

    pub(super) fn with_modules<'a>(
        source: &str,
        modules: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Self::with_modules_and_concurrency(source, modules, 1)
    }

    fn with_modules_and_concurrency<'a>(
        source: &str,
        modules: impl IntoIterator<Item = (&'a str, &'a str)>,
        max_concurrency: usize,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let root = directory.path().join("deterministic-module");
        fs::create_dir_all(&root)?;
        fs::write(
            root.join("SKILL.md"),
            "---\nname: deterministic-module\ndescription: Exercise the isolated deterministic module worker.\n---\n\n# Deterministic module\n\nCompute bounded JSON without host authority.\n",
        )?;
        fs::write(
            root.join("X.yaml"),
            "skill: deterministic-module\nrunners:\n  run:\n    default: true\n    type: javascript\n    module: main.mjs\n",
        )?;
        fs::write(root.join("main.mjs"), source)?;
        for (module, contents) in modules {
            let path = root.join(module);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(path, contents)?;
        }
        Ok(Self {
            _directory: directory,
            root,
            adapter: JavaScriptAdapter::with_max_concurrency(max_concurrency),
        })
    }

    pub(super) fn invoke(
        &self,
        inputs: JsonObject,
    ) -> Result<InvocationOutput, runx_runtime::RuntimeError> {
        self.invoke_source(inputs, javascript_source(), BTreeMap::new())
    }

    pub(super) fn invoke_with_environment(
        &self,
        inputs: JsonObject,
        requirements: EnvironmentRequirements,
        environment: BTreeMap<String, String>,
    ) -> Result<InvocationOutput, runx_runtime::RuntimeError> {
        let mut source = javascript_source();
        source.environment = requirements;
        self.invoke_source(inputs, source, environment)
    }

    pub(super) fn invoke_export(
        &self,
        export: &str,
        inputs: JsonObject,
    ) -> Result<InvocationOutput, runx_runtime::RuntimeError> {
        let mut source = javascript_source();
        source.javascript_export = Some(export.to_owned());
        self.invoke_source(inputs, source, BTreeMap::new())
    }

    pub(super) fn invoke_paged(
        &self,
        archive: &str,
        contents: &str,
        page_bytes: u64,
        inputs: JsonObject,
    ) -> Result<InvocationOutput, Box<dyn std::error::Error>> {
        self.invoke_paged_with_export(archive, contents, page_bytes, None, inputs)
    }

    pub(super) fn invoke_paged_export(
        &self,
        archive: &str,
        contents: &str,
        page_bytes: u64,
        export: &str,
        inputs: JsonObject,
    ) -> Result<InvocationOutput, Box<dyn std::error::Error>> {
        self.invoke_paged_with_export(archive, contents, page_bytes, Some(export), inputs)
    }

    fn invoke_paged_with_export(
        &self,
        archive: &str,
        contents: &str,
        page_bytes: u64,
        export: Option<&str>,
        mut inputs: JsonObject,
    ) -> Result<InvocationOutput, Box<dyn std::error::Error>> {
        let archive_path = self.root.join(archive);
        if let Some(parent) = archive_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(archive_path, contents)?;
        inputs.insert(
            "archive_file".to_owned(),
            JsonValue::String(archive.to_owned()),
        );
        inputs.insert(
            "archive_base".to_owned(),
            JsonValue::String("skill".to_owned()),
        );
        let mut source = javascript_source();
        source.javascript_export = export.map(str::to_owned);
        source.pages = Some(ArtifactPageSource {
            path_from: "archive_file".to_owned(),
            path_scope_from: Some("archive_base".to_owned()),
            media_type: "application/javascript".to_owned(),
            framing: ArtifactPageFraming::JsonArray,
            page_bytes,
        });
        Ok(self.invoke_source(inputs, source, BTreeMap::new())?)
    }

    fn invoke_source(
        &self,
        inputs: JsonObject,
        source: SkillSource,
        env: BTreeMap<String, String>,
    ) -> Result<InvocationOutput, runx_runtime::RuntimeError> {
        let requirements = ExecutionRequirements {
            environment: source.environment.clone(),
            ..ExecutionRequirements::default()
        };
        self.adapter.invoke(SkillInvocation {
            historical_context: Vec::new(),
            skill_name: "deterministic-module".to_owned(),
            step_id: None,
            artifacts: None,
            allowed_tools: None,
            source,
            requirements,
            inputs,
            resolved_inputs: JsonObject::new(),
            current_context: Vec::new(),
            provenance: Vec::new(),
            skill_directory: self.root.clone(),
            env,
            credential_delivery: CredentialDelivery::none(),
        })
    }

    pub(super) fn session_stats(
        &self,
    ) -> runx_runtime::adapters::javascript::JavaScriptSessionStats {
        self.adapter.session_stats()
    }
}

pub(super) fn success_json(
    output: &InvocationOutput,
) -> Result<JsonValue, Box<dyn std::error::Error>> {
    if output.status != InvocationStatus::Success {
        return Err(format!(
            "JavaScript invocation failed: {}",
            output
                .failure_message()
                .unwrap_or_else(|| "no diagnostic".to_owned())
        )
        .into());
    }
    Ok(output.value.clone())
}

pub(super) fn expected_json(value: serde_json::Value) -> JsonValue {
    serde_json::from_value(value).expect("test JSON must satisfy the Runx JSON contract")
}

fn javascript_source() -> SkillSource {
    SkillSource {
        source_type: SourceKind::JavaScript,
        command: None,
        module: Some("main.mjs".to_owned()),
        javascript_export: None,
        pages: None,
        args: Vec::new(),
        cwd: None,
        timeout_seconds: None,
        input_mode: None,
        environment: EnvironmentRequirements::default(),
        agent: None,
        task: None,
        outputs: None,
        graph: None,
        act: None,
        raw: JsonObject::new(),
    }
}
