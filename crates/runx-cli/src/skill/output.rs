use std::io::{self, Write};
use std::path::Path;
use std::process::ExitCode;

use runx_contracts::{ClosureDisposition, JsonObject, JsonValue, sha256_prefixed};

use runx_runtime::persist_json_artifact;
#[cfg(test)]
use runx_runtime::read_json_artifact;

const MAX_COMPACT_OUTPUT_BYTES: usize = 16 * 1024;
const MAX_COMPACT_SUMMARY_CHARS: usize = 512;

pub(super) fn write_skill_output(
    value: &JsonValue,
    json: bool,
    exit_code: ExitCode,
    resume: ResumeHint<'_>,
    artifacts: &Path,
    diagnostics: bool,
) -> ExitCode {
    if !json {
        return write_text_with_exit(value, exit_code, resume);
    }
    write_json_with_exit(value, exit_code, artifacts, diagnostics)
}

#[derive(Clone, Copy)]
pub(super) struct ResumeHint<'a> {
    pub(super) receipt_dir: Option<&'a Path>,
    pub(super) answers_path: Option<&'a Path>,
}

pub(super) fn run_result_exit_code(
    result: &runx_runtime::execution::orchestrator::RunResult,
) -> ExitCode {
    if let Some(disposition) = &result.disposition {
        closure_disposition_exit_code(disposition.clone())
    } else if result.needs_resolution() {
        ExitCode::from(2)
    } else {
        ExitCode::from(1)
    }
}

fn closure_disposition_exit_code(disposition: ClosureDisposition) -> ExitCode {
    match disposition {
        ClosureDisposition::Closed => ExitCode::SUCCESS,
        ClosureDisposition::Deferred => ExitCode::from(2),
        ClosureDisposition::Superseded
        | ClosureDisposition::Declined
        | ClosureDisposition::Blocked
        | ClosureDisposition::Failed
        | ClosureDisposition::Killed
        | ClosureDisposition::TimedOut => ExitCode::from(1),
    }
}

fn write_json_with_exit(
    value: &JsonValue,
    exit_code: ExitCode,
    artifacts: &Path,
    diagnostics: bool,
) -> ExitCode {
    match serialize_json_output(value, artifacts, diagnostics) {
        Ok(json) => {
            let mut stdout = io::stdout().lock();
            let result = stdout
                .write_all(json.as_bytes())
                .and_then(|_| stdout.write_all(b"\n"));
            match result {
                Ok(()) => exit_code,
                Err(_) => ExitCode::from(1),
            }
        }
        Err(error) => {
            let _ignored = writeln!(
                io::stderr(),
                "runx: failed to serialize skill result: {error}"
            );
            ExitCode::from(1)
        }
    }
}

fn serialize_json_output(
    value: &JsonValue,
    artifacts: &Path,
    diagnostics: bool,
) -> Result<String, String> {
    let output = project_json_output(value, artifacts, diagnostics)?;
    serde_json::to_string(&output).map_err(|error| error.to_string())
}

fn project_json_output(
    value: &JsonValue,
    artifacts: &Path,
    diagnostics: bool,
) -> Result<JsonValue, String> {
    let output = project_json_output_value(value, artifacts, diagnostics)?;
    if output.as_object().is_some() {
        let wire = serde_json::to_value(&output).map_err(|error| error.to_string())?;
        serde_json::from_value::<runx_contracts::SkillOutput>(wire)
            .map_err(|error| format!("invalid native skill output envelope: {error}"))?;
    }
    Ok(output)
}

