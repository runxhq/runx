use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use runx_contracts::{EnvironmentRequirements, ExecutionBoundaryKind, JsonObject, JsonValue};
use runx_parser::ProcessTool;
use runx_runtime::process_invocation::prepare_process_invocation;
use runx_runtime::{RUNX_CWD_ENV, RuntimeError};

#[test]
fn trusted_host_process_receives_exact_argv_cwd_and_declared_environment()
-> Result<(), Box<dyn std::error::Error>> {
    let workspace = tempfile::tempdir()?;
    let skill = workspace.path().join("skill");
    fs::create_dir_all(&skill)?;
    let mut tool = tool(
        Some("."),
        vec![
            "{{message}}".to_owned(),
            "{{env.REGION}}".to_owned(),
            "--at={{env.REGION}}/{{message}}".to_owned(),
        ],
    );
    tool.environment = EnvironmentRequirements {
        required: vec!["REGION".to_owned()],
        optional: Vec::new(),
    };
    let inputs = JsonObject::from([("message".to_owned(), JsonValue::String("hello".to_owned()))]);
    let env = environment(
        workspace.path(),
        [
            ("REGION", "ap-southeast-2"),
            ("UNDECLARED_SECRET", "blocked"),
            ("HTTPS_PROXY", "https://user:secret@proxy.example"),
        ],
    );

    let process = prepare_process_invocation(&tool, &skill, &inputs, &env)?;

    assert_eq!(process.command, "/usr/bin/printf");
    assert_eq!(
        process.args,
        ["hello", "ap-southeast-2", "--at=ap-southeast-2/hello"]
    );
    assert_eq!(process.cwd, fs::canonicalize(&skill)?);
    assert_eq!(
        process.env.get("REGION").map(String::as_str),
        Some("ap-southeast-2")
    );
    assert!(!process.env.contains_key("UNDECLARED_SECRET"));
    assert!(!process.env.contains_key("HTTPS_PROXY"));
    assert_eq!(process.boundary, ExecutionBoundaryKind::TrustedHostProcess);
    Ok(())
}

#[test]
fn a_process_argument_is_filled_once_from_declared_values() -> Result<(), Box<dyn std::error::Error>>
{
    let workspace = tempfile::tempdir()?;
    let skill = workspace.path().join("skill");
    fs::create_dir_all(&skill)?;
    let env = environment(
        workspace.path(),
        [("REGION", "ap-southeast-2"), ("UNDECLARED_SECRET", "blocked")],
    );
    let declared = EnvironmentRequirements {
        required: vec!["REGION".to_owned()],
        optional: Vec::new(),
    };
    let prepare = |arg: &str, message: &str| {
        let mut tool = tool(Some("."), vec![arg.to_owned()]);
        tool.environment = declared.clone();
        let inputs =
            JsonObject::from([("message".to_owned(), JsonValue::String(message.to_owned()))]);
        prepare_process_invocation(&tool, &skill, &inputs, &env).map(|process| process.args.clone())
    };

    assert_eq!(
        prepare("{{message}}", "{{env.REGION}}")?,
        ["{{env.REGION}}"],
        "an input's value is never expanded again"
    );
    for refused in [
        "{{env.UNDECLARED_SECRET}}",
        "{{missing}}",
        "{{ message }}",
        "{{message",
    ] {
        let error = match prepare(refused, "hello") {
            Err(error) => error.to_string(),
            Ok(args) => return Err(format!("{refused} filled as {args:?}").into()),
        };
        assert!(
            error.contains("neither a provided input nor a declared environment value"),
            "{refused}: {error}"
        );
    }
    Ok(())
}

#[test]
fn trusted_host_process_honors_explicit_relative_cwd_without_claiming_confinement()
-> Result<(), Box<dyn std::error::Error>> {
    let workspace = tempfile::tempdir()?;
    let skill = workspace.path().join("skill");
    let sibling = workspace.path().join("sibling");
    fs::create_dir_all(&skill)?;
    fs::create_dir_all(&sibling)?;
    let tool = tool(Some("../sibling"), Vec::new());
    let env = environment(workspace.path(), []);

    let process = prepare_process_invocation(&tool, &skill, &JsonObject::new(), &env)?;

    assert_eq!(process.cwd, fs::canonicalize(sibling)?);
    Ok(())
}

#[test]
fn large_inputs_use_owned_files_and_cleanup_with_the_invocation()
-> Result<(), Box<dyn std::error::Error>> {
    let workspace = tempfile::tempdir()?;
    let skill = workspace.path().join("skill");
    fs::create_dir_all(&skill)?;
    let tool = tool(None, Vec::new());
    let inputs = JsonObject::from([("large".to_owned(), JsonValue::String("x".repeat(80 * 1024)))]);
    let env = environment(workspace.path(), []);

    let process = prepare_process_invocation(&tool, &skill, &inputs, &env)?;
    let cleanup_path = process
        .cleanup_paths
        .first()
        .cloned()
        .ok_or("missing owned input directory")?;

    assert!(cleanup_path.exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        assert_eq!(
            fs::metadata(&cleanup_path)?.permissions().mode() & 0o777,
            0o700
        );
    }
    assert!(process.env.contains_key("RUNX_INPUTS_PATH"));
    assert!(!process.env.contains_key("RUNX_INPUTS_JSON"));
    assert!(process.env.contains_key("RUNX_INPUT_LARGE_PATH"));
    assert!(!process.env.contains_key("RUNX_INPUT_LARGE"));

    drop(process);
    assert!(!cleanup_path.exists());
    Ok(())
}

#[test]
fn colliding_author_input_environment_names_fail_before_spawn()
-> Result<(), Box<dyn std::error::Error>> {
    let workspace = tempfile::tempdir()?;
    let skill = workspace.path().join("skill");
    fs::create_dir_all(&skill)?;
    let inputs = JsonObject::from([
        (
            "thread-title".to_owned(),
            JsonValue::String("one".to_owned()),
        ),
        (
            "thread_title".to_owned(),
            JsonValue::String("two".to_owned()),
        ),
    ]);

    let Err(error) = prepare_process_invocation(
        &tool(None, Vec::new()),
        &skill,
        &inputs,
        &environment(workspace.path(), []),
    ) else {
        return Err("colliding names must fail".into());
    };

    assert!(matches!(
        error,
        RuntimeError::InvalidProcessInvocation { message }
            if message.contains("collide on environment variable RUNX_INPUT_THREAD_TITLE")
    ));
    Ok(())
}

fn tool(cwd: Option<&str>, args: Vec<String>) -> ProcessTool {
    ProcessTool {
        command: "/usr/bin/printf".to_owned(),
        args,
        cwd: cwd.map(str::to_owned),
        timeout_seconds: Some(5),
        input_mode: None,
        environment: EnvironmentRequirements::default(),
    }
}

fn environment<const N: usize>(
    workspace: &Path,
    values: [(&str, &str); N],
) -> BTreeMap<String, String> {
    let mut environment = values
        .into_iter()
        .map(|(name, value)| (name.to_owned(), value.to_owned()))
        .collect::<BTreeMap<_, _>>();
    environment.insert(
        RUNX_CWD_ENV.to_owned(),
        workspace.to_string_lossy().into_owned(),
    );
    environment
}
