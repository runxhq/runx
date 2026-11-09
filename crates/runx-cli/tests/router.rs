use runx_cli::command_spec::COMMAND_SPECS;
use runx_cli::config::{ConfigAction, ConfigPlan};
use runx_cli::export::{ExportPlan, Target};
use runx_cli::login::LoginPlan;
use runx_cli::mcp::McpPlan;
use runx_cli::registry::{RegistryAction, RegistryPlan};
use runx_cli::resume::ResumePlan;
use runx_cli::router::{
    DoctorMode, DoctorPlan, FilterMode, HistoryPlan, ListKind, ListPlan, RouterAction, TestPlan,
    ToolAction, ToolPlan, command_help_text, help_text, route_args,
};
use runx_cli::skill::{MethodRef, SkillAction, SkillPlan};
use runx_runtime::HostedApiCredentialPurpose;
use std::fs;
use std::path::{Path, PathBuf};

fn plan(args: &[&str]) -> RouterAction {
    route_args(args.iter().map(std::ffi::OsString::from).collect())
}

fn help(command: &str) -> String {
    command_help_text(command).unwrap_or_default()
}

#[test]
fn top_level_help_and_version_are_native() {
    assert_eq!(plan(&[]), RouterAction::PrintHelp);
    assert_eq!(plan(&["--help"]), RouterAction::PrintHelp);
    assert_eq!(plan(&["--help", "--json"]), RouterAction::PrintHelpJson);
    assert_eq!(plan(&["-j", "-h"]), RouterAction::PrintHelpJson);
    assert_eq!(plan(&["--version"]), RouterAction::PrintVersion);
    assert_eq!(
        plan(&["export", "--help"]),
        RouterAction::PrintCommandHelp("export")
    );

    let help = help_text();
    assert_help_line(
        &help,
        "runx verify [receipt-id] [--receipt-dir dir] [--receipt <path|->] [--trust home|configured] [--notary <path|-> --notary-key trusted.pem] [-j|--json]",
    );
    assert_help_line(
        &help,
        "runx skill <skill-ref|publisher/name@version|skill-dir|SKILL.md>[#method] [-i name=value...] [--inputs file|-] [--act json] [--run-authority json] [--max-spend usd] [--context json] [--run-id id] [-j] [--diagnostics] [--drive [--drive-rounds n]] [--registry url] [--digest sha256] [--package-digest sha256 --execution-closure-digest sha256] [--receipt-dir dir]",
    );
    assert_help_line(
        &help,
        "runx skill inspect <skill-ref|publisher/name@version|skill-dir|SKILL.md>[#method] [--inputs file|-] [--run-authority json] [-j] [--registry url] [--digest sha256]",
    );
    assert_help_line(
        &help,
        "runx resume <run-id> <answers.json|-> [--receipt-dir dir] [--package-digest sha256] [--execution-closure-digest sha256] [--drive [--drive-rounds n]] [--diagnostics] [-j|--json]",
    );
    assert_help_line(
        &help,
        "runx test <package-dir|SKILL.md|tests/<name>.yaml...> [--receipt-dir dir] [-j|--json]",
    );
    assert_help_line(&help, "runx doctor [path|authority|registry] [--json]");
    assert_help_line(
        &help,
        "runx export <claude|codex> [skill-ref...] [--project] [--json]",
    );
    assert_help_line(
        &help,
        "runx login [--provider name] [--for default|publish] [--token-stdin] [--api-base-url url] [--allow-local-api] [-j|--json]",
    );
    assert_help_line(
        &help,
        "runx connector list|inspect|use|revoke|import-mcp ... [-j|--json]",
    );
    assert_help_line(
        &help,
        "runx registry search|install|package|publish ... [-j|--json]",
    );
    assert!(
        !help.contains("runx harness"),
        "native help must not advertise harness, which runx test replaced"
    );
    assert!(
        !help.contains("runx url-add"),
        "native help must not advertise the internal URL index command"
    );
}

