// Module rationale: skill command keeps parse, inspect, entry provenance, and execution wiring together until the native skill UX settles.
use std::collections::BTreeMap;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use runx_contracts::{CheckOutcome, JsonObject, JsonValue, Readiness};
use runx_runtime::skill_front::{PreparedEntryProvenance, PreparedSkillRunStatus};
use runx_runtime::{ManagedAgentPolicy, OrchestratorError, SkillRunRequest, WorkspaceEnv};

mod environment_readiness;
mod inputs;
mod marketplace;
mod operator_context;
pub(crate) mod output;
mod parser;
mod resolver;

use environment_readiness::{
    append_text as append_environment_readiness_text, inspect as inspect_environment_readiness,
};
pub use inputs::CliInputs;
use inputs::read_input_document;
use operator_context::write_operator_context;
use output::{ResumeHint, run_result_exit_code, write_skill_output};
pub use parser::{parse_skill_plan, parse_skill_plan_with_workspace};
use resolver::{RegistryTrustState, ResolvedSkillRef, resolve_skill_ref_details};

#[derive(Debug, PartialEq)]
pub struct SkillPlan {
    pub action: SkillAction,
    pub skill_path: PathBuf,
    pub runner: Option<String>,
    pub receipt_dir: Option<PathBuf>,
    pub run_id: Option<String>,
    pub answers: Option<PathBuf>,
    pub registry: Option<String>,
    pub expected_digest: Option<String>,
    pub expected_package_digest: Option<String>,
    pub expected_execution_closure_digest: Option<String>,
    pub json: bool,
    pub diagnostics: bool,
    /// Internal command authorization for composite CLI commands such as
    /// `runx new`. User-supplied `runx skill` invocations cannot set this.
    pub trusted_command_execution: bool,
    pub full_operator_context: bool,
    pub inputs: CliInputs,
    pub input_document: Option<crate::document_input::DocumentInputSource>,
    /// The act the invoker states for the run (`--act`).
    pub act: Option<runx_contracts::CallAct>,
    /// The trusted host's authority for the run (`--run-authority`).
    pub run_authority: Option<Box<runx_runtime::RunAuthority>>,
    pub managed_agent: ManagedAgentPolicy,
}

#[derive(Debug, PartialEq)]
pub enum SkillAction {
    Inspect,
    Run,
}

