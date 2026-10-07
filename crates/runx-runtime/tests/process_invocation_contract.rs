#![cfg(feature = "cli-tool")]

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use runx_contracts::{EnvironmentRequirements, JsonObject, JsonValue};
use runx_parser::{SkillSource, SourceKind};
use runx_runtime::process_invocation::prepare_process_invocation;
use runx_runtime::{RUNX_CWD_ENV, RuntimeError};

#[test]
fn trusted_host_process_receives_exact_argv_cwd_and_declared_environment()
-> Result<(), Box<dyn std::error::Error>> {
    let workspace = tempfile::tempdir()?;
    let skill = workspace.path().join("skill");
    fs::create_dir_all(&skill)?;
    let source = source(
        Some("."),
        vec![
            "{{message}}".to_owned(),
            "{{env.REGION}}".to_owned(),
            "{{env.UNDECLARED_SECRET}}".to_owned(),
        ],
    );
    let requirements = EnvironmentRequirements {
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

    let process = prepare_process_invocation(&source, &requirements, &skill, &inputs, &env)?;

    assert_eq!(process.command, "/usr/bin/printf");
    assert_eq!(
        process.args,
        ["hello", "ap-southeast-2", "{{env.UNDECLARED_SECRET}}"]
    );
    assert_eq!(process.cwd, fs::canonicalize(&skill)?);
    assert_eq!(
        process.env.get("REGION").map(String::as_str),
        Some("ap-southeast-2")
    );
    assert!(!process.env.contains_key("UNDECLARED_SECRET"));
    assert!(!process.env.contains_key("HTTPS_PROXY"));
    assert_eq!(
        process
            .metadata
            .get("execution_boundary")
            .and_then(JsonValue::as_object)
            .and_then(|boundary| boundary.get("kind")),
        Some(&JsonValue::String("trusted_host_process".to_owned()))
    );
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
    let source = source(Some("../sibling"), Vec::new());
    let env = environment(workspace.path(), []);

    let process = prepare_process_invocation(
        &source,
        &EnvironmentRequirements::default(),
        &skill,
        &JsonObject::new(),
        &env,
    )?;

    assert_eq!(process.cwd, fs::canonicalize(sibling)?);
    Ok(())
}

#[test]
fn large_inputs_use_owned_files_and_cleanup_with_the_invocation()
-> Result<(), Box<dyn std::error::Error>> {
    let workspace = tempfile::tempdir()?;
    let skill = workspace.path().join("skill");
    fs::create_dir_all(&skill)?;
    let source = source(None, Vec::new());
    let inputs = JsonObject::from([("large".to_owned(), JsonValue::String("x".repeat(80 * 1024)))]);
    let env = environment(workspace.path(), []);

    let process = prepare_process_invocation(
        &source,
        &EnvironmentRequirements::default(),
        &skill,
        &inputs,
        &env,
    )?;
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
        &source(None, Vec::new()),
        &EnvironmentRequirements::default(),
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

fn source(cwd: Option<&str>, args: Vec<String>) -> SkillSource {
    SkillSource {
        source_type: SourceKind::CliTool,
        command: Some("/usr/bin/printf".to_owned()),
        args,
        cwd: cwd.map(str::to_owned),
        timeout_seconds: Some(5),
        input_mode: None,
        environment: EnvironmentRequirements::default(),
        module: None,
        javascript_export: None,
        pages: None,
        agent: None,
        task: None,
        outputs: None,
        graph: None,
        act: None,
        raw: JsonObject::new(),
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