#[test]
fn nested_skill_history_verify_and_notarize_help_are_native() {
    assert_eq!(
        plan(&["skill", "--help"]),
        RouterAction::PrintCommandHelp("skill")
    );
    assert_eq!(
        plan(&["skill", "-h"]),
        RouterAction::PrintCommandHelp("skill")
    );
    assert_eq!(
        plan(&["skill", "SKILL.md", "--help"]),
        RouterAction::PrintCommandHelp("skill")
    );
    assert_eq!(
        plan(&["history", "--help"]),
        RouterAction::PrintCommandHelp("history")
    );
    assert_eq!(
        plan(&["history", "-h"]),
        RouterAction::PrintCommandHelp("history")
    );
    assert_eq!(
        plan(&["history", "sourcey", "--help"]),
        RouterAction::PrintCommandHelp("history")
    );
    assert_eq!(
        plan(&["verify", "--help"]),
        RouterAction::PrintCommandHelp("verify")
    );
    assert_eq!(
        plan(&["verify", "-h"]),
        RouterAction::PrintCommandHelp("verify")
    );
    assert_eq!(
        plan(&["verify", "receipt-123", "--help"]),
        RouterAction::PrintCommandHelp("verify")
    );
    assert_eq!(
        plan(&["notarize", "--help"]),
        RouterAction::PrintCommandHelp("notarize")
    );
    assert_eq!(
        plan(&["notarize", "-h"]),
        RouterAction::PrintCommandHelp("notarize")
    );
    assert_eq!(
        plan(&["test", "--help"]),
        RouterAction::PrintCommandHelp("test")
    );
    assert_eq!(
        plan(&["test", "-h"]),
        RouterAction::PrintCommandHelp("test")
    );

    assert_help_line(
        &help("skill"),
        "runx skill <skill-ref|publisher/name@version|skill-dir|SKILL.md>[#method] [-i name=value...] [--inputs file|-] [--act json] [--run-authority json] [--max-spend usd] [--context json] [--run-id id] [-j] [--diagnostics] [--drive [--drive-rounds n]] [--registry url] [--digest sha256] [--package-digest sha256 --execution-closure-digest sha256] [--receipt-dir dir]",
    );
    assert_help_line(
        &help("skill"),
        "runx skill inspect <skill-ref|publisher/name@version|skill-dir|SKILL.md>[#method] [--inputs file|-] [--run-authority json] [-j] [--registry url] [--digest sha256]",
    );
    assert_help_line(
        &help("skill"),
        "--act json               State the act the run is invoked with, as a JSON act with its purpose",
    );
    assert_help_line(
        &help("history"),
        "runx history [query] [--skill s] [--state s] [--since iso] [--until iso] [--limit n] [--json]",
    );
    assert_help_line(
        &help("history"),
        "runx history <run-id> [--after cursor] [--json]",
    );
    assert!(!help("history").contains("<receipt-id>"));
    assert_help_line(
        &help("verify"),
        "runx verify [receipt-id] [--receipt-dir dir] [--receipt <path|->] [--trust home|configured] [--notary <path|-> --notary-key trusted.pem] [-j|--json]",
    );
    assert!(help("verify").contains("--trust home|configured"));
    assert!(help("history").contains("--limit n"));
    assert_help_line(
        &help("notarize"),
        "runx notarize <receipt.json> [--api-base-url url] [--allow-local-api] [-j|--json]",
    );
    assert!(help("notarize").contains("anyone can read it"));
    assert_help_line(
        &help("test"),
        "runx test <package-dir|SKILL.md|tests/<name>.yaml...> [--receipt-dir dir] [-j|--json]",
    );
    assert!(help("test").contains("A package runs each of its tests/*.yaml, in name order."));
}

#[test]
fn json_flags_make_argument_failures_machine_readable() {
    for args in [
        &["config", "set", "--json"][..],
        &["tool", "search", "--json"][..],
        &["export", "nope", "--json"][..],
        &["doctor", "registry", "extra", "-j"][..],
    ] {
        let action = plan(args);
        assert!(
            matches!(action, RouterAction::JsonError(_)),
            "expected JSON error for {args:?}, got {action:?}"
        );
        if let RouterAction::JsonError(error) = action {
            assert_eq!(error.code, "invalid_args", "{args:?}");
            assert_eq!(error.exit_code, 64, "{args:?}");
            assert!(!error.message.is_empty(), "{args:?}");
        }
    }
}

#[cfg(unix)]
#[test]
fn filesystem_paths_do_not_require_utf8() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let path = OsString::from_vec(b"skill-\xff".to_vec());
    let path_buf = PathBuf::from(path.clone());

    assert_eq!(
        route_args(vec!["mcp".into(), path.clone()]),
        RouterAction::RunMcp(McpPlan {
            run: None,
            authority: None,
            refs: vec![MethodRef {
                skill_path: path_buf.clone(),
                method: None,
            }],
            receipt_dir: None,
            http_listen: None,
            http_allow_non_loopback: false,
        })
    );
    assert_eq!(
        route_args(vec!["resume".into(), "gx_test".into(), path.clone(),]),
        RouterAction::RunResume(ResumePlan {
            run_id: "gx_test".to_owned(),
            answers_path: path_buf.clone(),
            receipt_dir: None,
            expected_package_digest: None,
            expected_execution_closure_digest: None,
            json: false,
            diagnostics: false,
            managed_agent: Default::default(),
        })
    );
    let action = route_args(vec!["skill".into(), path.clone()]);
    assert!(matches!(action, RouterAction::RunSkill(_)));
    if let RouterAction::RunSkill(skill) = action {
        assert_eq!(skill.skill_path, path_buf);
    }
    assert_eq!(
        route_args(vec!["tool".into(), "inspect".into(), path]),
        RouterAction::Error("runx tool search and inspect arguments must be UTF-8".to_owned())
    );
}