// Function rationale: the top-level command path owns resolve/inspect/run/failure presentation in one explicit dispatch.
pub fn run_native_skill_with_workspace(plan: SkillPlan, workspace: &WorkspaceEnv) -> ExitCode {
    let cwd = workspace.cwd().to_path_buf();
    let env = workspace.env().clone();
    let artifacts = workspace.paths().artifacts();
    let mut resolved = match resolve_skill_ref_details(
        &plan.skill_path,
        &cwd,
        resolver::SkillResolverOptions {
            env: &env,
            registry: plan.registry.as_deref(),
            expected_digest: plan.expected_digest.as_deref(),
        },
    ) {
        Ok(skill_path) => skill_path,
        Err(error) => {
            return write_skill_failure(&error.to_string(), plan.json, "skill_error", 1, None);
        }
    };
    if let Some(expected_package_digest) = &plan.expected_package_digest {
        resolved.package_digest = Some(expected_package_digest.clone());
    }
    let skill_path = resolved.runnable_path.clone();
    if plan.action == SkillAction::Inspect {
        return write_skill_inspection(
            &skill_path,
            plan.runner.as_deref(),
            plan.json,
            entry_provenance(&resolved),
            &env,
            &cwd,
        );
    }
    let inputs = match plan.input_document.as_ref() {
        Some(source) => match read_input_document(source, &env, &cwd) {
            Ok(inputs) => inputs,
            Err(error) => {
                return write_skill_failure(
                    &error,
                    plan.json,
                    "input_error",
                    1,
                    entry_provenance(&resolved),
                );
            }
        },
        None => match plan.inputs.typed(&skill_path, plan.runner.as_deref()) {
            Ok(inputs) => inputs,
            Err(error) => {
                return write_skill_failure(
                    &error,
                    plan.json,
                    "input_error",
                    1,
                    entry_provenance(&resolved),
                );
            }
        },
    };
    if resolved.paid_listing.is_some() && resolved.hosted_registry_url.is_some() {
        let mut output = match marketplace::discover_paid_skill(
            &resolved,
            plan.runner.as_deref(),
            &inputs,
            &env,
        ) {
            Ok(output) => output,
            Err(error) => {
                return write_skill_failure(
                    &error,
                    plan.json,
                    "marketplace_error",
                    1,
                    entry_provenance(&resolved),
                );
            }
        };
        attach_entry_provenance(&mut output, &resolved);
        let exit_code = ExitCode::from(2);
        return write_skill_output(
            &output,
            plan.json,
            exit_code,
            ResumeHint {
                receipt_dir: plan.receipt_dir.as_deref(),
                answers_path: plan.answers.as_deref(),
            },
            &artifacts,
            plan.diagnostics,
        );
    }
    let resume = ResumeHint {
        receipt_dir: plan.receipt_dir.as_deref(),
        answers_path: plan.answers.as_deref(),
    };
    let request = SkillRunRequest {
        current_context: Vec::new(),
        historical_context: Vec::new(),
        run_authority: plan.run_authority.as_deref().cloned(),
        act: plan.act.clone(),
        skill_path,
        receipt_dir: plan.receipt_dir.clone(),
        run_id: plan.run_id.clone(),
        answers_path: plan.answers.clone(),
        inputs,
        env,
        cwd,
        managed_agent: plan.managed_agent.clone(),
    };
    let orchestrator = match crate::runtime::local_orchestrator(&request.env) {
        Ok(orchestrator) => orchestrator,
        Err(error) => {
            return write_skill_failure(
                &format!("failed to initialize runtime effects: {error}"),
                plan.json,
                "skill_error",
                1,
                entry_provenance(&resolved),
            );
        }
    };
    let bound_execution =
        plan.expected_package_digest.is_some() && plan.expected_execution_closure_digest.is_some();
    let result = if bound_execution {
        orchestrator.run_skill_with_binding(
            &request,
            plan.runner.as_deref(),
            plan.expected_package_digest.as_deref(),
            plan.expected_execution_closure_digest.as_deref(),
        )
    } else if plan.trusted_command_execution {
        match plan.runner.as_deref() {
            Some(runner) => orchestrator.run_skill_with_runner(&request, runner),
            None => orchestrator.run_skill(&request),
        }
    } else {
        let mut prepared = match orchestrator.prepare_skill(
            request,
            plan.runner.as_deref(),
            prepared_entry_provenance(&resolved, plan.expected_execution_closure_digest.as_deref()),
        ) {
            Ok(prepared) => prepared,
            Err(error) => {
                return write_orchestrator_failure(
                    &error,
                    plan.json,
                    "skill_error",
                    1,
                    entry_provenance(&resolved),
                );
            }
        };
        // Operator context is written to stderr, so it never pollutes a
        // --json stdout payload; every invocation gets the same preflight facts.
        if let Err(error) = write_operator_context(prepared.report(), plan.full_operator_context) {
            return write_skill_failure(
                &error,
                plan.json,
                "skill_error",
                1,
                entry_provenance(&resolved),
            );
        }
        if prepared.report().status == PreparedSkillRunStatus::Blocked {
            let report = prepared.report();
            let message = report
                .blocked_reason
                .as_deref()
                .unwrap_or("operator context preparation blocked");
            let detail = report.refusal_receipt_id.as_ref().map(|receipt_id| {
                JsonObject::from([
                    (
                        "receipt_id".to_owned(),
                        JsonValue::String(receipt_id.clone()),
                    ),
                    (
                        "prepared_context_digest".to_owned(),
                        JsonValue::String(report.digest.clone()),
                    ),
                ])
            });
            return write_skill_failure_with_detail(
                message,
                plan.json,
                "operator_context_blocked",
                1,
                entry_provenance(&resolved),
                detail,
            );
        }
        if let Err(error) = prepared.bind_context() {
            return write_skill_failure(
                &error.to_string(),
                plan.json,
                "operator_context_admission_error",
                1,
                entry_provenance(&resolved),
            );
        }
        orchestrator.run_prepared_skill(&prepared)
    };
    match result {
        Ok(mut result) => {
            attach_entry_provenance(&mut result.output, &resolved);
            write_run_result(
                &result,
                RunOutput {
                    json: plan.json,
                    diagnostics: plan.diagnostics,
                    receipt_dir: resume.receipt_dir,
                    answers_path: resume.answers_path,
                    artifacts: &artifacts,
                },
            )
        }
        Err(error) => write_orchestrator_failure(
            &error,
            plan.json,
            "skill_error",
            1,
            entry_provenance(&resolved),
        ),
    }
}