fn project_json_output_value(
    value: &JsonValue,
    artifacts: &Path,
    diagnostics: bool,
) -> Result<JsonValue, String> {
    let JsonValue::Object(mut object) = value.clone() else {
        return Ok(value.clone());
    };

    object.insert(
        "outcome".to_owned(),
        JsonValue::String(semantic_outcome(&object).to_owned()),
    );
    normalize_pending_status(&mut object);
    if diagnostics {
        return Ok(JsonValue::Object(object));
    }

    let run_id = object_string(&object, "run_id")
        .unwrap_or("unbound-run")
        .to_owned();
    let mut diagnostic_payload = JsonObject::new();
    for key in ["context", "trace", "execution"] {
        if let Some(value) = object.remove(key) {
            diagnostic_payload.insert(key.to_owned(), value);
        }
    }
    if !diagnostic_payload.is_empty() {
        let reference = persist_json_artifact(
            artifacts,
            &run_id,
            "diagnostics",
            "execution",
            &JsonValue::Object(diagnostic_payload),
        )?;
        object.insert("diagnostics_ref".to_owned(), reference.clone());
        append_artifact_refs(&mut object, vec![reference]);
    }
    if let Some(requests) = object
        .get("requests")
        .and_then(JsonValue::as_array)
        .cloned()
    {
        let mut request_summaries = Vec::with_capacity(requests.len());
        let mut refs = Vec::with_capacity(requests.len());
        for request in &requests {
            let reference = persist_json_artifact(
                artifacts,
                &run_id,
                "requests",
                request_id(request).unwrap_or("request"),
                request,
            )?;
            refs.push(reference.clone());
            request_summaries.push(compact_request(request, reference));
        }
        object.insert("requests".to_owned(), JsonValue::Array(request_summaries));
        append_artifact_refs(&mut object, refs);
    }
    insert_next_command(&mut object);

    if serialized_len(&JsonValue::Object(object.clone()))? <= MAX_COMPACT_OUTPUT_BYTES {
        return Ok(JsonValue::Object(object));
    }

    let result_reference = if let Some(result) = object.get("result").cloned() {
        let reference = persist_json_artifact(artifacts, &run_id, "results", "result", &result)?;
        let schema = result_schema(&result).to_owned();
        object.insert(
            "result".to_owned(),
            JsonValue::Object(JsonObject::from([
                ("schema".to_owned(), JsonValue::String(schema)),
                ("artifact_ref".to_owned(), reference.clone()),
            ])),
        );
        append_artifact_refs(&mut object, vec![reference.clone()]);
        Some(reference)
    } else {
        None
    };

    if serialized_len(&JsonValue::Object(object.clone()))? <= MAX_COMPACT_OUTPUT_BYTES {
        return Ok(JsonValue::Object(object));
    }

    let request_set_reference = object
        .get("requests")
        .filter(|value| {
            value
                .as_array()
                .is_some_and(|requests| !requests.is_empty())
        })
        .cloned()
        .map(|requests| {
            persist_json_artifact(artifacts, &run_id, "requests", "request-set", &requests)
        })
        .transpose()?;
    if let Some(reference) = &request_set_reference {
        object.insert("requests".to_owned(), JsonValue::Array(Vec::new()));
        object.insert("request_set_ref".to_owned(), reference.clone());
    }

    if serialized_len(&JsonValue::Object(object.clone()))? <= MAX_COMPACT_OUTPUT_BYTES {
        return Ok(JsonValue::Object(object));
    }

    let full_output_reference =
        persist_json_artifact(artifacts, &run_id, "diagnostics", "full-output", value)?;
    let mut minimal = compact_identity(&object);
    if let Some(result) = object.get("result") {
        minimal.insert("result".to_owned(), result.clone());
    } else if let Some(reference) = result_reference {
        minimal.insert(
            "result".to_owned(),
            JsonValue::Object(JsonObject::from([("artifact_ref".to_owned(), reference)])),
        );
    }
    minimal.insert("requests".to_owned(), JsonValue::Array(Vec::new()));
    if let Some(reference) = request_set_reference {
        minimal.insert("request_set_ref".to_owned(), reference);
    }
    if let Some(next) = object.get("next") {
        minimal.insert("next".to_owned(), next.clone());
    }
    minimal.insert(
        "artifact_refs".to_owned(),
        JsonValue::Array(vec![full_output_reference]),
    );
    let minimal = JsonValue::Object(minimal);
    if serialized_len(&minimal)? > MAX_COMPACT_OUTPUT_BYTES {
        return Err("compact skill result exceeded the 16 KiB envelope bound".to_owned());
    }
    Ok(minimal)
}

