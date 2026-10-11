use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use runx_contracts::{CopyableCommand, JsonObject, JsonValue};

use crate::command::{CommandPlan, run_command};
use crate::error::{RunxError, RunxResult};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunxClientOptions {
    pub command: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: BTreeMap<String, String>,
}

impl Default for RunxClientOptions {
    fn default() -> Self {
        Self {
            command: vec!["runx".to_owned()],
            cwd: None,
            env: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunxClient {
    options: RunxClientOptions,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RunSkillOptions {
    pub runner: Option<String>,
    pub inputs: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ContinuePayload {
    pub answers: JsonObject,
    pub approvals: JsonObject,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RunxJsonReport {
    payload: JsonObject,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillSearchResult {
    pub skill_id: String,
    pub name: String,
    pub owner: String,
    pub source: String,
    pub source_label: String,
    pub trust_tier: String,
    pub required_scopes: Vec<String>,
    pub tags: Vec<String>,
    pub summary: Option<String>,
    pub version: Option<String>,
    pub digest: Option<String>,
    pub install_command: CopyableCommand,
    pub run_command: CopyableCommand,
}

impl RunxClient {
    pub fn new() -> Self {
        Self::with_options(RunxClientOptions::default())
    }

    pub fn with_command(command: Vec<String>) -> Self {
        Self::with_options(RunxClientOptions {
            command,
            ..RunxClientOptions::default()
        })
    }

    pub fn with_options(options: RunxClientOptions) -> Self {
        Self { options }
    }

    /// Searches the registry, or the registry at `registry` (a URL or a
    /// local registry directory).
    pub fn search_skills(
        &self,
        query: &str,
        registry: Option<&str>,
    ) -> RunxResult<Vec<SkillSearchResult>> {
        let mut args = vec!["registry".to_owned(), "search".to_owned(), query.to_owned()];
        if let Some(registry) = registry {
            args.push("--registry".to_owned());
            args.push(registry.to_owned());
        }
        let payload = self.run_json(args, None)?;
        let registry = json_object(
            payload
                .get("registry")
                .ok_or(RunxError::MissingField { field: "registry" })?,
            "registry",
        )?;
        let results = required_array(registry, "results")?;
        results.iter().map(search_result_from_json).collect()
    }

    pub fn run_skill(
        &self,
        skill_ref: &str,
        options: RunSkillOptions,
    ) -> RunxResult<RunxJsonReport> {
        let mut args = vec!["skill".to_owned(), skill_ref.to_owned()];
        if let Some(runner) = options.runner {
            args.push(runner);
        }
        for (name, value) in options.inputs {
            args.push(format!("--{name}"));
            args.push(value);
        }
        Ok(RunxJsonReport::new(self.run_json(args, None)?))
    }

    pub fn continue_run(
        &self,
        _skill_ref: &str,
        run_id: &str,
        payload: ContinuePayload,
    ) -> RunxResult<RunxJsonReport> {
        let answers_path = write_continue_payload(payload)?;
        let result = self.run_json(
            vec![
                "resume".to_owned(),
                run_id.to_owned(),
                answers_path.to_string_lossy().into_owned(),
            ],
            None,
        );
        let _ignored = fs::remove_file(&answers_path);
        Ok(RunxJsonReport::new(result?))
    }

    pub fn run_json(&self, args: Vec<String>, stdin: Option<String>) -> RunxResult<JsonObject> {
        let json_args = ensure_json_flag(args);
        let plan = CommandPlan::new(&self.options.command, &json_args)?
            .with_cwd(self.options.cwd.clone())
            .with_env(self.options.env.clone())
            .with_stdin(stdin);
        let output = run_command(&plan)?;
        decode_json_object(&output.stdout)
    }
}

impl Default for RunxClient {
    fn default() -> Self {
        Self::new()
    }
}

impl RunSkillOptions {
    pub fn with_input(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.inputs.insert(name.into(), value.into());
        self
    }
}

impl ContinuePayload {
    pub fn with_answer(mut self, id: impl Into<String>, value: JsonValue) -> Self {
        self.answers.insert(id.into(), value);
        self
    }

    pub fn with_approval(mut self, id: impl Into<String>, approved: bool) -> Self {
        self.approvals.insert(id.into(), JsonValue::Bool(approved));
        self
    }
}

impl RunxJsonReport {
    pub fn new(payload: JsonObject) -> Self {
        Self { payload }
    }

    pub fn status(&self) -> Option<&str> {
        optional_string_ref(&self.payload, "status")
    }

    pub fn get(&self, field: &str) -> Option<&JsonValue> {
        self.payload.get(field)
    }

    pub fn into_payload(self) -> JsonObject {
        self.payload
    }
}

fn ensure_json_flag(mut args: Vec<String>) -> Vec<String> {
    if !args.iter().any(|arg| arg == "--json") {
        args.push("--json".to_owned());
    }
    args
}

fn decode_json_object(stdout: &str) -> RunxResult<JsonObject> {
    match serde_json::from_str::<JsonValue>(stdout)? {
        JsonValue::Object(object) => Ok(object),
        _ => Err(RunxError::ExpectedObject),
    }
}

fn continue_payload_to_json(payload: ContinuePayload) -> JsonValue {
    let mut object = JsonObject::new();
    object.insert("answers".to_owned(), JsonValue::Object(payload.answers));
    object.insert("approvals".to_owned(), JsonValue::Object(payload.approvals));
    JsonValue::Object(object)
}

fn write_continue_payload(payload: ContinuePayload) -> RunxResult<PathBuf> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let path =
        std::env::temp_dir().join(format!("runx-continue-{}-{nanos}.json", std::process::id()));
    fs::write(
        &path,
        serde_json::to_vec(&continue_payload_to_json(payload))?,
    )?;
    Ok(path)
}

fn search_result_from_json(value: &JsonValue) -> RunxResult<SkillSearchResult> {
    let object = json_object(value, "results[]")?;
    Ok(SkillSearchResult {
        skill_id: required_string(object, "skill_id")?,
        name: required_string(object, "name")?,
        owner: required_string(object, "owner")?,
        source: required_string(object, "source")?,
        source_label: required_string(object, "source_label")?,
        trust_tier: required_string(object, "trust_tier")?,
        required_scopes: optional_string_array(object, "required_scopes")?,
        tags: optional_string_array(object, "tags")?,
        summary: optional_string(object, "summary")?,
        version: optional_string(object, "version")?,
        digest: optional_string(object, "digest")?,
        install_command: copyable_command(object, "install_command")?,
        run_command: copyable_command(object, "run_command")?,
    })
}

/// A command the CLI printed, refused unless its text is what its arguments
/// render to.
fn copyable_command(object: &JsonObject, field: &'static str) -> RunxResult<CopyableCommand> {
    let value = object.get(field).ok_or(RunxError::MissingField { field })?;
    serde_json::to_value(value)
        .and_then(serde_json::from_value)
        .map_err(|_| RunxError::InvalidField { field })
}

fn required_array<'a>(
    object: &'a JsonObject,
    field: &'static str,
) -> RunxResult<&'a Vec<JsonValue>> {
    match object.get(field) {
        Some(JsonValue::Array(values)) => Ok(values),
        Some(_) => Err(RunxError::InvalidField { field }),
        None => Err(RunxError::MissingField { field }),
    }
}

fn json_object<'a>(value: &'a JsonValue, field: &'static str) -> RunxResult<&'a JsonObject> {
    match value {
        JsonValue::Object(object) => Ok(object),
        _ => Err(RunxError::InvalidField { field }),
    }
}

fn required_string(object: &JsonObject, field: &'static str) -> RunxResult<String> {
    optional_string(object, field)?.ok_or(RunxError::MissingField { field })
}

fn optional_string(object: &JsonObject, field: &'static str) -> RunxResult<Option<String>> {
    match object.get(field) {
        Some(JsonValue::String(value)) => Ok(Some(value.clone())),
        Some(JsonValue::Null) | None => Ok(None),
        Some(_) => Err(RunxError::InvalidField { field }),
    }
}

fn optional_string_ref<'a>(object: &'a JsonObject, field: &str) -> Option<&'a str> {
    match object.get(field) {
        Some(JsonValue::String(value)) => Some(value.as_str()),
        _ => None,
    }
}

fn optional_string_array(object: &JsonObject, field: &'static str) -> RunxResult<Vec<String>> {
    match object.get(field) {
        Some(JsonValue::Array(values)) => values.iter().map(json_string).collect(),
        Some(JsonValue::Null) | None => Ok(Vec::new()),
        Some(_) => Err(RunxError::InvalidField { field }),
    }
}

fn json_string(value: &JsonValue) -> RunxResult<String> {
    match value {
        JsonValue::String(value) => Ok(value.clone()),
        _ => Err(RunxError::InvalidField { field: "array[]" }),
    }
}