/// How a run's result is written.
pub(crate) struct RunOutput<'a> {
    pub(crate) json: bool,
    pub(crate) diagnostics: bool,
    pub(crate) receipt_dir: Option<&'a Path>,
    pub(crate) answers_path: Option<&'a Path>,
    pub(crate) artifacts: &'a Path,
}

/// Write a run's result: sealed, or the requests it waits on and how to
/// resume it.
pub(crate) fn write_run_result(
    result: &runx_runtime::RunResult,
    output: RunOutput<'_>,
) -> ExitCode {
    write_skill_output(
        &result.output,
        output.json,
        run_result_exit_code(result),
        ResumeHint {
            receipt_dir: output.receipt_dir,
            answers_path: output.answers_path,
        },
        output.artifacts,
        output.diagnostics,
    )
}

fn prepared_entry_provenance(
    resolved: &ResolvedSkillRef,
    execution_closure_digest: Option<&str>,
) -> PreparedEntryProvenance {
    PreparedEntryProvenance {
        kind: match resolved.kind {
            resolver::SkillRefKind::ExplicitPath => "explicit_path",
            resolver::SkillRefKind::ExportedShim => "exported_shim",
            resolver::SkillRefKind::WorkspaceLocal => "workspace_local",
            resolver::SkillRefKind::Installed => "installed",
            resolver::SkillRefKind::Bundled => "bundled",
            resolver::SkillRefKind::Registry => "registry",
        }
        .to_owned(),
        reference: resolved.skill_id.clone(),
        source: resolved.registry_source.clone().unwrap_or_else(|| {
            if resolved.kind == resolver::SkillRefKind::Bundled {
                "bundled:runx".to_owned()
            } else {
                "local-path".to_owned()
            }
        }),
        source_label: resolved
            .registry_source_fingerprint
            .clone()
            .unwrap_or_else(|| {
                if resolved.kind == resolver::SkillRefKind::Bundled {
                    "Bundled Runx core".to_owned()
                } else {
                    resolved.runnable_path.to_string_lossy().into_owned()
                }
            }),
        skill_id: resolved.skill_id.clone(),
        version: resolved.version.clone(),
        digest: resolved.digest.clone(),
        package_digest: resolved.package_digest.clone(),
        execution_closure_digest: execution_closure_digest.map(str::to_owned),
        trust_tier: resolved.trust_tier.clone(),
    }
}