fn semantic_outcome(object: &JsonObject) -> &'static str {
    if let Some(trace_status) = object
        .get("trace")
        .and_then(JsonValue::as_object)
        .and_then(|trace| object_string(trace, "status"))
    {
        match trace_status {
            "failed" | "killed" | "timed_out" => return "failed",
            "blocked" | "declined" | "superseded" => return "blocked",
            _ => {}
        }
    }
    if let Some(disposition) = object
        .get("closure")
        .and_then(JsonValue::as_object)
        .and_then(|closure| object_string(closure, "disposition"))
    {
        return match disposition {
            "closed" => "completed",
            "deferred" => "deferred",
            "blocked" | "declined" | "superseded" => "blocked",
            "failed" | "killed" | "timed_out" => "failed",
            _ => "failed",
        };
    }
    match object_string(object, "status") {
        Some("needs_agent" | "needs_approval" | "payment_required") => "deferred",
        Some("sealed") => "completed",
        _ => "failed",
    }
}

fn normalize_pending_status(object: &mut JsonObject) {
    if object_string(object, "status") != Some("needs_agent") {
        return;
    }
    let Some(requests) = object.get("requests").and_then(JsonValue::as_array) else {
        return;
    };
    if !requests.is_empty()
        && requests.iter().all(|request| {
            request
                .as_object()
                .and_then(|request| object_string(request, "kind"))
                == Some("approval")
        })
    {
        object.insert(
            "status".to_owned(),
            JsonValue::String("needs_approval".to_owned()),
        );
    }
}

fn display_status(object: &JsonObject) -> &str {
    let status = object_string(object, "status").unwrap_or("unknown");
    if status == "needs_agent"
        && object
            .get("requests")
            .and_then(JsonValue::as_array)
            .is_some_and(|requests| {
                !requests.is_empty()
                    && requests.iter().all(|request| {
                        request
                            .as_object()
                            .and_then(|request| object_string(request, "kind"))
                            == Some("approval")
                    })
            })
    {
        "needs_approval"
    } else {
        status
    }
}

fn compact_request(request: &JsonValue, artifact_ref: JsonValue) -> JsonValue {
    let mut summary = JsonObject::new();
    if let Ok(bytes) = serde_json::to_vec(request) {
        summary.insert(
            "request_digest".to_owned(),
            JsonValue::String(sha256_prefixed(&bytes)),
        );
    }
    if let Some(id) = request_id(request) {
        summary.insert("id".to_owned(), JsonValue::String(id.to_owned()));
    }
    if let Some(kind) = request
        .as_object()
        .and_then(|request| object_string(request, "kind"))
    {
        summary.insert("kind".to_owned(), JsonValue::String(kind.to_owned()));
    }
    let invocation = request
        .as_object()
        .and_then(|request| request.get("invocation"))
        .and_then(JsonValue::as_object);
    if let Some(allowance) = invocation.and_then(|invocation| invocation.get("allowance")) {
        summary.insert("allowance".to_owned(), allowance.clone());
    }
    if let Some(output) = invocation
        .and_then(|invocation| invocation.get("envelope"))
        .and_then(JsonValue::as_object)
        .and_then(|envelope| envelope.get("output"))
    {
        if let Ok(bytes) = serde_json::to_vec(output) {
            summary.insert(
                "output_digest".to_owned(),
                JsonValue::String(sha256_prefixed(&bytes)),
            );
        }
        if let Some(schema) = output
            .as_object()
            .and_then(|output| object_string(output, "schema"))
        {
            summary.insert(
                "output_schema".to_owned(),
                JsonValue::String(schema.to_owned()),
            );
        }
    }
    summary.insert("artifact_ref".to_owned(), artifact_ref);
    JsonValue::Object(summary)
}