#[test]
fn documented_command_help_is_native() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        plan(&["list", "--help"]),
        RouterAction::PrintCommandHelp("list")
    );
    assert_eq!(
        plan(&["login", "--help"]),
        RouterAction::PrintCommandHelp("login")
    );
    assert_eq!(
        plan(&["connector", "--help"]),
        RouterAction::PrintCommandHelp("connector")
    );
    assert_eq!(
        plan(&["registry", "--help"]),
        RouterAction::PrintCommandHelp("registry")
    );
    assert_eq!(
        plan(&["registry"]),
        RouterAction::PrintCommandUsageError("registry")
    );

    assert_help_line(
        &help("registry"),
        "runx registry install <publisher>/<name>[@version] [--registry url] [--digest sha256] [-j|--json]",
    );
    assert_help_line(
        &help("list"),
        "runx list [tools|skills|packets] [--only valid|invalid] [-j|--json]",
    );
    assert_help_line(
        &help("login"),
        "runx login [--provider name] [--for default|publish] [--token-stdin] [--api-base-url url] [--allow-local-api] [-j|--json]",
    );
    let resume_help = command_help_text("resume").unwrap_or_default();
    let example = resume_help
        .lines()
        .find(|line| line.trim().starts_with("{\"answers\":"))
        .ok_or("resume help must include a bound answer document")?;
    let example: runx_contracts::ResumeDocument = serde_json::from_str(example.trim())?;
    assert!(
        example
            .answers
            .values()
            .all(|answer| !answer.request_digest.as_str().is_empty())
    );
    assert!(resume_help.contains("Agents must never author it"));
    assert_help_line(
        &help("registry"),
        "runx registry search <query> [--registry url] [--limit n] [-j|--json]",
    );
    assert!(!help("registry").contains("--installation-id"));
    for retired in [
        "--trust-tier",
        "--upsert",
        "--profile",
        "--version",
        "url|path",
    ] {
        assert!(!help("registry").contains(retired), "{retired}");
    }
    Ok(())
}

#[test]
fn every_documented_command_routes_to_registry_help() {
    for spec in COMMAND_SPECS {
        assert_eq!(
            plan(&[spec.name, "--help"]),
            RouterAction::PrintCommandHelp(spec.name),
            "--help routing drifted for {}",
            spec.name
        );
        assert_eq!(
            plan(&[spec.name, "-h"]),
            RouterAction::PrintCommandHelp(spec.name),
            "-h routing drifted for {}",
            spec.name
        );
        let help = command_help_text(spec.name).unwrap_or_default();
        assert!(
            help.starts_with(&format!("runx {}\n", spec.name)),
            "detailed help missing for {}",
            spec.name
        );
    }
}

#[test]
fn routes_login_to_native_plan() {
    assert_eq!(
        plan(&[
            "login",
            "--provider",
            "github",
            "--for",
            "publish",
            "--token-stdin",
            "--api-base-url",
            "https://runx.test",
            "--json",
        ]),
        RouterAction::RunLogin(LoginPlan {
            provider: Some("github".to_owned()),
            purpose: Some(HostedApiCredentialPurpose::Publish),
            api_base_url: Some("https://runx.test".to_owned()),
            token_stdin: true,
            allow_local_api: false,
            json: true,
        })
    );
    assert_eq!(
        plan(&["login", "--unknown"]),
        RouterAction::Error("unknown login flag --unknown".to_owned())
    );
}

#[test]
fn routes_doctor_registry_to_native_plan() {
    assert_eq!(
        plan(&["doctor", "registry", "--json"]),
        RouterAction::RunDoctor(DoctorPlan {
            mode: DoctorMode::Registry,
            path: None,
            json: true,
        })
    );
    assert_eq!(
        plan(&["doctor", "registry", "workspace"]),
        RouterAction::Error("runx doctor registry does not accept a path".to_owned())
    );
}