fn write_skill_inspection(
    skill_path: &Path,
    runner: Option<&str>,
    json: bool,
    provenance: Option<JsonObject>,
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> ExitCode {
    match inspect_skill(skill_path, runner, provenance, env, cwd) {
        Ok((value, _)) if json => crate::cli_io::write_stdout_code(
            &format!(
                "{}\n",
                serde_json::to_string_pretty(&value).unwrap_or_else(|_| "{}".to_owned())
            ),
            0,
        ),
        Ok((value, readiness)) => write_inspection_text(&value, &readiness),
        Err(message) => write_skill_failure(&message, json, "skill_error", 1, None),
    }
}

// Function rationale: inspection assembles one public JSON contract from SKILL.md, run.yaml, fixtures, and selected runner metadata.
fn inspect_skill(
    skill_path: &Path,
    selected_runner: Option<&str>,
    provenance: Option<JsonObject>,
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<(JsonValue, Vec<Readiness>), String> {
    let mut output = runx_runtime::inspect_skill_package(skill_path, selected_runner, Some(env))
        .map_err(|error| error.to_string())?;
    let JsonValue::Object(object) = &mut output else {
        return Err("native skill inspection returned a non-object".to_owned());
    };
    if let Some(provenance) = provenance {
        object.insert("entry_provenance".to_owned(), JsonValue::Object(provenance));
    }
    let mut readiness = Vec::new();
    if object.get("runner").is_some() {
        readiness =
            runx_runtime::inspect_connector_readiness(skill_path, selected_runner, env, cwd)
                .map_err(|error| error.to_string())?;
        if !readiness.is_empty() {
            let status = if readiness.iter().all(|operation| operation.ready) {
                "ready"
            } else {
                "needs_connector_setup"
            };
            object.insert(
                "connector_readiness".to_owned(),
                JsonValue::from_serialize(&readiness).map_err(|error| error.to_string())?,
            );
            object.insert(
                "readiness".to_owned(),
                JsonValue::Object(JsonObject::from([(
                    "status".to_owned(),
                    JsonValue::String(status.to_owned()),
                )])),
            );
        }
    }
    if object.get("runner").is_some()
        && let Some(environment) = inspect_environment_readiness(object, env)?
    {
        let ready = environment
            .as_object()
            .and_then(|value| value.get("status"))
            .and_then(JsonValue::as_str)
            == Some("ready");
        object.insert("environment".to_owned(), environment);
        if !ready {
            object.insert(
                "readiness".to_owned(),
                JsonValue::Object(JsonObject::from([(
                    "status".to_owned(),
                    JsonValue::String("needs_environment".to_owned()),
                )])),
            );
        }
    }
    Ok((output, readiness))
}

// Function rationale: text rendering mirrors the inspect JSON shape and is kept adjacent to avoid presentation drift.
fn write_inspection_text(value: &JsonValue, readiness: &[Readiness]) -> ExitCode {
    let Some(object) = value.as_object() else {
        return crate::cli_io::write_stdout_code("{}\n", 0);
    };
    let mut out = String::new();
    out.push_str(&format!(
        "skill: {}\n",
        object_string(object, "name").unwrap_or("<unnamed>")
    ));
    if let Some(description) = object_string(object, "description") {
        out.push_str(&format!("description: {description}\n"));
    }
    if let Some(version) = object_string(object, "version") {
        out.push_str(&format!("version: {version}\n"));
    }
    if let Some(runner) = object.get("runner").and_then(JsonValue::as_object) {
        out.push_str(&format!(
            "runner: {}\n",
            object_string(runner, "name").unwrap_or("<unknown>")
        ));
        if let Some(kind) = object_string(runner, "type") {
            out.push_str(&format!("type: {kind}\n"));
        }
        if let Some(readiness) = object.get("readiness").and_then(JsonValue::as_object)
            && let Some(status) = object_string(readiness, "status")
        {
            out.push_str(&format!("readiness: {status}\n"));
        }
        out.extend(readiness.iter().map(readiness_line));
        append_environment_readiness_text(&mut out, object);
        if let Some(capabilities) = object.get("capabilities").and_then(JsonValue::as_object) {
            for key in ["execution", "completion", "approval"] {
                if let Some(value) = capabilities.get(key) {
                    out.push_str(&format!("{key}: {}\n", display_json_scalar(value)));
                }
            }
        }
        if let Some(inputs) = runner.get("inputs").and_then(JsonValue::as_array)
            && !inputs.is_empty()
        {
            out.push_str("inputs:\n");
            for input in inputs {
                if let Some(input) = input.as_object() {
                    let name = object_string(input, "name").unwrap_or("<unknown>");
                    let kind = object_string(input, "type").unwrap_or("json");
                    let required = input
                        .get("required")
                        .and_then(JsonValue::as_bool)
                        .unwrap_or(false);
                    let marker = if required { "required" } else { "optional" };
                    out.push_str(&format!("  - {name}: {kind} ({marker})\n"));
                }
            }
        }
        if let Some(outputs) = runner.get("outputs").and_then(JsonValue::as_array)
            && !outputs.is_empty()
        {
            out.push_str("outputs:\n");
            for output in outputs {
                if let Some(output) = output.as_object() {
                    let name = object_string(output, "name").unwrap_or("<unknown>");
                    let kind = object_string(output, "type").unwrap_or("json");
                    out.push_str(&format!("  - {name}: {kind}\n"));
                }
            }
        }
        if let Some(examples) = object.get("examples").and_then(JsonValue::as_array)
            && !examples.is_empty()
        {
            out.push_str("examples:\n");
            for example in examples {
                if let Some(example) = example.as_str() {
                    out.push_str(&format!("  - {example}\n"));
                }
            }
        }
        if let Some(resume) = object.get("resume").and_then(JsonValue::as_object)
            && resume
                .get("may_pause")
                .and_then(JsonValue::as_bool)
                .unwrap_or(false)
        {
            out.push_str(&format!(
                "resume: {}\n",
                object_string(resume, "command").unwrap_or("runx resume <run-id> -")
            ));
        }
        out.push_str("run: runx skill <skill> [runner]\n");
    } else if let Some(runners) = object.get("runners").and_then(JsonValue::as_array) {
        out.push_str("runners:\n");
        for runner in runners {
            if let Some(runner) = runner.as_str() {
                out.push_str(&format!("  - {runner}\n"));
            }
        }
        out.push_str("next: runx skill <skill> <runner>\n");
    }
    crate::cli_io::write_stdout_code(&out, 0)
}

fn display_json_scalar(value: &JsonValue) -> String {
    value
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| serde_json::to_string(value).unwrap_or_else(|_| "null".to_owned()))
}

fn object_string<'a>(object: &'a JsonObject, key: &str) -> Option<&'a str> {
    object.get(key).and_then(JsonValue::as_str)
}