fn insert_next_command(object: &mut JsonObject) {
    if !matches!(
        object_string(object, "status"),
        Some("needs_agent" | "needs_approval")
    ) {
        object.insert("next".to_owned(), JsonValue::Null);
        return;
    }
    if let Some(run_id) = object_string(object, "run_id") {
        object.insert(
            "next".to_owned(),
            JsonValue::String(format!(
                "runx resume {} - --json",
                crate::resume::shell_token(run_id)
            )),
        );
    }
}

fn request_id(request: &JsonValue) -> Option<&str> {
    request
        .as_object()
        .and_then(|request| object_string(request, "id"))
}

fn result_schema(result: &JsonValue) -> &str {
    result
        .as_object()
        .and_then(|result| {
            object_string(result, "packet").or_else(|| object_string(result, "schema"))
        })
        .unwrap_or("runx.untyped_result.v1")
}

fn append_artifact_refs(object: &mut JsonObject, refs: Vec<JsonValue>) {
    let entry = object
        .entry("artifact_refs".to_owned())
        .or_insert_with(|| JsonValue::Array(Vec::new()));
    if let JsonValue::Array(existing) = entry {
        existing.extend(refs);
    }
}

fn compact_identity(object: &JsonObject) -> JsonObject {
    let mut minimal = JsonObject::new();
    for key in [
        "schema",
        "status",
        "outcome",
        "skill_name",
        "runner",
        "run_id",
        "receipt_id",
    ] {
        if let Some(value) = object.get(key) {
            minimal.insert(key.to_owned(), compact_scalar(value));
        }
    }
    if let Some(JsonValue::Object(closure)) = object.get("closure") {
        let mut compact = JsonObject::new();
        for key in ["disposition", "reason_code"] {
            if let Some(value) = closure.get(key) {
                compact.insert(key.to_owned(), compact_scalar(value));
            }
        }
        minimal.insert("closure".to_owned(), JsonValue::Object(compact));
    }
    minimal
}

fn compact_scalar(value: &JsonValue) -> JsonValue {
    match value {
        JsonValue::String(value) => JsonValue::String(truncate_chars(value)),
        value => value.clone(),
    }
}

fn truncate_chars(value: &str) -> String {
    if value.chars().count() <= MAX_COMPACT_SUMMARY_CHARS {
        return value.to_owned();
    }
    let mut truncated = value
        .chars()
        .take(MAX_COMPACT_SUMMARY_CHARS.saturating_sub(1))
        .collect::<String>();
    truncated.push('…');
    truncated
}

fn serialized_len(value: &JsonValue) -> Result<usize, String> {
    serde_json::to_vec(value)
        .map(|bytes| bytes.len())
        .map_err(|error| error.to_string())
}

fn write_text_with_exit(
    value: &JsonValue,
    exit_code: ExitCode,
    resume: ResumeHint<'_>,
) -> ExitCode {
    let mut stdout = io::stdout().lock();
    let result = write_skill_text(&mut stdout, value, resume);
    match result {
        Ok(()) => exit_code,
        Err(_) => ExitCode::from(1),
    }
}