#[test]
fn routes_mcp_serve_to_native_plan() {
    assert_eq!(
        plan(&[
            "mcp",
            "fixtures/skills/echo#default",
            "skills/research",
            "--receipt-dir=receipts",
        ]),
        RouterAction::RunMcp(McpPlan {
            run: None,
            authority: None,
            refs: vec![
                MethodRef {
                    skill_path: PathBuf::from("fixtures/skills/echo"),
                    method: Some("default".to_owned()),
                },
                bare_ref("skills/research"),
            ],
            receipt_dir: Some(PathBuf::from("receipts")),
            http_listen: None,
            http_allow_non_loopback: false,
        })
    );
    // A method is named one way, `<ref>#<method>`.
    assert_eq!(
        plan(&["mcp", "fixtures/skills/echo", "--runner", "default"]),
        RouterAction::Error("unknown mcp flag --runner".to_owned())
    );
    assert!(matches!(
        plan(&["mcp", "fixtures/skills/echo#"]),
        RouterAction::Error(message) if message.contains("<ref>#<method>")
    ));
}

fn bare_ref(path: &str) -> MethodRef {
    MethodRef {
        skill_path: PathBuf::from(path),
        method: None,
    }
}

#[test]
fn mcp_http_listen_defaults_to_loopback_and_requires_explicit_non_loopback_opt_in() {
    assert_eq!(
        plan(&["mcp", "fixtures/skills/echo", "--http-listen"]),
        RouterAction::RunMcp(McpPlan {
            run: None,
            authority: None,
            refs: vec![bare_ref("fixtures/skills/echo")],
            receipt_dir: None,
            http_listen: Some("127.0.0.1:8080".to_owned()),
            http_allow_non_loopback: false,
        })
    );
    assert_eq!(
        plan(&[
            "mcp",
            "fixtures/skills/echo",
            "--http-listen=0.0.0.0:8080",
            "--http-allow-non-loopback",
        ]),
        RouterAction::RunMcp(McpPlan {
            run: None,
            authority: None,
            refs: vec![bare_ref("fixtures/skills/echo")],
            receipt_dir: None,
            http_listen: Some("0.0.0.0:8080".to_owned()),
            http_allow_non_loopback: true,
        })
    );
}

#[test]
fn mcp_rejects_unknown_shapes_without_delegating() {
    assert_eq!(
        plan(&["mcp", "fixtures/skills/echo", "--legacy-js-only"]),
        RouterAction::Error("unknown mcp flag --legacy-js-only".to_owned())
    );
    assert_eq!(
        plan(&["mcp"]),
        RouterAction::Error(
            "runx mcp serves skill references, or one run's turns with --run <run-id>".to_owned()
        )
    );
    assert!(matches!(
        plan(&["mcp", "fixtures/skills/echo", "--run", "rx_1"]),
        RouterAction::Error(message) if message.contains("not both")
    ));
    assert_eq!(
        plan(&["mcp", "--run", "rx_1", "--receipt-dir", "receipts"]),
        RouterAction::RunMcp(McpPlan {
            run: Some("rx_1".to_owned()),
            authority: None,
            refs: Vec::new(),
            receipt_dir: Some(PathBuf::from("receipts")),
            http_listen: None,
            http_allow_non_loopback: false,
        })
    );
    assert_eq!(
        plan(&["turn", "rx_1"]),
        RouterAction::Error("unknown command turn".to_owned())
    );
}

#[test]
fn routes_test_to_native_method() {
    assert_eq!(
        plan(&[
            "test",
            "fixtures/test/echo-skill.yaml",
            "--receipt-dir",
            ".runx/receipts",
            "-j"
        ]),
        RouterAction::RunTest(TestPlan {
            fixture_paths: vec!["fixtures/test/echo-skill.yaml".into()],
            receipt_dir: Some(".runx/receipts".into()),
        })
    );
}

#[test]
fn routes_multiple_test_fixtures_to_native_method() {
    assert_eq!(
        plan(&[
            "test",
            "fixtures/test/echo-skill.yaml",
            "fixtures/test/sequential-workflow.yaml",
            "--json",
        ]),
        RouterAction::RunTest(TestPlan {
            fixture_paths: vec![
                "fixtures/test/echo-skill.yaml".into(),
                "fixtures/test/sequential-workflow.yaml".into(),
            ],
            receipt_dir: None,
        })
    );
}

#[test]
fn test_rejects_missing_fixture_path() {
    assert_eq!(
        plan(&["test"]),
        RouterAction::Error("runx test requires a package or a test file".to_owned())
    );
}