fn attach_entry_provenance(output: &mut JsonValue, resolved: &ResolvedSkillRef) {
    let Some(provenance) = entry_provenance(resolved) else {
        return;
    };
    let JsonValue::Object(object) = output else {
        return;
    };
    object.insert("entry_provenance".to_owned(), JsonValue::Object(provenance));
}

fn entry_provenance(resolved: &ResolvedSkillRef) -> Option<JsonObject> {
    resolved.skill_id.as_ref()?;
    let JsonValue::Object(mut provenance) =
        JsonValue::from_serialize(&prepared_entry_provenance(resolved, None)).ok()?
    else {
        return None;
    };
    insert_optional(
        &mut provenance,
        "profile_digest",
        resolved.profile_digest.as_ref(),
    );
    insert_optional(
        &mut provenance,
        "registry_source",
        resolved.registry_source.as_ref(),
    );
    insert_optional(
        &mut provenance,
        "registry_source_fingerprint",
        resolved.registry_source_fingerprint.as_ref(),
    );
    insert_optional(
        &mut provenance,
        "registry_key_id",
        resolved.registry_key_id.as_ref(),
    );
    if matches!(
        resolved.trust_state.as_ref(),
        Some(RegistryTrustState::Trusted)
    ) {
        provenance.insert(
            "trust_state".to_owned(),
            JsonValue::String("trusted".to_owned()),
        );
    }
    Some(provenance)
}

fn insert_optional(object: &mut JsonObject, key: &str, value: Option<&String>) {
    if let Some(value) = value {
        object.insert(key.to_owned(), JsonValue::String(value.clone()));
    }
}

fn write_skill_failure(
    message: &str,
    json: bool,
    code: &str,
    exit_code: u8,
    provenance: Option<JsonObject>,
) -> ExitCode {
    write_skill_failure_with_detail(message, json, code, exit_code, provenance, None)
}