fn write_skill_text(
    writer: &mut dyn Write,
    value: &JsonValue,
    resume: ResumeHint<'_>,
) -> io::Result<()> {
    let Some(object) = value.as_object() else {
        let text = serde_json::to_string(value).unwrap_or_else(|_| "null".to_owned());
        return writeln!(writer, "{text}");
    };
    writeln!(writer, "status: {}", display_status(object))?;
    writeln!(writer, "outcome: {}", semantic_outcome(object))?;
    if let Some(skill_name) = object_string(object, "skill_name") {
        writeln!(writer, "skill: {skill_name}")?;
    }
    if let Some(run_id) = object_string(object, "run_id") {
        writeln!(writer, "run_id: {run_id}")?;
    }
    if let Some(receipt_id) = object_string(object, "receipt_id") {
        writeln!(writer, "receipt_id: {receipt_id}")?;
    }
    if let Some(provenance) = object
        .get("registry_provenance")
        .and_then(JsonValue::as_object)
    {
        writeln!(writer, "registry:")?;
        write_registry_provenance(writer, provenance)?;
    }
    if let Some(reason) = stop_reason(object) {
        writeln!(writer, "reason: {}", truncate_chars(reason))?;
    }
    if let Some(result) = object
        .get("result")
        .filter(|result| !matches!(result, JsonValue::Null))
    {
        let text = serde_json::to_string(result).unwrap_or_else(|_| "null".to_owned());
        writeln!(writer, "result: {}", truncate_chars(&text))?;
    }
    if let Some(requests) = object.get("requests").and_then(JsonValue::as_array) {
        write_pending_requests(writer, object, requests, resume)?;
    }
    Ok(())
}

fn write_pending_requests(
    writer: &mut dyn Write,
    object: &JsonObject,
    requests: &[JsonValue],
    resume: ResumeHint<'_>,
) -> io::Result<()> {
    writeln!(writer, "pending_requests: {}", requests.len())?;
    for request in requests {
        if let Some(request) = request.as_object() {
            let id = object_string(request, "id").unwrap_or("<unknown>");
            let kind = object_string(request, "kind").unwrap_or("<unknown>");
            writeln!(writer, "- {kind}: {id}")?;
        }
    }
    if let Some(template) = answers_template(requests) {
        writeln!(writer, "answers_template:")?;
        write_indented_json(writer, &template)?;
    }
    if let Some(run_id) = object_string(object, "run_id") {
        let command =
            crate::resume::render_skill_resume_command(crate::resume::SkillResumeCommand {
                run_id,
                receipt_dir: resume.receipt_dir,
                answers_path: resume.answers_path,
            });
        writeln!(writer, "next: resolve the request, then rerun: {command}")?;
    }
    Ok(())
}

fn answers_template(requests: &[JsonValue]) -> Option<JsonValue> {
    let mut answers = JsonObject::new();
    let mut approvals = JsonObject::new();
    for request in requests {
        let Some(request) = request.as_object() else {
            continue;
        };
        let Some(id) = object_string(request, "id") else {
            continue;
        };
        if object_string(request, "kind") == Some("approval") {
            approvals.insert(id.to_owned(), JsonValue::Bool(false));
        } else {
            answers.insert(id.to_owned(), JsonValue::Object(JsonObject::new()));
        }
    }
    if answers.is_empty() && approvals.is_empty() {
        return None;
    }
    let mut template = JsonObject::new();
    if !answers.is_empty() {
        template.insert("answers".to_owned(), JsonValue::Object(answers));
    }
    if !approvals.is_empty() {
        template.insert("approvals".to_owned(), JsonValue::Object(approvals));
    }
    Some(JsonValue::Object(template))
}

fn write_indented_json(writer: &mut dyn Write, value: &JsonValue) -> io::Result<()> {
    let json = serde_json::to_string_pretty(value).unwrap_or_else(|_| "{}".to_owned());
    for line in json.lines() {
        writeln!(writer, "  {line}")?;
    }
    Ok(())
}

fn write_registry_provenance(writer: &mut dyn Write, object: &JsonObject) -> io::Result<()> {
    for key in [
        "skill_id",
        "version",
        "digest",
        "profile_digest",
        "registry_source",
        "registry_source_fingerprint",
        "trust_tier",
        "registry_key_id",
        "trust_state",
    ] {
        if let Some(value) = object_string(object, key) {
            writeln!(writer, "  {key}: {value}")?;
        }
    }
    Ok(())
}