#[test]
fn routes_canonical_skill_run_to_native_plan() {
    assert_eq!(
        plan(&[
            "skill",
            "skills/code-intake#intake",
            "--receipt-dir",
            ".runx/receipts",
            "--json",
            "--input",
            "severity=low",
            "-i",
            "thread_title=Docs bug",
        ]),
        RouterAction::RunSkill(Box::new(SkillPlan {
            action: SkillAction::Run,
            skill_path: PathBuf::from("skills/code-intake"),
            method: Some("intake".to_owned()),
            receipt_dir: Some(PathBuf::from(".runx/receipts")),
            run_id: None,
            registry: None,
            expected_digest: None,
            expected_package_digest: None,
            expected_execution_closure_digest: None,
            json: true,
            diagnostics: false,
            inputs: runx_cli::skill::CliInputs {
                texts: [
                    ("thread_title".to_owned(), "Docs bug".to_owned()),
                    ("severity".to_owned(), "low".to_owned()),
                ]
                .into_iter()
                .collect(),
            },
            input_document: None,
            act: None,
            run_authority: None,
            authority: None,
            context: None,
            managed_agent: Default::default(),
        }))
    );
}

#[test]
fn a_host_gives_a_run_and_its_inspection_its_authority() -> Result<(), String> {
    let authority =
        r#"{"actor_ref":{"type":"principal","uri":"runx:principal:user_1"},"max_child_depth":2}"#;
    let RouterAction::RunSkill(run) = plan(&["skill", "skills/echo", "--run-authority", authority])
    else {
        return Err("a run plan".to_owned());
    };
    let authority = run.run_authority.ok_or("the run carries its authority")?;
    assert_eq!(authority.max_child_depth, Some(2));
    assert!(authority.delegations.is_empty());

    assert!(matches!(
        plan(&["skill", "skills/echo", "--run-authority", r#"{"actor":"x"}"#]),
        RouterAction::Error(message) if message.contains("must be a JSON run authority")
    ));
    // Inspect projects readiness under the grants a host would state.
    let RouterAction::RunSkill(inspect) = plan(&[
        "skill",
        "inspect",
        "skills/echo",
        "--run-authority",
        r#"{"grants":{"ausca/inbox":"grant_ausca_1"}}"#,
    ]) else {
        return Err("an inspect plan".to_owned());
    };
    let grants = inspect
        .run_authority
        .ok_or("the inspection carries its authority")?
        .grants;
    assert_eq!(
        grants
            .iter()
            .map(|(connector, grant)| (connector.as_str(), grant.as_str()))
            .collect::<Vec<_>>(),
        [("ausca/inbox", "grant_ausca_1")]
    );
    assert!(matches!(
        plan(&["skill", "skills/echo", "--run-authority", r#"{"grants":{"Not A Name":"grant_1"}}"#]),
        RouterAction::Error(message) if message.contains("must be a JSON run authority")
    ));
    assert!(matches!(
        plan(&["skill", "inspect", "skills/echo", "--max-spend", "1"]),
        RouterAction::Error(message) if message.contains("not accepted by inspect")
    ));
    Ok(())
}

#[test]
fn skill_rejects_resolver_flags_for_management_actions() {
    for action in ["publish", "search", "validate"] {
        assert_eq!(
            plan(&["skill", action, "--registry", "https://registry.runx.test"]),
            RouterAction::Error(
                "runx skill --registry, --digest, --package-digest, and --execution-closure-digest are only supported when running a skill ref"
                    .to_owned()
            ),
            "{action}"
        );
        assert_eq!(
            plan(&["skill", action, "--digest", "sha256:abc"]),
            RouterAction::Error(
                "runx skill --registry, --digest, --package-digest, and --execution-closure-digest are only supported when running a skill ref"
                    .to_owned()
            ),
            "{action}"
        );
    }
}

/// One setup noun and one install: `connect`, `credential` and `add` are gone.
#[test]
fn there_is_no_connect_credential_or_add_command() {
    for command in ["connect", "credential", "add", "url-add"] {
        assert_eq!(
            plan(&[command, "list"]),
            RouterAction::Error(format!("unknown command {command}")),
            "{command}"
        );
    }
}

#[test]
fn routes_export_to_native_plan() {
    assert_eq!(
        plan(&["export", "claude", "brand-voice", "--project", "--json"]),
        RouterAction::RunExport(ExportPlan {
            target: Target::Claude,
            refs: vec!["brand-voice".to_owned()],
            project: true,
            json: true,
        })
    );
    assert_eq!(
        plan(&["export", "codex"]),
        RouterAction::RunExport(ExportPlan {
            target: Target::Codex,
            refs: Vec::new(),
            project: false,
            json: false,
        })
    );
}

#[test]
fn export_rejects_unknown_target_and_flags() {
    assert_eq!(
        plan(&["export", "vscode"]),
        RouterAction::Error("runx export target must be claude or codex, got vscode".to_owned())
    );
    assert_eq!(
        plan(&["export", "claude", "--project=true"]),
        RouterAction::Error("--project does not take a value".to_owned())
    );
}

#[test]
fn routes_config_to_native_plan() {
    assert_eq!(
        plan(&["config", "set", "agent.model", "gpt-test", "--json"]),
        RouterAction::RunConfig(ConfigPlan {
            action: ConfigAction::Set,
            key: Some("agent.model".to_owned()),
            value: Some("gpt-test".to_owned()),
            json: true,
        })
    );
}

/// Development evaluators and the receipt notary have no command of the
/// old names: the evaluators are build tools, and a receipt is notarized.
#[test]
fn the_shipped_cli_has_no_eval_or_receipt_publish_command() {
    for command in ["kernel", "parser", "publish"] {
        assert_eq!(
            plan(&[command, "eval"]),
            RouterAction::Error(format!("unknown command {command}"))
        );
    }
}

#[test]
fn routes_doctor_history_list_new_and_init_to_native_plans() {
    assert_eq!(
        plan(&[
            "doctor",
            "fixtures/doctor/empty-success/workspace",
            "--json"
        ]),
        RouterAction::RunDoctor(DoctorPlan {
            mode: DoctorMode::Workspace,
            path: Some(PathBuf::from("fixtures/doctor/empty-success/workspace")),
            json: true,
        })
    );
    assert_eq!(
        plan(&["doctor", "authority", "--json"]),
        RouterAction::RunDoctor(DoctorPlan {
            mode: DoctorMode::Authority,
            path: None,
            json: true,
        })
    );
    assert_eq!(
        plan(&["history", "sourcey", "--json"]),
        RouterAction::RunHistory(HistoryPlan {
            args: vec!["history".into(), "sourcey".into(), "--json".into()],
        })
    );
    assert_eq!(
        plan(&[
            "resume",
            "run_123",
            "answers.json",
            "--receipt-dir",
            "receipts",
            "-j",
        ]),
        RouterAction::RunResume(ResumePlan {
            run_id: "run_123".to_owned(),
            answers_path: PathBuf::from("answers.json"),
            receipt_dir: Some(PathBuf::from("receipts")),
            expected_package_digest: None,
            expected_execution_closure_digest: None,
            json: true,
            diagnostics: false,
            managed_agent: Default::default(),
        })
    );
    assert_eq!(
        plan(&["list", "packets", "--only", "valid", "--json"]),
        RouterAction::RunList(ListPlan {
            kind: ListKind::Packets,
            filter: FilterMode::Valid,
            json: true,
        })
    );
    assert_eq!(
        plan(&["list", "skills", "--only=invalid"]),
        RouterAction::RunList(ListPlan {
            kind: ListKind::Skills,
            filter: FilterMode::Invalid,
            json: false,
        })
    );
    assert!(matches!(
        plan(&["list", "--only", "ok"]),
        RouterAction::Error(message) if message.contains("valid or invalid")
    ));
    // A repository or a skills/ directory marks a project; nothing is
    // initialized to mark one.
    assert_eq!(
        plan(&["init"]),
        RouterAction::Error("unknown command init".to_owned())
    );
}

#[test]
fn managed_agent_rounds_need_explicit_managed_agent_consent() {
    assert_eq!(
        plan(&["skill", "skills/docs-demo", "--drive-rounds", "2"]),
        RouterAction::Error("runx skill --drive-rounds requires --drive".to_owned())
    );
    let action = plan(&[
        "skill",
        "skills/docs-demo",
        "--drive",
        "--drive-rounds",
        "2",
    ]);
    assert!(matches!(action, RouterAction::RunSkill(_)));
    if let RouterAction::RunSkill(skill) = action {
        assert_eq!(
            skill.managed_agent,
            runx_runtime::ManagedAgentPolicy::Inline { max_rounds: 2 }
        );
    }
}

/// `runx test` runs every test; there is no second test loop.
#[test]
fn there_is_no_dev_command() {
    assert_eq!(
        plan(&["dev"]),
        RouterAction::Error("unknown command dev".to_owned())
    );
}

#[test]
fn unsupported_doctor_and_list_shapes_fail_closed() {
    assert_eq!(
        plan(&["doctor", "--fix"]),
        RouterAction::Error("unknown doctor flag --fix".to_owned())
    );
    assert_eq!(
        plan(&["doctor", "authority", "workspace"]),
        RouterAction::Error("runx doctor authority does not accept a path".to_owned())
    );
    assert_eq!(
        plan(&["list", "skills", "--source", "registry"]),
        RouterAction::Error("unknown list flag --source".to_owned())
    );
}

#[test]
fn routes_registry_to_native_plan() {
    assert_eq!(
        plan(&[
            "registry",
            "search",
            "echo",
            "--registry",
            "https://registry.runx.test",
            "--limit",
            "10",
            "--json",
        ]),
        RouterAction::RunRegistry(RegistryPlan {
            action: RegistryAction::Search,
            subject: "echo".to_owned(),
            registry: Some("https://registry.runx.test".to_owned()),
            version: None,
            expected_digest: None,
            publisher: None,
            limit: Some(10),
            records: None,
            json: true,
        })
    );
    assert_eq!(
        plan(&["registry", "package", "skills/example", "--json"]),
        RouterAction::RunRegistry(RegistryPlan {
            action: RegistryAction::Package,
            subject: "skills/example".to_owned(),
            registry: None,
            version: None,
            expected_digest: None,
            publisher: None,
            limit: None,
            records: None,
            json: true,
        })
    );
    assert_eq!(
        plan(&[
            "registry",
            "package",
            "skills/example",
            "--registry",
            "https://runx.example",
        ]),
        RouterAction::Error("runx registry package accepts only --json".to_owned())
    );
}

/// Records a host holds install in place of a ref, from one JSON array in a
/// file or on stdin; a ref beside them, a second document or a flag only a
/// ref takes is refused.
#[test]
fn registry_install_takes_records_in_place_of_a_ref() {
    for (source, expected) in [
        (
            "records.json",
            runx_cli::document_input::DocumentInputSource::Path(PathBuf::from("records.json")),
        ),
        ("-", runx_cli::document_input::DocumentInputSource::Stdin),
    ] {
        assert_eq!(
            plan(&["registry", "install", "--records", source, "--json"]),
            RouterAction::RunRegistry(RegistryPlan {
                action: RegistryAction::Install,
                subject: String::new(),
                registry: None,
                version: None,
                expected_digest: None,
                publisher: None,
                limit: None,
                records: Some(expected),
                json: true,
            })
        );
    }
    assert_eq!(
        plan(&["registry", "install", "acme/echo@1.0.0", "--records", "-"]),
        RouterAction::Error(
            "runx registry install --records takes no ref: each record names the version it installs"
                .to_owned()
        )
    );
    assert_eq!(
        plan(&[
            "registry",
            "install",
            "--records",
            "a.json",
            "--records",
            "b.json"
        ]),
        RouterAction::Error(
            "runx registry install accepts exactly one --records document".to_owned()
        )
    );
    assert_eq!(
        plan(&["registry", "install", "--records", "-", "--digest", "sha256:abc"]),
        RouterAction::Error(
            "runx registry install accepts only --records, --registry and --json when it installs records"
                .to_owned()
        )
    );
}

/// A package is its directory, its version is its run.yaml's and its tier
/// is the registry's to grant: no flag names any of them.
#[test]
fn registry_refuses_the_flags_a_package_or_its_registry_owns() {
    for (verb, flag) in [
        ("publish", vec!["--version", "1.0.0"]),
        ("publish", vec!["--trust-tier", "verified"]),
        ("publish", vec!["--upsert"]),
        ("publish", vec!["--profile", "run.yaml"]),
        ("package", vec!["--profile", "run.yaml"]),
    ] {
        let mut args = vec!["registry", verb, "skills/example"];
        args.extend(flag.iter().copied());
        assert_eq!(
            plan(&args),
            RouterAction::Error(format!("unknown registry flag {}", flag[0])),
            "{verb} {flag:?}"
        );
    }
}

#[test]
fn routes_registry_install_for_either_kind_and_refuses_retired_verbs() {
    let digest = format!("sha256:{}", "a".repeat(64));
    assert_eq!(
        plan(&[
            "registry",
            "install",
            "acme/ops-api@1.0.0",
            "--registry",
            "https://runx.example",
            "--digest",
            &digest,
            "--json",
        ]),
        RouterAction::RunRegistry(RegistryPlan {
            action: RegistryAction::Install,
            subject: "acme/ops-api".to_owned(),
            registry: Some("https://runx.example".to_owned()),
            version: Some("1.0.0".to_owned()),
            expected_digest: Some(digest.clone()),
            publisher: None,
            limit: None,
            records: None,
            json: true,
        })
    );
    assert_eq!(
        plan(&["registry", "install", "acme/ops-api", "--to", "dir"]),
        RouterAction::Error("unknown registry flag --to".to_owned())
    );
    assert_eq!(
        plan(&["registry", "install", "acme/ops-api", "--version", "1.0.0"]),
        RouterAction::Error("unknown registry flag --version".to_owned())
    );
    assert_eq!(
        plan(&[
            "registry",
            "publish",
            "connectors/ops-api",
            "--digest",
            &digest
        ]),
        RouterAction::Error(
            "runx registry publish accepts only --registry, --publisher and --json".to_owned()
        )
    );
    for retired in ["read", "resolve", "connector"] {
        assert_eq!(
            plan(&["registry", retired, "acme/ops-api"]),
            RouterAction::Error(format!("unknown registry subcommand {retired}")),
            "{retired}"
        );
    }
}

#[test]
fn rejects_zero_registry_limit() {
    assert_eq!(
        plan(&["registry", "search", "echo", "--limit", "0"]),
        RouterAction::Error("--limit must be greater than zero".to_owned())
    );
}

#[test]
fn rejects_unsafe_registry_version() {
    assert_eq!(
        plan(&["registry", "install", "acme/echo@../bad"]),
        RouterAction::Error(
            runx_contracts::validate_registry_version("../bad")
                .err()
                .unwrap_or_default()
        )
    );
    assert!(matches!(
        plan(&["registry", "install", "acme/echo@-rf"]),
        RouterAction::Error(message) if message.contains("starts with an ASCII letter or digit")
    ));
}

#[test]
fn every_command_that_prints_json_reads_dash_j_as_json() {
    assert!(matches!(
        plan(&["registry", "install", "acme/echo@1.0.0", "-j"]),
        RouterAction::RunRegistry(plan) if plan.json && plan.subject == "acme/echo"
    ));
    assert!(matches!(
        plan(&["export", "claude", "-j"]),
        RouterAction::RunExport(plan) if plan.json
    ));
}

#[test]
fn routes_tool_to_native_plan_and_rejects_unknown_subcommands() {
    assert_eq!(
        plan(&[
            "tool", "search", "echo", "writer", "--source", "native", "--json",
        ]),
        RouterAction::RunTool(ToolPlan {
            action: ToolAction::Search,
            ref_or_query: "echo writer".to_owned(),
            source: Some("native".to_owned()),
            json: true,
        })
    );
    assert_eq!(
        plan(&["tool", "publish", "fixture.echo"]),
        RouterAction::Error("unknown tool subcommand publish".to_owned())
    );
}

#[test]
fn native_router_argument_errors_exit_with_usage_code() -> Result<(), Box<dyn std::error::Error>> {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_runx"))
        .args(["notarize", "--json"])
        .output()?;

    assert_eq!(output.status.code(), Some(64));
    assert_eq!(String::from_utf8(output.stderr)?, "");
    let value = serde_json::from_slice::<serde_json::Value>(&output.stdout)?;
    assert_eq!(value["status"], "failure");
    assert_eq!(value["error"]["code"], "invalid_args");
    assert!(
        value["error"]["message"].as_str().is_some_and(|message| {
            message.contains("runx notarize requires a receipt JSON path")
        })
    );
    Ok(())
}

#[test]
fn package_manifest_is_native_binary_shaped() -> Result<(), Box<dyn std::error::Error>> {
    let package_json = fs::read_to_string(repo_root()?.join("packages/cli/package.json"))?;
    let manifest = serde_json::from_str::<serde_json::Value>(&package_json)?;
    assert_eq!(manifest["bin"]["runx"], "./bin/runx");
    assert_eq!(
        manifest["files"],
        serde_json::json!(["LICENSE", "bin/runx", "native/supported-platforms.json"])
    );
    assert!(manifest.get("main").is_none());
    assert!(manifest.get("types").is_none());
    assert!(manifest.get("dependencies").is_none());
    assert!(manifest.get("exports").is_none());
    assert!(manifest.get("scripts").is_none());
    assert_not_contains(&package_json, "workspace:");
    assert_not_contains(&package_json, "runtime-local");
    Ok(())
}

fn repo_root() -> Result<PathBuf, Box<dyn std::error::Error>> {
    Ok(Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()?)
}

fn assert_help_line(help: &str, expected: &str) {
    assert!(
        help.lines().any(|line| line.trim() == expected),
        "missing help line: {expected}"
    );
}

fn assert_not_contains(contents: &str, needle: &str) {
    assert!(
        !contents.contains(needle),
        "packaged CLI must not contain {needle}"
    );
}