fn write_orchestrator_failure(
    error: &OrchestratorError,
    json: bool,
    code: &str,
    exit_code: u8,
    provenance: Option<JsonObject>,
) -> ExitCode {
    write_skill_failure_with_detail(
        &error.to_string(),
        json,
        code,
        exit_code,
        provenance,
        input_contract_detail(error),
    )
}

fn write_skill_failure_with_detail(
    message: &str,
    json: bool,
    code: &str,
    exit_code: u8,
    provenance: Option<JsonObject>,
    detail: Option<JsonObject>,
) -> ExitCode {
    if json {
        let output = skill_json_failure_output(message, code, provenance, detail);
        return crate::cli_io::write_stdout_code(&output, exit_code);
    }
    let _ignored = writeln!(io::stderr(), "runx: {message}");
    ExitCode::from(exit_code)
}

fn input_contract_detail(error: &OrchestratorError) -> Option<JsonObject> {
    let (runtime, receipt_id) = match error {
        OrchestratorError::SkillRun(
            runx_runtime::execution::skill_front::SkillRunError::Runtime(runtime),
        )
        | OrchestratorError::Runtime(runtime) => (runtime, None),
        OrchestratorError::SkillRun(
            runx_runtime::execution::skill_front::SkillRunError::PreflightRefused {
                source,
                receipt_id,
            },
        ) => (source.as_ref(), Some(receipt_id)),
        _ => return None,
    };
    let runx_runtime::RuntimeError::InputContract {
        owner,
        input,
        path,
        accepted_schema,
        ..
    } = runtime
    else {
        return None;
    };
    let mut detail = JsonObject::from([
        ("owner".to_owned(), JsonValue::String((*owner).to_owned())),
        ("input".to_owned(), JsonValue::String(input.clone())),
        ("path".to_owned(), JsonValue::String(path.clone())),
        (
            "accepted_schema".to_owned(),
            accepted_schema.as_ref().clone(),
        ),
    ]);
    if let Some(receipt_id) = receipt_id {
        detail.insert(
            "receipt_id".to_owned(),
            JsonValue::String(receipt_id.clone()),
        );
    }
    Some(detail)
}

fn skill_json_failure_output(
    message: &str,
    code: &str,
    provenance: Option<JsonObject>,
    detail: Option<JsonObject>,
) -> String {
    let mut error = JsonObject::new();
    error.insert("message".to_owned(), JsonValue::String(message.to_owned()));
    error.insert("code".to_owned(), JsonValue::String(code.to_owned()));
    if let Some(detail) = detail {
        error.extend(detail);
    }
    let mut output = JsonObject::new();
    output.insert("status".to_owned(), JsonValue::String("failure".to_owned()));
    output.insert("error".to_owned(), JsonValue::Object(error));
    if let Some(provenance) = provenance {
        output.insert("entry_provenance".to_owned(), JsonValue::Object(provenance));
    }
    serde_json::to_string_pretty(&JsonValue::Object(output))
        .map(|json| format!("{json}\n"))
        .unwrap_or_else(|_| crate::router::json_failure_output(message, code))
}

/// One required operation: where it would run and what it still needs.
pub(crate) fn readiness_line(readiness: &Readiness) -> String {
    let blocker = readiness
        .reasons
        .iter()
        .find(|reason| reason.outcome == CheckOutcome::Fail)
        .map(|reason| reason.detail.as_str());
    format!(
        "connector: {}/{} ({}{})\n",
        readiness
            .connector
            .as_ref()
            .map_or(readiness.requirement.as_str(), |connector| connector
                .as_str()),
        readiness.operation,
        readiness
            .instance_id
            .as_ref()
            .map_or("no instance", |instance| instance.as_str()),
        match (readiness.ready, blocker) {
            (true, _) => ", ready".to_owned(),
            (false, Some(detail)) => format!(", {detail}"),
            (false, None) => ", not ready".to_owned(),
        }
    )
}