/// Why the run stopped short, as the runtime reported it: a guard's or an
/// authority's refusal, or a failure's message.
fn stop_reason(object: &JsonObject) -> Option<&str> {
    ["refusal", "error"].into_iter().find_map(|key| {
        object
            .get(key)
            .and_then(JsonValue::as_object)
            .and_then(|stop| object_string(stop, "message"))
    })
}

fn object_string<'a>(object: &'a JsonObject, key: &str) -> Option<&'a str> {
    object.get(key).and_then(JsonValue::as_str)
}

#[cfg(test)]
mod tests {
    #[cfg(test)]
    use std::fs;
    use std::path::Path;
    use std::process::ExitCode;

    use runx_contracts::{JsonObject, JsonValue};

    use super::{
        ResumeHint, closure_disposition_exit_code, persist_json_artifact, read_json_artifact,
        run_result_exit_code, serialize_json_output, write_skill_text,
    };

    #[cfg(unix)]
    #[test]
    fn project_artifacts_are_private_and_existing_artifacts_are_restricted()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::PermissionsExt;

        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let workspace = std::env::temp_dir().join(format!(
            "runx-artifact-permission-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::create_dir(&workspace)?;
        let runx_dir = workspace.join(".runx");
        let reference = persist_json_artifact(
            &runx_dir.join("artifacts"),
            "run_private",
            "results",
            "result",
            &JsonValue::String("private message preview".to_owned()),
        )?;
        let path = reference
            .as_object()
            .and_then(|value| value.get("path"))
            .and_then(JsonValue::as_str)
            .ok_or("artifact has no path")?;
        let path = Path::new(path);
        for directory in [
            runx_dir.join("artifacts"),
            runx_dir.join("artifacts/skill-runs"),
            runx_dir.join("artifacts/skill-runs/run_private"),
            runx_dir.join("artifacts/skill-runs/run_private/results"),
        ] {
            assert_eq!(fs::metadata(directory)?.permissions().mode() & 0o777, 0o700);
        }
        assert_eq!(fs::metadata(path)?.permissions().mode() & 0o777, 0o600);

        fs::set_permissions(
            runx_dir.join("artifacts"),
            fs::Permissions::from_mode(0o755),
        )?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o644))?;
        persist_json_artifact(
            &runx_dir.join("artifacts"),
            "run_private",
            "results",
            "result",
            &JsonValue::String("private message preview".to_owned()),
        )?;
        assert_eq!(
            fs::metadata(runx_dir.join("artifacts"))?
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(fs::metadata(path)?.permissions().mode() & 0o777, 0o600);
        fs::remove_dir_all(workspace)?;
        Ok(())
    }

    #[test]
    fn exact_intent_artifact_refuses_tampering_and_path_escape()
    -> Result<(), Box<dyn std::error::Error>> {
        let workspace = std::env::temp_dir().join(format!(
            "runx-exact-intent-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos()
        ));
        fs::create_dir(&workspace)?;
        let artifacts = workspace.join(".runx").join("artifacts");
        let exact = JsonValue::String("exact private update".to_owned());
        let reference =
            persist_json_artifact(&artifacts, "assistant", "assistant", "intent", &exact)?;
        assert_eq!(read_json_artifact(&artifacts, &reference)?, exact);
        let path = reference
            .as_object()
            .and_then(|item| item.get("path"))
            .and_then(JsonValue::as_str)
            .ok_or("artifact has no path")?;
        fs::write(path, b"\"changed\"")?;
        assert!(read_json_artifact(&artifacts, &reference).is_err());
        let mut escaped = reference.clone();
        let JsonValue::Object(ref mut fields) = escaped else {
            unreachable!()
        };
        fields.insert(
            "path".to_owned(),
            JsonValue::String(
                workspace
                    .join("outside.json")
                    .to_string_lossy()
                    .into_owned(),
            ),
        );
        fs::write(workspace.join("outside.json"), b"\"exact private update\"")?;
        assert!(read_json_artifact(&artifacts, &escaped).is_err());
        fs::remove_dir_all(workspace)?;
        Ok(())
    }

    #[test]
    fn json_output_is_compact_and_omits_diagnostic_context()
    -> Result<(), Box<dyn std::error::Error>> {
        let result = JsonValue::Object(JsonObject::from([
            ("message".to_owned(), JsonValue::String("ready".to_owned())),
            (
                "status".to_owned(),
                JsonValue::String("complete".to_owned()),
            ),
        ]));
        let value = JsonValue::Object(JsonObject::from([
            ("status".to_owned(), JsonValue::String("failure".to_owned())),
            ("result".to_owned(), result.clone()),
            (
                "context".to_owned(),
                JsonValue::Object(JsonObject::from([(
                    "step_outputs".to_owned(),
                    JsonValue::Object(JsonObject::from([
                        ("final".to_owned(), result),
                        (
                            "prior".to_owned(),
                            JsonValue::Object(JsonObject::from([(
                                "evidence".to_owned(),
                                JsonValue::String("kept".to_owned()),
                            )])),
                        ),
                        (
                            "matching_subset".to_owned(),
                            JsonValue::Object(JsonObject::from([(
                                "message".to_owned(),
                                JsonValue::String("ready".to_owned()),
                            )])),
                        ),
                    ])),
                )])),
            ),
        ]));

        let workspace = std::env::temp_dir().join(format!(
            "runx-compact-output-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos()
        ));
        let artifacts = workspace.join(".runx").join("artifacts");
        let serialized = serialize_json_output(&value, &artifacts, false)?;
        let projected: serde_json::Value = serde_json::from_str(&serialized)?;

        assert!(projected.get("context").is_none());
        assert_eq!(projected["outcome"], "failed");
        assert_eq!(projected["result"]["message"], "ready");
        assert_eq!(projected["next"], serde_json::Value::Null);

        let diagnostic = serialize_json_output(&value, &artifacts, true)?;
        let diagnostic: serde_json::Value = serde_json::from_str(&diagnostic)?;
        assert!(diagnostic.get("context").is_some());
        fs::remove_dir_all(workspace)?;
        Ok(())
    }

    #[test]
    fn terminal_dispositions_have_exhaustive_exit_semantics() {
        let docs = include_str!("../../../../docs/cli-exit-codes.md");
        for (disposition, label, expected, numeric) in [
            (
                runx_contracts::ClosureDisposition::Closed,
                "closed",
                ExitCode::SUCCESS,
                0,
            ),
            (
                runx_contracts::ClosureDisposition::Deferred,
                "deferred",
                ExitCode::from(2),
                2,
            ),
            (
                runx_contracts::ClosureDisposition::Superseded,
                "superseded",
                ExitCode::from(1),
                1,
            ),
            (
                runx_contracts::ClosureDisposition::Declined,
                "declined",
                ExitCode::from(1),
                1,
            ),
            (
                runx_contracts::ClosureDisposition::Blocked,
                "blocked",
                ExitCode::from(1),
                1,
            ),
            (
                runx_contracts::ClosureDisposition::Failed,
                "failed",
                ExitCode::from(1),
                1,
            ),
            (
                runx_contracts::ClosureDisposition::Killed,
                "killed",
                ExitCode::from(1),
                1,
            ),
            (
                runx_contracts::ClosureDisposition::TimedOut,
                "timed_out",
                ExitCode::from(1),
                1,
            ),
        ] {
            assert_eq!(
                closure_disposition_exit_code(disposition),
                expected,
                "{label} typed exit code"
            );
            assert!(
                docs.contains(&format!("| `{label}` | {numeric} |")),
                "CLI exit-code documentation omitted {label}"
            );
        }

        let malformed = runx_runtime::execution::orchestrator::RunResult {
            status: runx_runtime::execution::orchestrator::RunStatus::Sealed,
            disposition: None,
            output: JsonValue::Object(JsonObject::new()),
            receipt_refs: Vec::new(),
            child_receipt_refs: Vec::new(),
            pending_requests: Vec::new(),
            diagnostics: Vec::new(),
        };
        assert_eq!(run_result_exit_code(&malformed), ExitCode::from(1));
    }

    #[test]
    fn text_output_shows_the_result_and_why_a_run_stopped() {
        let mut value = base_result();
        value.insert(
            "result".to_owned(),
            JsonValue::Object(JsonObject::from([(
                "forecast".to_owned(),
                JsonValue::String("wet morning".to_owned()),
            )])),
        );
        value.insert(
            "refusal".to_owned(),
            JsonValue::Object(JsonObject::from([(
                "message".to_owned(),
                JsonValue::String("guard '$steps.decide.approved' is unresolved".to_owned()),
            )])),
        );

        let output = render(value);

        assert!(
            output.contains(r#"result: {"forecast":"wet morning"}"#),
            "{output}"
        );
        assert!(
            output.contains("reason: guard '$steps.decide.approved' is unresolved"),
            "{output}"
        );
    }

    #[test]
    fn text_output_includes_resume_metadata_for_pending_requests() {
        let mut value = base_result();
        value.insert(
            "status".to_owned(),
            JsonValue::String("needs_agent".to_owned()),
        );
        value.insert(
            "requests".to_owned(),
            JsonValue::Array(vec![JsonValue::Object(JsonObject::from([
                ("id".to_owned(), JsonValue::String("request_1".to_owned())),
                ("kind".to_owned(), JsonValue::String("agent_act".to_owned())),
            ]))]),
        );

        let output = render_with_resume(
            value,
            ResumeHint {
                receipt_dir: Some(Path::new("custom receipts")),
                answers_path: Some(Path::new("operator answers.json")),
            },
        );

        assert!(output.contains(
            "runx resume run_weather 'operator answers.json' --receipt-dir 'custom receipts'"
        ));
        assert!(output.contains("answers_template:"));
        assert!(output.contains(r#""request_1": {}"#));
    }

    #[test]
    fn text_output_emits_fail_closed_approval_template() {
        let mut value = base_result();
        value.insert(
            "status".to_owned(),
            JsonValue::String("needs_agent".to_owned()),
        );
        value.insert(
            "requests".to_owned(),
            JsonValue::Array(vec![JsonValue::Object(JsonObject::from([
                (
                    "id".to_owned(),
                    JsonValue::String("sourcey.discovery.approval".to_owned()),
                ),
                ("kind".to_owned(), JsonValue::String("approval".to_owned())),
            ]))]),
        );

        let output = render(value);

        assert!(output.contains(r#""approvals""#));
        assert!(output.contains(r#""sourcey.discovery.approval": false"#));
        assert!(!output.contains(r#""sourcey.discovery.approval": {}"#));
    }

    fn base_result() -> JsonObject {
        JsonObject::from([
            ("status".to_owned(), JsonValue::String("sealed".to_owned())),
            (
                "skill_name".to_owned(),
                JsonValue::String("weather-forecast".to_owned()),
            ),
            (
                "run_id".to_owned(),
                JsonValue::String("run_weather".to_owned()),
            ),
            (
                "receipt_id".to_owned(),
                JsonValue::String("sha256:abc".to_owned()),
            ),
        ])
    }

    fn render(value: JsonObject) -> String {
        render_with_resume(
            value,
            ResumeHint {
                receipt_dir: None,
                answers_path: None,
            },
        )
    }

    fn render_with_resume(value: JsonObject, resume: ResumeHint<'_>) -> String {
        let mut output = Vec::new();
        let write_result = write_skill_text(&mut output, &JsonValue::Object(value), resume);
        assert!(write_result.is_ok(), "text output renders");
        let rendered = String::from_utf8(output);
        assert!(rendered.is_ok(), "text output is utf8");
        rendered.unwrap_or_default()
    }
}
